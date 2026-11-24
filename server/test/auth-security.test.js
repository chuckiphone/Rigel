import assert from 'node:assert/strict';
import test from 'node:test';
import Fastify from 'fastify';
import { SignJWT } from 'jose';

import { buildApp } from '../src/app.js';
import { loadConfig } from '../src/config.js';
import { registerAuthRoutes } from '../src/routes/auth.js';
import {
  hashPassword,
  safeEqual,
  signAccessToken,
  validPassword,
  verifyAccessToken,
} from '../src/security.js';

const ownerId = '00000000-0000-4000-8000-000000000001';
const sessionId = '00000000-0000-4000-8000-000000000002';
const jwtSecret = 'test-secret-that-is-long-enough-for-hs256-signing';

const user = {
  id: ownerId,
  email: 'owner@example.com',
  email_verified: true,
  reset_required: false,
  role: 'user',
  daily_llm_micros: 0,
  created_at: new Date().toISOString(),
  last_login_at: null,
};

async function accessToken(sid = sessionId) {
  return signAccessToken({
    secret: jwtSecret,
    userId: ownerId,
    sessionId: sid,
    email: user.email,
    minutes: 15,
  });
}

test('a whitespace-only password is never a valid credential', () => {
  assert.equal(validPassword('          '), false);
  assert.equal(validPassword('\n\t        '), false);
  assert.equal(validPassword('correct horse battery staple'), true);
});

test('relay tokens compare through fixed-length digests and reject a near miss', () => {
  assert.equal(safeEqual('relay-token', 'relay-token'), true);
  assert.equal(safeEqual('relay-token', 'relay-tokeN'), false);
  assert.equal(safeEqual('relay-token', 'relay-token '), false);
});

test('an HS384 token is refused even when it is signed with the API secret', async () => {
  const token = await new SignJWT({ sid: sessionId, email: user.email })
    .setProtectedHeader({ alg: 'HS384', typ: 'JWT' })
    .setSubject(ownerId)
    .setIssuer('rigel-cloud')
    .setAudience('rigel-api')
    .setIssuedAt()
    .setExpirationTime('15m')
    .sign(new TextEncoder().encode(jwtSecret));
  await assert.rejects(() => verifyAccessToken(jwtSecret, token));
});

async function makeApp({ active = true } = {}) {
  const calls = [];
  const db = {
    async query(sql, values) {
      calls.push({ sql, values });
      if (sql.includes('JOIN auth_sessions')) {
        return { rows: active ? [user] : [] };
      }
      return { rows: [] };
    },
  };
  const app = await buildApp({
    config: {
      production: false,
      jwtSecret,
      allowedOrigins: [],
      relayTokens: {},
      accessMinutes: 15,
      refreshDays: 30,
    },
    pool: db,
    mailer: {},
    logger: false,
  });
  return { app, calls };
}

test('production does not accept a localhost browser origin by default', () => {
  const config = loadConfig({
    NODE_ENV: 'production',
    DATABASE_URL: 'postgres://rigel:test@db/rigel',
    JWT_SECRET: 'x'.repeat(32),
    APP_URL: 'https://askrigel.com',
    API_URL: 'https://api.askrigel.com',
  });
  assert.equal(config.allowedOrigins.some((origin) => origin.includes('localhost')), false);
  assert.equal(config.allowedOrigins.includes('https://askrigel.com'), true);
});

test('a revoked session cannot use its still-unexpired access token', async () => {
  const token = await accessToken();
  const active = await makeApp({ active: true });
  const revoked = await makeApp({ active: false });
  try {
    const allowed = await active.app.inject({
      method: 'GET',
      url: '/v1/auth/me',
      headers: { authorization: `Bearer ${token}` },
    });
    assert.equal(allowed.statusCode, 200);
    assert.deepEqual(active.calls[0].values, [ownerId, sessionId]);

    const denied = await revoked.app.inject({
      method: 'GET',
      url: '/v1/auth/me',
      headers: { authorization: `Bearer ${token}` },
    });
    assert.equal(denied.statusCode, 401);
    assert.deepEqual(denied.json(), { error: 'invalid_token' });
  } finally {
    await active.app.close();
    await revoked.app.close();
  }
});

test('a token naming no session is refused before any session query', async () => {
  const { app, calls } = await makeApp();
  try {
    const token = await accessToken('not-a-session-id');
    const response = await app.inject({
      method: 'GET',
      url: '/v1/auth/me',
      headers: { authorization: `Bearer ${token}` },
    });
    assert.equal(response.statusCode, 401);
    assert.deepEqual(calls, []);
  } finally {
    await app.close();
  }
});

test('account deletion requires the current password', async () => {
  const passwordHash = await hashPassword('correct horse battery staple');
  let deleted = false;
  const app = Fastify({ logger: false });
  app.decorate('config', { production: false, refreshDays: 30 });
  app.decorate('db', {
    async query(sql, values) {
      if (sql.includes('SELECT password_hash FROM users')) {
        return { rows: values[0] === ownerId ? [{ password_hash: passwordHash }] : [] };
      }
      if (sql.includes('DELETE FROM users')) {
        deleted = true;
        return { rows: [] };
      }
      return { rows: [] };
    },
  });
  app.decorate('mailer', {});
  app.decorate('authenticate', async (request) => {
    request.user = { id: ownerId, email: user.email };
  });
  await registerAuthRoutes(app);
  await app.ready();
  try {
    const wrong = await app.inject({
      method: 'DELETE',
      url: '/v1/auth/account',
      payload: { confirm: 'DELETE', password: 'not the password' },
    });
    assert.equal(wrong.statusCode, 403);
    assert.deepEqual(wrong.json(), { error: 'invalid_current_password' });
    assert.equal(deleted, false);

    const missing = await app.inject({
      method: 'DELETE',
      url: '/v1/auth/account',
      payload: { confirm: 'DELETE' },
    });
    assert.equal(missing.statusCode, 403);
    assert.equal(deleted, false);

    const correct = await app.inject({
      method: 'DELETE',
      url: '/v1/auth/account',
      payload: { confirm: 'DELETE', password: 'correct horse battery staple' },
    });
    assert.equal(correct.statusCode, 204);
    assert.equal(deleted, true);
  } finally {
    await app.close();
  }
});
