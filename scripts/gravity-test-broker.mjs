#!/usr/bin/env node
/**
 * Local Gravity broker for end-to-end observation.
 *
 * It serves the real marketing site and mounts the real Firebase handler
 * (`functions/lib/gravity.js`) with the real `@gravity-ai/api` client, so both
 * the native window and the website exercise the same request, sanitisation and
 * no-fill path as production. The first successful test fill is cached so every
 * surface can be compared against the exact same creative during one run.
 *
 * This file is a development harness. It is not part of the Functions deploy.
 */

import { createServer } from 'node:http';
import { appendFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const { Gravity } = require('../functions/node_modules/@gravity-ai/api');
const { createGravityHandler } = require('../functions/lib/gravity');

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const WEB = path.join(ROOT, 'web');
const PORT = Number(process.env.GRAVITY_TEST_BROKER_PORT || 5000);
const LOG = path.resolve(
  ROOT,
  process.env.GRAVITY_TEST_BROKER_LOG || path.join('tmp', 'gravity-evidence', 'broker-events.jsonl'),
);
const FIREBASE = JSON.parse(readFileSync(path.join(ROOT, 'firebase.json'), 'utf8'));
const SECURITY_HEADERS = Object.fromEntries(
  (FIREBASE.hosting?.headers?.find((row) => row.source === '**')?.headers ?? [])
    .filter((row) => row.key.toLowerCase() !== 'cache-control')
    .map((row) => [row.key, row.value]),
);

mkdirSync(path.dirname(LOG), { recursive: true });
writeFileSync(LOG, '');

let fill = process.env.GRAVITY_TEST_NO_FILL !== '1';
let cached = null;

const FIXTURE = process.env.GRAVITY_TEST_FIXTURE === '1';
let noArt = process.env.GRAVITY_TEST_NO_ART === '1';

// A deterministic Gravity-shaped fill. The artwork is served by real public HTTPS hosts, so the
// broker sanitizer, the Rust model and each renderer's CSP are exercised together. Session and
// Home use different companies and different artwork fields; the website gets a third company.
const FIXTURE_CREATIVES = {
  session: {
    adText: 'Serverless Postgres that scales to zero. Start free in a minute.',
    title: 'Ship a database this afternoon',
    brandName: 'Neon',
    cta: 'Try Neon Free',
    url: 'https://neon.tech',
    favicon: 'https://github.com/neondatabase.png?size=200',
    clickUrl: 'https://trygravity.ai/track/click?p=fixture-session',
    impUrl: 'https://trygravity.ai/ack?p=fixture-session',
    placement: 'below_response',
    placement_id: 'rigel-sponsor',
  },
  home: {
    adText: 'A frontend cloud for every preview, branch and agent handoff.',
    title: 'Your agents ship faster on Vercel',
    brandName: 'Vercel',
    cta: 'Deploy now',
    url: 'https://vercel.com',
    // Deliberately no favicon: Home exercises the brandImage branch.
    brandImage: 'https://github.com/vercel.png?size=200',
    clickUrl: 'https://trygravity.ai/track/click?p=fixture-home',
    impUrl: 'https://trygravity.ai/ack?p=fixture-home',
    placement: 'below_response',
    placement_id: 'rigel-sponsor',
  },
  website: {
    adText: 'Production-ready models for coding agents and the teams that ship them.',
    title: 'Build with the latest models',
    brandName: 'OpenAI',
    cta: 'Explore OpenAI',
    url: 'https://openai.com',
    favicon: 'https://github.com/openai.png?size=200',
    clickUrl: 'https://trygravity.ai/track/click?p=fixture-website',
    impUrl: 'https://trygravity.ai/ack?p=fixture-website',
    placement: 'below_response',
    placement_id: 'rigel-sponsor',
  },
  fallback: {
    adText: 'The weekly report for teams that would rather ship than guess.',
    title: 'Measure the work that ships',
    brandName: 'Northstar Analytics',
    cta: 'See the report',
    url: 'https://example.com',
    // Both are intentionally unusable: one wrong scheme, one missing. The broker must drop them
    // and the card must still render its initials without changing layout.
    favicon: 'http://cdn.example/northstar.png',
    clickUrl: 'https://trygravity.ai/track/click?p=fixture-fallback',
    impUrl: 'https://trygravity.ai/ack?p=fixture-fallback',
    placement: 'below_response',
    placement_id: 'rigel-sponsor',
  },
};

function fixtureSource(incoming) {
  const context = incoming?.body?.gravity_context ?? {};
  if (context.device?.browser === 'web') return 'website';
  return String(context.sessionId || '').endsWith(':session') ? 'session' : 'home';
}

function fixtureCreative(source) {
  return noArt ? FIXTURE_CREATIVES.fallback : FIXTURE_CREATIVES[source] ?? FIXTURE_CREATIVES.home;
}

function publicFixtureState() {
  if (!FIXTURE) return null;
  return {
    session: publicAd(FIXTURE_CREATIVES.session),
    home: publicAd(FIXTURE_CREATIVES.home),
    website: publicAd(FIXTURE_CREATIVES.website),
    fallback: publicAd(FIXTURE_CREATIVES.fallback),
  };
}

function event(kind, fields = {}) {
  appendFileSync(LOG, `${JSON.stringify({ at: new Date().toISOString(), kind, ...fields })}\n`);
}

function publicAd(ad) {
  if (!ad) return null;
  return {
    title: ad.title || '',
    brand: ad.brandName || '',
    text: ad.adText || '',
    cta: ad.cta || '',
    click: ad.clickUrl || '',
    impression: ad.impUrl || '',
    favicon: ad.favicon || '',
    brandImage: ad.brandImage || '',
  };
}

const gravity = new Gravity({ production: false, supportsSpec: false, timeoutMs: 8_000 });
const handler = createGravityHandler({
  test: true,
  getAds: async (incoming, messages, placements) => {
    if (!fill) {
      event('no-fill-request');
      return { ads: [], status: 204, elapsed: '0ms', requestBody: null };
    }
    if (FIXTURE) {
      const source = fixtureSource(incoming);
      const creative = fixtureCreative(source);
      cached = creative;
      event('fixture-fill', { source, ...publicAd(creative) });
      return { ads: [creative], status: 200, elapsed: '0ms', requestBody: null };
    }
    if (cached) {
      event('cached-fill-served', publicAd(cached));
      return { ads: [cached], status: 200, elapsed: '0ms', requestBody: null };
    }
    const result = await gravity.getAds(incoming, messages, placements);
    if (result.ads?.length) {
      cached = result.ads[0];
      event('gravity-fill', { status: result.status, ...publicAd(cached) });
    } else {
      event('gravity-no-fill', { status: result.status });
    }
    return result;
  },
  log: (message) => event('handler-log', { message }),
});

function readBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    request.on('data', (chunk) => chunks.push(chunk));
    request.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    request.on('error', reject);
  });
}

