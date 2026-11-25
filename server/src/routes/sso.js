import { createHash, randomUUID } from 'node:crypto';
import { createRemoteJWKSet, customFetch, jwtVerify } from 'jose';
import * as samlify from 'samlify';
import * as samlXsd from '@authenio/samlify-node-xmllint';

import {
  audit,
  createSession,
  parseCookies,
  publicUser,
  sessionResponse,
  setRefreshCookie,
} from './auth.js';
import { randomToken, safeEqual, sha256 } from '../security.js';
import {
  findProvider,
  providerForEmail,
  providerPublic,
  safeReturnTo,
  withSsoMarker,
} from '../sso.js';

samlify.setSchemaValidator(samlXsd);

const STATE_COOKIE = 'rigel_sso_state';
const OIDC_ALGORITHMS = ['RS256', 'RS384', 'RS512', 'PS256', 'PS384', 'PS512', 'ES256', 'ES384', 'ES512'];
const SAML_ALGORITHMS = [
  'http://www.w3.org/2001/04/xmldsig-more#rsa-sha256',
  'http://www.w3.org/2001/04/xmldsig-more#rsa-sha384',
  'http://www.w3.org/2001/04/xmldsig-more#rsa-sha512',
  'http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256',
];

const caches = new WeakMap();

function cacheFor(app) {
  let cache = caches.get(app);
  if (!cache) {
    cache = {
      sp: new Map(),
      idp: new Map(),
      discovery: new Map(),
      jwks: new Map(),
    };
    caches.set(app, cache);
  }
  return cache;
}

function stateCookie(config, value, maxAge) {
  const secure = config.production ? '; Secure' : '';
  return `${STATE_COOKIE}=${encodeURIComponent(value)}; HttpOnly; Path=/v1/auth/sso; SameSite=Lax; Max-Age=${maxAge}${secure}`;
}

function setStateCookie(reply, config, value) {
  reply.header('set-cookie', stateCookie(config, value, Math.max(60, config.sso.stateMinutes * 60)));
}

function appendCookie(reply, value) {
  const current = reply.getHeader('set-cookie');
  const values = current === undefined ? [] : Array.isArray(current) ? current : [current];
  reply.header('set-cookie', [...values, value]);
}

function native(request) {
  return request.headers['x-rigel-native'] === '1';
}

function emailValue(value) {
  if (Array.isArray(value)) return emailValue(value[0]);
  return typeof value === 'string' ? value.trim() : '';
}

function claim(payload, name) {
  return emailValue(payload?.[name]);
}

function samlAttribute(attributes, name) {
  if (!attributes || !name) return '';
  const direct = attributes[name] ?? attributes[name.toLowerCase()];
  if (direct !== undefined) return emailValue(direct);
  const key = Object.keys(attributes).find((candidate) => candidate.toLowerCase() === name.toLowerCase());
  return key ? emailValue(attributes[key]) : '';
}

function sameDomain(email, provider) {
  const domain = String(email).toLowerCase().split('@')[1] ?? '';
  return provider.domains.includes(domain);
}

function failure(reply, status, error) {
  return reply.code(status).send({ error });
}

function spFor(app, provider) {
  const cache = cacheFor(app);
  if (cache.sp.has(provider.id)) return cache.sp.get(provider.id);
  const sp = samlify.ServiceProvider({
    entityID: app.config.sso.spEntityId,
    assertionConsumerService: [
      { Binding: samlify.Constants.namespace.binding.post, Location: app.config.sso.spAcsUrl },
    ],
    privateKey: app.config.sso.spKey,
    signingCert: app.config.sso.spCert,
    authnRequestsSigned: true,
    wantAssertionsSigned: true,
    wantMessageSigned: false,
    allowUnsolicitedResponse: false,
    allowedSignatureAlgorithms: SAML_ALGORITHMS,
    replayCache: {
      async saveIfAbsent(key, expiresAt) {
        await app.db.query('DELETE FROM sso_replays WHERE expires_at <= now()').catch(() => undefined);
        const { rowCount } = await app.db.query(
          `INSERT INTO sso_replays (provider_id, replay_key, expires_at)
           VALUES ($1, $2, $3)
           ON CONFLICT (provider_id, replay_key) DO NOTHING`,
          [provider.id, key, expiresAt],
        );
        return rowCount === 1;
      },
    },
  });
  cache.sp.set(provider.id, sp);
  return sp;
}

