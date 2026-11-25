import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { PGlite } from '@electric-sql/pglite';
import Fastify from 'fastify';
import formbody from '@fastify/formbody';
import { SignJWT, exportJWK, generateKeyPair } from 'jose';
import * as samlify from 'samlify';
import * as samlXsd from '@authenio/samlify-node-xmllint';
import selfsigned from 'selfsigned';

import { buildApp } from '../src/app.js';
import { hashPassword } from '../src/security.js';
import { loadSso } from '../src/sso.js';

samlify.setSchemaValidator(samlXsd);

const idpPair = await selfsigned.generate(
  [{ name: 'commonName', value: 'rigel-test-idp' }],
  { days: 3650, keySize: 2048, algorithm: 'sha256' },
);
const spPair = await selfsigned.generate(
  [{ name: 'commonName', value: 'rigel-test-sp' }],
  { days: 3650, keySize: 2048, algorithm: 'sha256' },
);
const idpCert = idpPair.cert;
const idpKey = idpPair.private;
const spCert = spPair.cert;
const spKey = spPair.private;
const jwtSecret = 'test-secret-that-is-long-enough-for-hs256-signing';
const apiUrl = 'http://127.0.0.1:8080';
const spEntityId = `${apiUrl}/v1/auth/sso/saml/metadata`;
const spAcsUrl = `${apiUrl}/v1/auth/sso/saml/acs`;
const samlIssuer = 'urn:rigel:test:idp';
const samlEntryPoint = 'https://idp.example.test/sso';

function sql(name) {
  return readFileSync(fileURLToPath(new URL(`../sql/${name}.sql`, import.meta.url)), 'utf8').replace(
    /CREATE EXTENSION IF NOT EXISTS pgcrypto;/g,
    '',
  );
}

async function makePool() {
  const pglite = new PGlite();
  for (const name of ['001_init', '002_relay', '003_sso']) await pglite.exec(sql(name));
  const query = async (statement, values = []) => {
    const result = await pglite.query(statement, values);
    return { ...result, rowCount: result.affectedRows ?? result.rows.length };
  };
  const client = { query, release() {} };
  return {
    query,
    connect: async () => client,
    end: async () => pglite.close(),
    pglite,
  };
}

function firstCookie(response, name = 'rigel_sso_state') {
  const header = response.headers['set-cookie'];
  const values = Array.isArray(header) ? header : [header];
  const value = values.find((row) => String(row).startsWith(`${name}=`));
  return value ? String(value).split(';')[0] : '';
}

async function makeApp({ providers, breakGlassAdmins = [] }) {
  const sso = loadSso(
    {
      SSO_PROVIDERS: JSON.stringify(providers),
      SSO_SP_CERT: spCert,
      SSO_SP_KEY: spKey,
      SSO_BREAK_GLASS_ADMINS: breakGlassAdmins.join(','),
    },
    { production: false, apiUrl },
  );
  const pool = await makePool();
  const config = {
    production: false,
    jwtSecret,
    allowedOrigins: [],
    relayTokens: {},
    accessMinutes: 15,
    refreshDays: 30,
    appUrl: 'https://app.example',
    apiUrl,
    llmDefaultDailyMicros: 2_000_000,
    sso,
  };
  const app = await buildApp({ config, pool, mailer: {}, logger: false });
  return { app, pool, sso };
}

function oidcProvider(issuer) {
  return {
    id: 'acme-oidc',
    type: 'oidc',
    label: 'Acme Okta',
    issuer,
    client_id: 'rigel-client',
    client_secret: 'rigel-secret',
    domains: ['example.com'],
    enforce: true,
  };
}

function samlProvider() {
  return {
    id: 'acme-saml',
    type: 'saml',
    label: 'Acme SAML',
    entry_point: samlEntryPoint,
    issuer: samlIssuer,
    cert: idpCert,
    domains: ['example.com'],
    enforce: true,
  };
}