function responseAdapter() {
  const state = { status: 200, headers: {}, body: null, ended: false };
  return {
    state,
    status(code) {
      state.status = code;
      return this;
    },
    set(headers) {
      Object.assign(state.headers, headers);
      return this;
    },
    json(body) {
      state.body = body;
      return this;
    },
    end() {
      state.ended = true;
      return this;
    },
  };
}

async function gravityRequest(request, response) {
  const raw = await readBody(request);
  let body = raw;
  if (raw && (raw.startsWith('{') || raw.startsWith('['))) {
    try {
      body = JSON.parse(raw);
    } catch {
      // The handler will answer invalid_json.
    }
  }
  const source = body && typeof body === 'object' && body.gravity_context ? 'website' : 'app';
  const headers = request.headers;
  // Loopback is not a public client address, and Gravity's test inventory no-fills it. Firebase
  // supplies the real address in production; the local harness supplies a documented TEST-NET
  // address so the same broker path can exercise a real test fill.
  headers['x-forwarded-for'] = process.env.GRAVITY_TEST_CLIENT_IP || '198.51.100.9';
  const outgoing = {
    method: request.method,
    headers,
    body,
    ip: request.socket.remoteAddress || '',
    get(name) {
      const value = headers[String(name).toLowerCase()];
      return Array.isArray(value) ? value[0] : value;
    },
  };
  const adapted = responseAdapter();
  await handler(outgoing, adapted);
  event('broker-response', {
    source,
    status: adapted.state.status,
    ads: Array.isArray(adapted.state.body?.ads) ? adapted.state.body.ads.length : null,
    test: adapted.state.body?.test ?? null,
  });
  response.writeHead(adapted.state.status, adapted.state.headers);
  response.end(adapted.state.ended ? undefined : JSON.stringify(adapted.state.body));
}