function idpFor(app, provider) {
  const cache = cacheFor(app);
  if (cache.idp.has(provider.id)) return cache.idp.get(provider.id);
  const idp = samlify.IdentityProvider({
    entityID: provider.entityId,
    signingCert: provider.cert,
    wantAuthnRequestsSigned: true,
    singleSignOnService: [
      { Binding: samlify.Constants.namespace.binding.redirect, Location: provider.entryPoint },
    ],
    allowedSignatureAlgorithms: SAML_ALGORITHMS,
  });
  cache.idp.set(provider.id, idp);
  return idp;
}

function endpoint(value, app, label) {
  let url;
  try {
    url = new URL(String(value ?? ''));
  } catch {
    throw new Error(`${label} is not an absolute URL`);
  }
  const loopback = ['localhost', '127.0.0.1', '::1', '[::1]'].includes(url.hostname.toLowerCase());
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback && !app.config.production)) {
    throw new Error(`${label} is not a secure endpoint`);
  }
  if (url.username || url.password) throw new Error(`${label} carries credentials`);
  return url;
}

async function discovery(app, provider) {
  const cache = cacheFor(app);
  const cached = cache.discovery.get(provider.id);
  if (cached && cached.expires > Date.now()) return cached.value;
  let response;
  try {
    response = await app.fetchImpl(`${provider.issuer}/.well-known/openid-configuration`, {
      headers: { accept: 'application/json' },
      redirect: 'error',
      signal: AbortSignal.timeout(10_000),
    });
  } catch (error) {
    throw new Error(`OIDC discovery failed: ${error.message}`);
  }
  if (!response.ok) throw new Error(`OIDC discovery returned ${response.status}`);
  const document = await response.json();
  if (document.issuer !== provider.issuer) throw new Error('OIDC discovery issuer mismatch');
  const value = {
    authorization_endpoint: endpoint(document.authorization_endpoint, app, 'authorization_endpoint').toString(),
    token_endpoint: endpoint(document.token_endpoint, app, 'token_endpoint').toString(),
    jwks_uri: endpoint(document.jwks_uri, app, 'jwks_uri').toString(),
  };
  cache.discovery.set(provider.id, { value, expires: Date.now() + 10 * 60_000 });
  return value;
}

function jwks(app, provider, document) {
  const cache = cacheFor(app);
  if (cache.jwks.has(provider.id)) return cache.jwks.get(provider.id);
  const remote = createRemoteJWKSet(new URL(document.jwks_uri), {
    [customFetch]: app.fetchImpl,
    timeoutDuration: 10_000,
    cooldownDuration: 30_000,
    cacheMaxAge: 10 * 60_000,
  });
  cache.jwks.set(provider.id, remote);
  return remote;
}

async function consumeState(app, request) {
  const cookies = parseCookies(request.headers.cookie);
  const supplied = String(cookies[STATE_COOKIE] ?? '');
  if (!supplied) return null;
  const echoed = String(request.query?.state ?? request.body?.RelayState ?? '');
  if (!echoed || !safeEqual(echoed, supplied)) return null;
  const { rows } = await app.db.query(
    `DELETE FROM sso_states
      WHERE state_hash = $1 AND expires_at > now()
      RETURNING provider_id, binding, request_id, nonce, code_verifier, return_to`,
    [sha256(supplied)],
  );
  return rows[0] ?? null;
}