function testSp() {
  return samlify.ServiceProvider({
    entityID: spEntityId,
    assertionConsumerService: [
      { Binding: samlify.Constants.namespace.binding.post, Location: spAcsUrl },
    ],
    privateKey: spKey,
    signingCert: spCert,
    authnRequestsSigned: true,
    wantAssertionsSigned: true,
    wantMessageSigned: false,
    allowUnsolicitedResponse: false,
  });
}

function testIdp() {
  return samlify.IdentityProvider({
    entityID: samlIssuer,
    privateKey: idpKey,
    signingCert: idpCert,
    wantAuthnRequestsSigned: true,
    singleSignOnService: [
      { Binding: samlify.Constants.namespace.binding.redirect, Location: samlEntryPoint },
    ],
  });
}

async function startOidcProvider({ email = 'owner@example.com', emailVerified = true, nonceOverride } = {}) {
  const { publicKey, privateKey } = await generateKeyPair('RS256');
  const jwk = await exportJWK(publicKey);
  Object.assign(jwk, { kid: 'rigel-test-key', alg: 'RS256', use: 'sig' });
  const codes = new Map();
  let issuer = '';

  const provider = Fastify({ logger: false });
  await provider.register(formbody);
  provider.get('/.well-known/openid-configuration', async () => ({
    issuer,
    authorization_endpoint: `${issuer}/authorize`,
    token_endpoint: `${issuer}/token`,
    jwks_uri: `${issuer}/jwks`,
    response_types_supported: ['code'],
    subject_types_supported: ['public'],
    id_token_signing_alg_values_supported: ['RS256'],
  }));
  provider.get('/jwks', async () => ({ keys: [jwk] }));
  provider.get('/authorize', async (request, reply) => {
    const query = request.query ?? {};
    assert.equal(query.client_id, 'rigel-client');
    assert.equal(query.response_type, 'code');
    assert.equal(query.code_challenge_method, 'S256');
    assert.ok(query.code_challenge);
    assert.ok(query.nonce);
    const code = `code-${codes.size + 1}`;
    codes.set(code, {
      challenge: query.code_challenge,
      nonce: nonceOverride ?? query.nonce,
      redirectUri: query.redirect_uri,
    });
    const target = new URL(query.redirect_uri);
    target.searchParams.set('code', code);
    target.searchParams.set('state', query.state);
    return reply.redirect(target.toString());
  });
  provider.post('/token', async (request, reply) => {
    const body = request.body ?? {};
    const entry = codes.get(body.code);
    if (!entry) return reply.code(400).send({ error: 'invalid_grant' });
    assert.equal(body.grant_type, 'authorization_code');
    assert.equal(body.client_id, 'rigel-client');
    assert.equal(body.client_secret, 'rigel-secret');
    assert.equal(body.redirect_uri, entry.redirectUri);
    const challenge = createHash('sha256').update(String(body.code_verifier)).digest('base64url');
    if (challenge !== entry.challenge) return reply.code(400).send({ error: 'invalid_grant' });
    const idToken = await new SignJWT({
      email,
      email_verified: emailVerified,
      nonce: entry.nonce,
    })
      .setProtectedHeader({ alg: 'RS256', kid: 'rigel-test-key', typ: 'JWT' })
      .setSubject('oidc-subject-1')
      .setIssuer(issuer)
      .setAudience('rigel-client')
      .setIssuedAt()
      .setExpirationTime('5m')
      .sign(privateKey);
    return { id_token: idToken, token_type: 'Bearer', expires_in: 300 };
  });
  const address = await provider.listen({ port: 0, host: '127.0.0.1' });
  issuer = address.replace(/\/$/, '');
  return { provider, issuer, close: () => provider.close() };
}