function contentType(file) {
  const ext = path.extname(file).toLowerCase();
  return (
    {
      '.html': 'text/html; charset=utf-8',
      '.css': 'text/css; charset=utf-8',
      '.js': 'text/javascript; charset=utf-8',
      '.mjs': 'text/javascript; charset=utf-8',
      '.svg': 'image/svg+xml',
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.webp': 'image/webp',
      '.woff2': 'font/woff2',
      '.json': 'application/json; charset=utf-8',
      '.txt': 'text/plain; charset=utf-8',
      '.ico': 'image/x-icon',
    }[ext] || 'application/octet-stream'
  );
}

function serveStatic(request, response, pathname) {
  let relative = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
  if (relative === 'app' || relative === 'app/') relative = 'app/index.html';
  const file = path.resolve(WEB, relative);
  if (!file.startsWith(`${WEB}${path.sep}`) || !existsSync(file)) {
    response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
    response.end('not found');
    return;
  }
  const resolved = statSync(file).isDirectory() ? path.join(file, 'index.html') : file;
  if (!existsSync(resolved)) {
    response.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
    response.end('not found');
    return;
  }
  response.writeHead(200, {
    ...SECURITY_HEADERS,
    'cache-control': 'no-store',
    'content-type': contentType(resolved),
  });
  response.end(readFileSync(resolved));
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url || '/', `http://${request.headers.host || `127.0.0.1:${PORT}`}`);
  // Caddy's production bridge rewrites `/api/gravity-ad` to `/gravityAd` before proxying to
  // Firebase. Accept that same upstream path when Caddy is the local front door.
  if (url.pathname === '/api/gravity-ad' || url.pathname === '/gravityAd') {
    try {
      await gravityRequest(request, response);
    } catch (error) {
      event('broker-error', { message: String(error).slice(0, 240) });
      response.writeHead(500, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: 'test_broker_failed' }));
    }
    return;
  }
  if (url.pathname === '/api/test-state') {
    response.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    response.end(
      JSON.stringify({
        fill,
        cached: publicAd(cached),
        fixture: publicFixtureState(),
        noArt,
      }),
    );
    return;
  }
  if (url.pathname === '/api/test-mode' && request.method === 'POST') {
    const raw = await readBody(request);
    try {
      const next = JSON.parse(raw);
      if (typeof next.fill === 'boolean') {
        fill = next.fill;
      }
      if (typeof next.noArt === 'boolean') noArt = next.noArt;
      cached = null;
      event('mode', { fill, noArt });
    } catch {
      // Keep the current mode and report it below.
    }
    response.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    response.end(JSON.stringify({ fill, noArt }));
    return;
  }
  serveStatic(request, response, url.pathname);
});

server.listen(PORT, '127.0.0.1', () => {
  event('listening', { port: PORT, fill });
  console.log(`gravity test broker: http://127.0.0.1:${PORT} (fill=${fill})`);
  console.log(`events: ${LOG}`);
});
