const PROVIDER_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;
const DOMAIN = /^(?=.{1,253}$)(?!-)[a-z0-9-]{1,63}(?<!-)(?:\.(?!-)[a-z0-9-]{1,63}(?<!-))+$/;
const LOOPBACK = new Set(['localhost', '127.0.0.1', '::1', '[::1]']);

function fail(message) {
  throw new Error(`SSO configuration: ${message}`);
}

function isLoopbackHost(hostname) {
  return LOOPBACK.has(hostname.toLowerCase());
}

/** https everywhere, except a loopback development provider when this is not production. */
export function isSecureEndpoint(value, production, label) {
  let url;
  try {
    url = new URL(String(value ?? ''));
  } catch {
    fail(`${label} must be an absolute URL`);
  }
  if (url.username || url.password) fail(`${label} must not carry credentials`);
  if (url.protocol === 'https:') return url;
  if (!production && url.protocol === 'http:' && isLoopbackHost(url.hostname)) return url;
  fail(`${label} must use https${production ? '' : ' (http is allowed only on loopback)'}`);
}

function text(value) {
  return String(value ?? '').trim();
}

function list(value) {
  return Array.isArray(value)
    ? value.map((item) => text(item).toLowerCase()).filter(Boolean)
    : [];
}

function normalizePem(value, kind) {
  const raw = text(value);
  if (!raw) return '';
  if (raw.includes('-----BEGIN')) return raw.endsWith('\n') ? raw : `${raw}\n`;
  const body = raw.replace(/\s+/g, '').match(/.{1,64}/g)?.join('\n') ?? '';
  return `-----BEGIN ${kind}-----\n${body}\n-----END ${kind}-----\n`;
}

function provider(raw, production, apiUrl, index) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) fail(`provider ${index + 1} must be an object`);
  const id = text(raw.id);
  if (!PROVIDER_ID.test(id)) fail(`provider ${index + 1} has an invalid id`);
  const type = text(raw.type).toLowerCase();
  if (type !== 'oidc' && type !== 'saml') fail(`${id} has an unsupported type (expected oidc or saml)`);
  const domains = list(raw.domains);
  if (domains.length === 0) fail(`${id} names no email domain`);
  for (const domain of domains) {
    if (!DOMAIN.test(domain)) fail(`${id} has an invalid domain: ${domain}`);
  }
  const common = {
    id,
    type,
    domains,
    enforce: raw.enforce === true,
    label: text(raw.label) || id,
  };

  if (type === 'oidc') {
    const issuerUrl = isSecureEndpoint(raw.issuer, production, `${id}.issuer`);
    const clientId = text(raw.client_id);
    const clientSecret = text(raw.client_secret);
    if (!clientId) fail(`${id} needs client_id`);
    if (!clientSecret) fail(`${id} needs client_secret`);
    const tokenAuthMethod = text(raw.token_auth_method) || 'client_secret_post';
    if (!['client_secret_post', 'client_secret_basic'].includes(tokenAuthMethod)) {
      fail(`${id}.token_auth_method must be client_secret_post or client_secret_basic`);
    }
    const scopes = Array.isArray(raw.scopes)
      ? raw.scopes.map(text).filter(Boolean)
      : text(raw.scopes || 'openid email profile').split(/\s+/).filter(Boolean);
    if (!scopes.includes('openid')) scopes.unshift('openid');
    const redirectUri = `${apiUrl}/v1/auth/sso/oidc/callback`;
    isSecureEndpoint(redirectUri, production, `${id}.redirect_uri`);
    return {
      ...common,
      issuer: issuerUrl.toString().replace(/\/+$/, ''),
      clientId,
      clientSecret,
      tokenAuthMethod,
      scopes,
      emailClaim: text(raw.email_claim) || 'email',
      subjectClaim: text(raw.subject_claim) || 'sub',
      requireVerifiedEmail: raw.require_verified_email !== false,
      redirectUri,
    };
  }

  const entryPoint = isSecureEndpoint(raw.entry_point, production, `${id}.entry_point`).toString();
  const issuer = text(raw.issuer);
  if (!issuer) fail(`${id} needs issuer`);
  const cert = normalizePem(raw.cert, 'CERTIFICATE');
  if (!cert) fail(`${id} needs the identity provider signing certificate`);
  return {
    ...common,
    entryPoint,
    issuer,
    cert,
    emailAttribute: text(raw.email_attribute) || 'email',
    subjectAttribute: text(raw.subject_attribute) || '',
    entityId: text(raw.entity_id) || issuer,
  };
}