async function startSso(app, type = 'oidc') {
  const start = await app.inject({
    method: 'GET',
    url: `/v1/auth/sso/${type === 'oidc' ? 'acme-oidc' : 'acme-saml'}/start?return_to=/welcome%3Ftab%3Dhome`,
  });
  assert.equal(start.statusCode, 302);
  const cookie = firstCookie(start);
  assert.match(cookie, /^rigel_sso_state=/);
  return { start, cookie, location: start.headers.location };
}

test('OIDC discovery, login round trip, refresh, identity link and one-time state', async () => {
  const provider = await startOidcProvider();
  const { app, pool } = await makeApp({ providers: [oidcProvider(provider.issuer)] });
  try {
    const discovery = await app.inject({
      method: 'POST',
      url: '/v1/auth/sso/discover',
      payload: { email: 'owner@example.com' },
    });
    assert.equal(discovery.statusCode, 200);
    assert.equal(discovery.json().provider.id, 'acme-oidc');
    assert.equal(discovery.json().provider.enforced, true);

    const { cookie, location } = await startSso(app);
    const authorize = await fetch(location, { redirect: 'manual' });
    assert.equal(authorize.status, 302);
    const callbackLocation = authorize.headers.get('location');
    assert.ok(callbackLocation, 'the provider returns a callback URL');
    const callback = new URL(callbackLocation);
    const done = await app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
      headers: { cookie },
    });
    assert.equal(done.statusCode, 302);
    assert.equal(done.headers.location, 'https://app.example/welcome?tab=home&sso=ok');

    const refreshCookie = firstCookie(done, 'rigel_refresh');
    assert.match(refreshCookie, /^rigel_refresh=/);
    const refreshed = await app.inject({
      method: 'POST',
      url: '/v1/auth/refresh',
      headers: { cookie: refreshCookie },
    });
    assert.equal(refreshed.statusCode, 200);
    assert.equal(refreshed.json().user.email, 'owner@example.com');
    assert.equal(refreshed.json().user.email_verified, true);

    const me = await app.inject({
      method: 'GET',
      url: '/v1/auth/me',
      headers: { authorization: `Bearer ${refreshed.json().access_token}` },
    });
    assert.equal(me.statusCode, 200);
    assert.equal(me.json().user.email, 'owner@example.com');

    const linked = await pool.query(
      `SELECT s.provider_id, s.subject, u.email
         FROM sso_identities s JOIN users u ON u.id = s.user_id`,
    );
    assert.deepEqual(linked.rows, [
      { provider_id: 'acme-oidc', subject: 'oidc-subject-1', email: 'owner@example.com' },
    ]);
    const audit = await pool.query(
      `SELECT detail FROM audit_log WHERE action = 'sso_login' ORDER BY created_at DESC LIMIT 1`,
    );
    assert.equal(audit.rows[0].detail.provider_type, 'oidc');

    const replay = await app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
      headers: { cookie },
    });
    assert.equal(replay.statusCode, 400);
    assert.deepEqual(replay.json(), { error: 'invalid_sso_state' });
  } finally {
    await app.close();
    await pool.end();
    await provider.close();
  }
});

test('password login is refused for an enforced domain, but a listed admin can break glass', async () => {
  const { app, pool } = await makeApp({
    providers: [oidcProvider('https://idp.example.test')],
    breakGlassAdmins: ['owner@example.com', 'admin@example.com'],
  });
  try {
    const password = await hashPassword('correct horse battery staple');
    await pool.query(
      `INSERT INTO users (email, password_hash, email_verified, role)
       VALUES ('owner@example.com', $1, TRUE, 'user'),
              ('admin@example.com', $1, TRUE, 'admin')`,
      [password],
    );

    const blocked = await app.inject({
      method: 'POST',
      url: '/v1/auth/login',
      payload: { email: 'owner@example.com', password: 'correct horse battery staple' },
    });
    assert.equal(blocked.statusCode, 403);
    assert.deepEqual(blocked.json(), { error: 'sso_required', provider_id: 'acme-oidc' });

    const signup = await app.inject({
      method: 'POST',
      url: '/v1/auth/signup',
      payload: { email: 'new@example.com', password: 'correct horse battery staple' },
    });
    assert.equal(signup.statusCode, 403);
    assert.deepEqual(signup.json(), { error: 'sso_required', provider_id: 'acme-oidc' });

    const admin = await app.inject({
      method: 'POST',
      url: '/v1/auth/login',
      headers: { 'x-rigel-native': '1' },
      payload: { email: 'admin@example.com', password: 'correct horse battery staple' },
    });
    assert.equal(admin.statusCode, 200);
    assert.equal(admin.json().user.role, 'admin');
    const row = await pool.query(
      `SELECT detail FROM audit_log WHERE action = 'login' AND user_id =
         (SELECT id FROM users WHERE email = 'admin@example.com')`,
    );
    assert.equal(row.rows[0].detail.break_glass, true);
  } finally {
    await app.close();
    await pool.end();
  }
});

