-- SSO identities, browser-bound login state and SAML assertion replay protection.
-- Provider configuration stays in the deployment environment; only the link between an
-- external subject and a local user is stored here.

CREATE TABLE IF NOT EXISTS sso_identities (
  id UUID PRIMARY KEY,
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  provider_id TEXT NOT NULL,
  subject TEXT NOT NULL,
  email TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  last_login_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (provider_id, subject)
);
CREATE INDEX IF NOT EXISTS sso_identities_user_idx ON sso_identities (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS sso_states (
  state_hash TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  binding TEXT NOT NULL CHECK (binding IN ('oidc', 'saml')),
  request_id TEXT,
  nonce TEXT,
  code_verifier TEXT,
  return_to TEXT NOT NULL DEFAULT '/',
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS sso_states_expires_idx ON sso_states (expires_at);

CREATE TABLE IF NOT EXISTS sso_replays (
  provider_id TEXT NOT NULL,
  replay_key TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (provider_id, replay_key)
);
CREATE INDEX IF NOT EXISTS sso_replays_expires_idx ON sso_replays (expires_at);