/**
 * Turn `SSO_PROVIDERS` into validated providers. Secrets stay in this object; callers must use
 * `providerPublic` before sending anything to a browser.
 */
export function loadSso(env = {}, { production = false, apiUrl = '' } = {}) {
  const source = text(env.SSO_PROVIDERS);
  if (!source) {
    return {
      enabled: false,
      providers: [],
      breakGlassAdmins: [],
      stateMinutes: 10,
      spEntityId: '',
      spAcsUrl: '',
      spCert: '',
      spKey: '',
    };
  }

  let parsed;
  try {
    parsed = JSON.parse(source);
  } catch {
    fail('SSO_PROVIDERS must be a JSON array');
  }
  if (!Array.isArray(parsed)) fail('SSO_PROVIDERS must be a JSON array');
  if (parsed.length === 0) fail('SSO_PROVIDERS is empty; omit it to disable SSO');

  const providers = parsed.map((row, index) => provider(row, production, apiUrl, index));
  const ids = new Set();
  const domains = new Set();
  for (const row of providers) {
    if (ids.has(row.id)) fail(`provider id ${row.id} repeats`);
    ids.add(row.id);
    for (const domain of row.domains) {
      if (domains.has(domain)) fail(`email domain ${domain} is claimed by more than one provider`);
      domains.add(domain);
    }
  }

  const saml = providers.filter((row) => row.type === 'saml');
  const spEntityId = text(env.SSO_SP_ENTITY_ID) || `${apiUrl}/v1/auth/sso/saml/metadata`;
  const spAcsUrl = text(env.SSO_SP_ACS_URL) || `${apiUrl}/v1/auth/sso/saml/acs`;
  const spCert = normalizePem(env.SSO_SP_CERT, 'CERTIFICATE');
  const spKey = normalizePem(env.SSO_SP_KEY, 'PRIVATE KEY');
  if (saml.length > 0) {
    if (!spCert || !spKey) fail('SAML providers need SSO_SP_CERT and SSO_SP_KEY');
    if (!spEntityId) fail('SSO_SP_ENTITY_ID must not be empty');
    isSecureEndpoint(spAcsUrl, production, 'SSO_SP_ACS_URL');
  }

  const stateMinutes = Math.min(30, Math.max(1, Number.parseInt(env.SSO_STATE_MINUTES ?? '10', 10) || 10));
  return {
    enabled: true,
    providers,
    breakGlassAdmins: String(env.SSO_BREAK_GLASS_ADMINS ?? '')
      .split(',')
      .map((email) => email.trim().toLowerCase())
      .filter(Boolean),
    stateMinutes,
    spEntityId,
    spAcsUrl,
    spCert,
    spKey,
  };
}

export function findProvider(sso, id) {
  return sso?.providers?.find((provider) => provider.id === id);
}

export function providerForEmail(sso, email) {
  const domain = text(email).toLowerCase().split('@')[1];
  if (!domain) return null;
  return sso?.providers?.find((provider) => provider.domains.includes(domain)) ?? null;
}

/** A break-glass admin must be both listed in configuration and already an admin account. */
export function isBreakGlassAdmin(sso, user) {
  if (!user || user.role !== 'admin') return false;
  return sso?.breakGlassAdmins?.includes(text(user.email).toLowerCase()) ?? false;
}

export function providerPublic(provider) {
  return {
    id: provider.id,
    type: provider.type,
    label: provider.label,
    domains: [...provider.domains],
    enforced: provider.enforce,
  };
}

/** Only a same-origin path can be a post-login destination. */
export function safeReturnTo(value) {
  const candidate = String(value ?? '').trim();
  if (!candidate.startsWith('/') || candidate.startsWith('//')) return '/';
  if (candidate.includes('\\') || candidate.includes('\0')) return '/';
  return candidate;
}

/** Add the completion marker without losing a query string or fragment. */
export function withSsoMarker(base, returnTo) {
  const url = new URL(`${String(base).replace(/\/+$/, '')}${returnTo}`);
  url.searchParams.set('sso', 'ok');
  return url.toString();
}
