import Fastify from 'fastify';
import cors from '@fastify/cors';
import formbody from '@fastify/formbody';
import rateLimit from '@fastify/rate-limit';
import { publicConfig } from './config.js';
import { bearer, verifyAccessToken } from './security.js';
import { registerAuthRoutes } from './routes/auth.js';
import { registerRelayRoutes } from './routes/relay.js';
import { registerSsoRoutes } from './routes/sso.js';

export async function buildApp({ config, pool, mailer, logger = true, fetchImpl = fetch }) {
  const app = Fastify({
    logger,
    bodyLimit: 6 * 1024 * 1024,
    // Caddy is exactly one hop in front of the API. Trusting every proxy would let a caller
    // forge the leftmost X-Forwarded-For entry and rotate past the per-IP auth rate limits.
    trustProxy: config.production ? 1 : false,
    disableRequestLogging: config.production,
  });

  const allowedOrigins = new Set(config.allowedOrigins);
  await app.register(cors, {
    origin(origin, callback) {
      if (!origin || allowedOrigins.has(origin)) return callback(null, true);
      return callback(null, false);
    },
    credentials: true,
    methods: ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'],
    allowedHeaders: ['content-type', 'authorization', 'x-rigel-native', 'x-rigel-client'],
    maxAge: 86_400,
  });
  await app.register(rateLimit, {
    max: 240,
    timeWindow: '1 minute',
    allowList: [],
  });
  await app.register(formbody);

  app.decorate('config', config);
  app.decorate('db', pool);
  app.decorate('mailer', mailer);
  app.decorate('fetchImpl', fetchImpl);
  app.decorate('startedAt', Date.now());

  app.decorate('authenticate', async function authenticate(request, reply) {
    const token = bearer(request.headers.authorization);
    if (!token) return reply.code(401).send({ error: 'unauthorized' });
    let payload;
    try {
      payload = await verifyAccessToken(config.jwtSecret, token);
    } catch {
      return reply.code(401).send({ error: 'invalid_token' });
    }
    // A JWT is valid until it expires, but a session can be revoked sooner. Logout-all and a
    // password change both revoke rows, so the session is checked on every authenticated
    // request instead of trusting the token's remaining minutes.
    if (
      typeof payload.sid !== 'string' ||
      !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(payload.sid)
    ) {
      return reply.code(401).send({ error: 'invalid_token' });
    }
    const { rows } = await app.db.query(
      `SELECT u.id, u.email, u.email_verified, u.reset_required, u.role,
              u.daily_llm_micros, u.created_at, u.last_login_at
         FROM users u
         JOIN auth_sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
          AND s.revoked_at IS NULL
          AND s.expires_at > now()`,
      [payload.sub, payload.sid],
    );
    if (!rows[0]) return reply.code(401).send({ error: 'invalid_token' });
    request.user = rows[0];
    request.sessionId = payload.sid;
    return undefined;
  });

  app.addHook('onSend', async (request, reply, payload) => {
    reply.header('x-content-type-options', 'nosniff');
    reply.header('referrer-policy', 'no-referrer');
    reply.header('x-frame-options', 'DENY');
    if (request.url.startsWith('/v1/')) reply.header('cache-control', 'no-store');
    return payload;
  });

  app.setErrorHandler((error, request, reply) => {
    const status = Number(error.statusCode) || 500;
    if (status >= 500) request.log.error({ err: error }, 'request failed');
    else request.log.info({ err: error.message, url: request.url }, 'request rejected');
    if (reply.sent) return undefined;
    return reply.code(status).send({
      error: status >= 500 ? 'internal_error' : (error.code || 'bad_request'),
      ...(status >= 500 || config.production ? {} : { message: error.message }),
    });
  });

  app.get('/health', async () => {
    await app.db.query('SELECT 1');
    return {
      ok: true,
      service: 'rigel-api',
      version: 1,
      uptime_seconds: Math.floor((Date.now() - app.startedAt) / 1000),
    };
  });

  app.get('/v1/meta', async () => publicConfig(config));

  await registerAuthRoutes(app);
  await registerSsoRoutes(app);
  if (config.relayTokens?.rigel && config.relayTokens?.muse) {
    await registerRelayRoutes(app);
  }

  return app;
}