test('an unknown email domain is not offered an SSO provider', async () => {
  const { app, pool } = await makeApp({ providers: [oidcProvider('https://idp.example.test')] });
  try {
    const response = await app.inject({
      method: 'POST',
      url: '/v1/auth/sso/discover',
      payload: { email: 'owner@other.example' },
    });
    assert.equal(response.statusCode, 404);
    assert.deepEqual(response.json(), { error: 'sso_not_configured' });
  } finally {
    await app.close();
    await pool.end();
  }
});

test('OIDC failure paths: state mismatch, nonce mismatch and an unverified email', async () => {
  const provider = await startOidcProvider({ emailVerified: false });
  const { app, pool } = await makeApp({ providers: [oidcProvider(provider.issuer)] });
  try {
    const { cookie, location } = await startSso(app);
    const authorize = await fetch(location, { redirect: 'manual' });
    const callback = new URL(authorize.headers.get('location'));

    const noCookie = await app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
    });
    assert.equal(noCookie.statusCode, 400);
    assert.deepEqual(noCookie.json(), { error: 'invalid_sso_state' });

    const wrongCookie = await app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
      headers: { cookie: 'rigel_sso_state=not-the-state' },
    });
    assert.equal(wrongCookie.statusCode, 400);
    assert.deepEqual(wrongCookie.json(), { error: 'invalid_sso_state' });

    const unverified = await app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
      headers: { cookie },
    });
    assert.equal(unverified.statusCode, 403);
    assert.deepEqual(unverified.json(), { error: 'email_not_verified' });
  } finally {
    await app.close();
    await pool.end();
    await provider.close();
  }

  const mismatch = await startOidcProvider({ nonceOverride: 'nonce-from-another-login' });
  const second = await makeApp({ providers: [oidcProvider(mismatch.issuer)] });
  try {
    const { cookie, location } = await startSso(second.app);
    const authorize = await fetch(location, { redirect: 'manual' });
    const callback = new URL(authorize.headers.get('location'));
    const rejected = await second.app.inject({
      method: 'GET',
      url: `${callback.pathname}${callback.search}`,
      headers: { cookie },
    });
    assert.equal(rejected.statusCode, 400);
    assert.deepEqual(rejected.json(), { error: 'sso_authentication_failed' });
  } finally {
    await second.app.close();
    await second.pool.end();
    await mismatch.close();
  }
});