async function saveState(app, config, row) {
  const expires = new Date(Date.now() + config.sso.stateMinutes * 60_000);
  await app.db.query('DELETE FROM sso_states WHERE expires_at <= now()').catch(() => undefined);
  await app.db.query(
    `INSERT INTO sso_states
       (state_hash, provider_id, binding, request_id, nonce, code_verifier, return_to, expires_at)
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
    [sha256(row.state), row.providerId, row.binding, row.requestId ?? null, row.nonce ?? null, row.verifier ?? null, row.returnTo, expires],
  );
}

async function linkUser(app, { provider, subject, email }) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const client = await app.db.connect();
    try {
      await client.query('BEGIN');
      const identity = await client.query(
        `SELECT user_id FROM sso_identities WHERE provider_id = $1 AND subject = $2`,
        [provider.id, subject],
      );
      let user;
      if (identity.rows[0]) {
        const found = await client.query(
          `SELECT id, email, email_verified, reset_required, role, daily_llm_micros, created_at
             FROM users WHERE id = $1`,
          [identity.rows[0].user_id],
        );
        user = found.rows[0];
      } else {
        const existing = await client.query(
          `SELECT id, email, email_verified, reset_required, role, daily_llm_micros, created_at
             FROM users WHERE lower(email) = lower($1) FOR UPDATE`,
          [email],
        );
        user = existing.rows[0];
        if (!user) {
          const created = await client.query(
            `INSERT INTO users (email, password_hash, email_verified, daily_llm_micros)
             VALUES ($1, NULL, TRUE, $2)
             RETURNING id, email, email_verified, reset_required, role, daily_llm_micros, created_at`,
            [email, app.config.llmDefaultDailyMicros],
          );
          user = created.rows[0];
        }
        await client.query(
          `INSERT INTO sso_identities (id, user_id, provider_id, subject, email)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (provider_id, subject) DO NOTHING`,
          [randomUUID(), user.id, provider.id, subject, email],
        );
      }
      if (!user) throw new Error('SSO identity points at a missing user');
      await client.query('UPDATE users SET last_login_at = now(), updated_at = now() WHERE id = $1', [user.id]);
      await client.query(
        'UPDATE sso_identities SET last_login_at = now() WHERE provider_id = $1 AND subject = $2',
        [provider.id, subject],
      );
      await client.query('COMMIT');
      return user;
    } catch (error) {
      await client.query('ROLLBACK').catch(() => undefined);
      if (error.code !== '23505' || attempt === 1) throw error;
    } finally {
      client.release();
    }
  }
  throw new Error('SSO identity could not be linked');
}

async function completeLogin(app, request, reply, { provider, user, returnTo, kind }) {
  const { config } = app;
  const session = await createSession(app, request, user);
  await audit(app, user.id, 'sso_login', {
    provider_id: provider.id,
    provider_type: provider.type,
    kind,
  });
  if (native(request)) {
    return reply.send({ ...sessionResponse(user, session, true), user: publicUser(user) });
  }
  setRefreshCookie(reply, config, session.refreshToken);
  appendCookie(reply, stateCookie(config, '', 0));
  return reply.redirect(withSsoMarker(config.appUrl, returnTo));
}

export async function registerSsoRoutes(app) {
  const { config } = app;
  const sso = config.sso;

  app.get('/v1/auth/sso/providers', async () => ({
    enabled: Boolean(sso?.enabled),
    providers: sso?.enabled ? sso.providers.map(providerPublic) : [],
  }));

  app.post(
    '/v1/auth/sso/discover',
    { config: { rateLimit: { max: 20, timeWindow: '1 minute' } } },
    async (request, reply) => {
      if (!sso?.enabled) return failure(reply, 404, 'sso_not_configured');
      const email = String(request.body?.email ?? '').trim().toLowerCase();
      const provider = providerForEmail(sso, email);
      if (!provider) return failure(reply, 404, 'sso_not_configured');
      return { provider: providerPublic(provider) };
    },
  );

  app.get('/v1/auth/sso/saml/metadata', async (request, reply) => {
    if (!sso?.enabled) return failure(reply, 404, 'sso_not_configured');
    const saml = sso.providers.find((provider) => provider.type === 'saml');
    if (!saml) return failure(reply, 404, 'saml_not_configured');
    return reply.type('application/xml').send(spFor(app, saml).getMetadata());
  });

  app.get(
    '/v1/auth/sso/:provider/start',
    { config: { rateLimit: { max: 30, timeWindow: '1 minute' } } },
    async (request, reply) => {
      if (!sso?.enabled) return failure(reply, 404, 'sso_not_configured');
      const provider = findProvider(sso, String(request.params.provider ?? ''));
      if (!provider) return failure(reply, 404, 'sso_provider_not_found');
      const returnTo = safeReturnTo(request.query?.return_to);
      const state = randomToken(32);

      try {
        if (provider.type === 'oidc') {
          const document = await discovery(app, provider);
          const verifier = randomToken(48);
          const challenge = createHash('sha256').update(verifier).digest('base64url');
          const nonce = randomToken(24);
          await saveState(app, config, {
            state,
            providerId: provider.id,
            binding: 'oidc',
            nonce,
            verifier,
            returnTo,
          });
          const url = new URL(document.authorization_endpoint);
          url.searchParams.set('response_type', 'code');
          url.searchParams.set('client_id', provider.clientId);
          url.searchParams.set('redirect_uri', provider.redirectUri);
          url.searchParams.set('scope', provider.scopes.join(' '));
          url.searchParams.set('state', state);
          url.searchParams.set('nonce', nonce);
          url.searchParams.set('code_challenge', challenge);
          url.searchParams.set('code_challenge_method', 'S256');
          setStateCookie(reply, config, state);
          return reply.redirect(url.toString());
        }

        const login = spFor(app, provider).createLoginRequest(idpFor(app, provider), 'redirect', {
          relayState: state,
        });
        await saveState(app, config, {
          state,
          providerId: provider.id,
          binding: 'saml',
          requestId: login.id,
          returnTo,
        });
        setStateCookie(reply, config, state);
        return reply.redirect(login.context);
      } catch (error) {
        request.log.error({ err: error, provider: provider.id }, 'SSO start failed');
        return failure(reply, 502, 'sso_provider_unavailable');
      }
    },
  );

  app.get(
    '/v1/auth/sso/oidc/callback',
    { config: { rateLimit: { max: 60, timeWindow: '1 minute' } } },
    async (request, reply) => {
      if (!sso?.enabled) return failure(reply, 404, 'sso_not_configured');
      const row = await consumeState(app, request);
      if (!row) return failure(reply, 400, 'invalid_sso_state');
      if (row.binding !== 'oidc') return failure(reply, 400, 'invalid_sso_state');
      const provider = findProvider(sso, row.provider_id);
      if (!provider || provider.type !== 'oidc') return failure(reply, 404, 'sso_provider_not_found');
      if (request.query?.error) {
        request.log.warn({ provider: provider.id, error: request.query.error }, 'OIDC provider refused login');
        return failure(reply, 400, 'sso_provider_error');
      }
      const code = String(request.query?.code ?? '');
      if (!code) return failure(reply, 400, 'invalid_sso_response');

      try {
        const document = await discovery(app, provider);
        const body = new URLSearchParams({
          grant_type: 'authorization_code',
          code,
          redirect_uri: provider.redirectUri,
          client_id: provider.clientId,
          code_verifier: row.code_verifier,
        });
        const headers = { 'content-type': 'application/x-www-form-urlencoded', accept: 'application/json' };
        if (provider.tokenAuthMethod === 'client_secret_basic') {
          headers.authorization = `Basic ${Buffer.from(
            `${encodeURIComponent(provider.clientId)}:${encodeURIComponent(provider.clientSecret)}`,
          ).toString('base64')}`;
        } else {
          body.set('client_secret', provider.clientSecret);
        }
        const tokenResponse = await app.fetchImpl(document.token_endpoint, {
          method: 'POST',
          headers,
          body,
          redirect: 'error',
          signal: AbortSignal.timeout(10_000),
        });
        if (!tokenResponse.ok) throw new Error(`token endpoint returned ${tokenResponse.status}`);
        const tokens = await tokenResponse.json();
        if (typeof tokens.id_token !== 'string') throw new Error('token response has no id_token');

        const { payload } = await jwtVerify(tokens.id_token, jwks(app, provider, document), {
          issuer: provider.issuer,
          audience: provider.clientId,
          algorithms: OIDC_ALGORITHMS,
          clockTolerance: 30,
        });
        if (String(payload.nonce ?? '') !== String(row.nonce ?? '')) {
          throw new Error('OIDC nonce mismatch');
        }
        const email = claim(payload, provider.emailClaim).toLowerCase();
        const subject = claim(payload, provider.subjectClaim) || claim(payload, 'sub');
        if (!email || !subject) throw new Error('OIDC response is missing email or subject');
        if (provider.requireVerifiedEmail && payload.email_verified !== true) {
          return failure(reply, 403, 'email_not_verified');
        }
        if (!sameDomain(email, provider)) return failure(reply, 403, 'email_domain_not_allowed');

        const user = await linkUser(app, { provider, subject, email });
        return completeLogin(app, request, reply, {
          provider,
          user,
          returnTo: safeReturnTo(row.return_to),
          kind: 'oidc',
        });
      } catch (error) {
        request.log.error({ err: error, provider: provider.id }, 'OIDC callback failed');
        return failure(reply, 400, 'sso_authentication_failed');
      }
    },
  );

  app.post(
    '/v1/auth/sso/saml/acs',
    { config: { rateLimit: { max: 60, timeWindow: '1 minute' } } },
    async (request, reply) => {
      if (!sso?.enabled) return failure(reply, 404, 'sso_not_configured');
      const row = await consumeState(app, request);
      if (!row) return failure(reply, 400, 'invalid_sso_state');
      if (row.binding !== 'saml') return failure(reply, 400, 'invalid_sso_state');
      const provider = findProvider(sso, row.provider_id);
      if (!provider || provider.type !== 'saml') return failure(reply, 404, 'sso_provider_not_found');
      const response = String(request.body?.SAMLResponse ?? '');
      if (!response) return failure(reply, 400, 'invalid_sso_response');

      try {
        const parsed = await spFor(app, provider).parseLoginResponse(
          idpFor(app, provider),
          'post',
          { body: { SAMLResponse: response, RelayState: request.body?.RelayState } },
          { inResponseTo: row.request_id ?? null },
        );
        const attributes = parsed.extract.attributes ?? {};
        const email = (
          samlAttribute(attributes, provider.emailAttribute) ||
          String(parsed.extract.nameID ?? '').trim()
        ).toLowerCase();
        const subject =
          samlAttribute(attributes, provider.subjectAttribute) ||
          String(parsed.extract.nameID ?? '').trim();
        if (!email || !subject) throw new Error('SAML assertion is missing email or subject');
        if (!sameDomain(email, provider)) return failure(reply, 403, 'email_domain_not_allowed');
        const user = await linkUser(app, { provider, subject, email });
        return completeLogin(app, request, reply, {
          provider,
          user,
          returnTo: safeReturnTo(row.return_to),
          kind: 'saml',
        });
      } catch (error) {
        request.log.error({ err: error, provider: provider.id }, 'SAML assertion rejected');
        return failure(reply, 400, 'saml_response_rejected');
      }
    },
  );
}