test('SAML login round trip validates the signed assertion and rejects a tampered one', async () => {
  const { app, pool } = await makeApp({ providers: [samlProvider()] });
  try {
    const metadata = await app.inject({ method: 'GET', url: '/v1/auth/sso/saml/metadata' });
    assert.equal(metadata.statusCode, 200);
    assert.match(metadata.headers['content-type'], /application\/xml/);
    assert.match(metadata.body, /EntityDescriptor/);
    assert.match(metadata.body, /AssertionConsumerService/);

    const sp = testSp();
    const idp = testIdp();
    const { cookie, location } = await startSso(app, 'saml');
    const startUrl = new URL(location);
    assert.equal(startUrl.origin + startUrl.pathname, samlEntryPoint);
    const query = Object.fromEntries(startUrl.searchParams);
    const octetString = startUrl.search
      .slice(1)
      .split('&')
      .filter((pair) => /^(SAMLRequest|RelayState|SigAlg)=/.test(pair))
      .join('&');
    assert.ok(query.SAMLRequest);
    assert.ok(query.RelayState);

    const parsed = await idp.parseLoginRequest(sp, 'redirect', { query, octetString });
    const response = await idp.createLoginResponse(
      sp,
      parsed,
      'post',
      { email: 'owner@example.com' },
      { relayState: query.RelayState },
    );

    const done = await app.inject({
      method: 'POST',
      url: '/v1/auth/sso/saml/acs',
      headers: { cookie, 'content-type': 'application/x-www-form-urlencoded' },
      payload: new URLSearchParams({
        SAMLResponse: response.context,
        RelayState: query.RelayState,
      }).toString(),
    });
    assert.equal(done.statusCode, 302);
    assert.equal(done.headers.location, 'https://app.example/welcome?tab=home&sso=ok');
    const refreshCookie = firstCookie(done, 'rigel_refresh');
    const refreshed = await app.inject({
      method: 'POST',
      url: '/v1/auth/refresh',
      headers: { cookie: refreshCookie },
    });
    assert.equal(refreshed.statusCode, 200);
    assert.equal(refreshed.json().user.email, 'owner@example.com');
    const replayRows = await pool.query('SELECT provider_id, replay_key FROM sso_replays');
    assert.equal(replayRows.rows.length, 1);
    assert.equal(replayRows.rows[0].provider_id, 'acme-saml');
    assert.match(replayRows.rows[0].replay_key, /_/);

    const replay = await app.inject({
      method: 'POST',
      url: '/v1/auth/sso/saml/acs',
      headers: { cookie, 'content-type': 'application/x-www-form-urlencoded' },
      payload: new URLSearchParams({
        SAMLResponse: response.context,
        RelayState: query.RelayState,
      }).toString(),
    });
    assert.equal(replay.statusCode, 400);
    assert.deepEqual(replay.json(), { error: 'invalid_sso_state' });

    const tamperedStart = await startSso(app, 'saml');
    const tamperedUrl = new URL(tamperedStart.location);
    const tamperedQuery = Object.fromEntries(tamperedUrl.searchParams);
    const tamperedOctet = tamperedUrl.search
      .slice(1)
      .split('&')
      .filter((pair) => /^(SAMLRequest|RelayState|SigAlg)=/.test(pair))
      .join('&');
    const tamperedRequest = await idp.parseLoginRequest(sp, 'redirect', {
      query: tamperedQuery,
      octetString: tamperedOctet,
    });
    const tampered = await idp.createLoginResponse(
      sp,
      tamperedRequest,
      'post',
      { email: 'owner@example.com' },
      { relayState: tamperedQuery.RelayState },
    );
    const xml = Buffer.from(tampered.context, 'base64')
      .toString('utf8')
      .replace('owner@example.com', 'attacker@example.com');
    const tamperedResponse = await app.inject({
      method: 'POST',
      url: '/v1/auth/sso/saml/acs',
      headers: {
        cookie: tamperedStart.cookie,
        'content-type': 'application/x-www-form-urlencoded',
      },
      payload: new URLSearchParams({
        SAMLResponse: Buffer.from(xml).toString('base64'),
        RelayState: tamperedQuery.RelayState,
      }).toString(),
    });
    assert.equal(tamperedResponse.statusCode, 400);
    assert.deepEqual(tamperedResponse.json(), { error: 'saml_response_rejected' });
    const users = await pool.query("SELECT email FROM users ORDER BY email");
    assert.deepEqual(users.rows.map((row) => row.email), ['owner@example.com']);
  } finally {
    await app.close();
    await pool.end();
  }
});
