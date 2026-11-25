# Enterprise SSO

The self-hosted Rigel API accepts browser logins through an OIDC or SAML identity provider. The
provider configuration lives in the deployment environment; the database stores only the link
between an external subject and a local user.

**What was exercised.** `server/test/sso.test.js` runs a real HTTP OIDC provider (discovery,
authorization redirect, PKCE token exchange with a signed ID token, JWKS verification) and a
SAML identity provider that signs an assertion with a real test certificate. The same suite proves
the failure paths: a missing or replayed state, a nonce mismatch, an unverified email, a tampered
SAML assertion and a password login on an enforced domain. No live Okta, Entra ID, ADFS or other
production tenant was contacted.

## Configure

Set `SSO_PROVIDERS` to a JSON array. Each provider owns one or more email domains and chooses
`enforce: true` when password login and signup must be refused for that domain.

### OIDC provider

```json
[
  {
    "id": "acme-okta",
    "type": "oidc",
    "label": "Acme Okta",
    "issuer": "https://acme.okta.com",
    "client_id": "0oa...",
    "client_secret": "the client secret",
    "domains": ["acme.com"],
    "enforce": true
  }
]
```

Register this redirect URI at the provider:

```
https://api.example.com/v1/auth/sso/oidc/callback
```

Optional keys: `scopes` (default `openid email profile`), `email_claim` (default `email`),
`subject_claim` (default `sub`), `require_verified_email` (default `true`) and
`token_auth_method` (`client_secret_post` by default, or `client_secret_basic`). With the
default, an ID token whose `email_verified` is not `true` is refused.

### SAML provider

```json
[
  {
    "id": "acme-saml",
    "type": "saml",
    "label": "Acme ADFS",
    "entry_point": "https://adfs.acme.com/adfs/ls/",
    "issuer": "http://adfs.acme.com/adfs/services/trust",
    "cert": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----",
    "domains": ["acme.com"],
    "enforce": true
  }
]
```

The SP metadata is at `GET /v1/auth/sso/saml/metadata`. Give that URL or XML to the identity
provider administrator. The assertion consumer service is
`https://api.example.com/v1/auth/sso/saml/acs` and expects the HTTP-POST binding.

`cert` is the IdP signing certificate; a PEM block or its one-line base64 body both work. Optional
`email_attribute` (default `email`) names the assertion attribute carrying the address;
`subject_attribute` selects a stable attribute instead of the SAML NameID. The assertion must be
signed, must answer the AuthnRequest this API issued, and is accepted once; a replay is refused by
the `sso_replays` row.

SAML deployments also need the SP key pair used to build and identify this service provider:

```dotenv
SSO_SP_CERT=-----BEGIN CERTIFICATE-----
...
-----END CERTIFICATE-----
SSO_SP_KEY=-----BEGIN PRIVATE KEY-----
...
-----END PRIVATE KEY-----
SSO_SP_ENTITY_ID=https://api.example.com/v1/auth/sso/saml/metadata
SSO_SP_ACS_URL=https://api.example.com/v1/auth/sso/saml/acs
```

A self-signed certificate is enough for most IdPs; it identifies the SP and is not used to sign
the login response. The private key never leaves the API environment.

### Break-glass admin

An enforced domain refuses password login. The exception is an account that is **already** an
admin in the `users` table and is named in `SSO_BREAK_GLASS_ADMINS`:

```dotenv
SSO_BREAK_GLASS_ADMINS=admin@acme.com
```

The login response is a normal session and the `login` audit row carries
`"break_glass": true`. A listed email with the `user` role is still refused, and a new account
cannot be created through the break-glass path. Keep this account's password in the operator's
password manager and rotate it after an IdP outage.

## Flow

1. `POST /v1/auth/sso/discover` with `{"email":"person@acme.com"}` returns the provider, or
   `404 sso_not_configured`. `GET /v1/auth/sso/providers` lists the public provider metadata.
2. `GET /v1/auth/sso/{provider}/start?return_to=/welcome` writes a one-use state row and a
   `rigel_sso_state` cookie, then redirects to the provider. OIDC adds PKCE `S256` and a nonce;
   SAML creates a signed AuthnRequest with the state as `RelayState`.
3. The provider redirects to the OIDC callback or posts the SAML assertion to the ACS. The state
   cookie and the echoed state must match, the row is deleted atomically, and the provider response
   is verified.
4. The API creates a normal refresh session, sets the `rigel_refresh` HttpOnly cookie, and
   redirects to `return_to` with `sso=ok`. A client sending `x-rigel-native: 1` receives the JSON
   session instead of the redirect.

`return_to` must be a same-origin path beginning with a single `/`; an absolute URL is replaced by
`/`. Single logout, SCIM provisioning and group-to-role mapping are not implemented.

## Security boundaries

- A provider may only assert an address in the domains configured for it. An address from another
  domain is `403 email_domain_not_allowed`.
- OIDC ID tokens are checked against the configured issuer, audience, signature, nonce and
  expiry. Discovery and token endpoints must be HTTPS; plain HTTP is accepted only on loopback in
  a non-production run.
- SAML responses go through schema validation, XML-signature verification, audience and recipient
  checks, `InResponseTo`, the assertion validity window, and a database-backed replay cache.
- The state row expires after ten minutes (`SSO_STATE_MINUTES`, 1–30) and is deleted on first use.
  The state cookie is HttpOnly, SameSite=Lax, path-scoped to `/v1/auth/sso`, and Secure in
  production.
- An SSO login links to an existing account with the same verified email. This is why automatic
  linking is limited to a configured provider whose domain list contains that email; do not add a
  domain another tenant controls.
- The provider configuration is validated at startup. Duplicate provider ids, duplicate domains,
  malformed URLs and missing SAML key material fail the process before it listens.

## Verify

```bash
cd server
node --test test/sso.test.js
```

The suite covers the full OIDC exchange, the signed SAML exchange, identity linking, session
refresh, a one-use state, enforced password login, a non-admin break-glass refusal, a nonce
mismatch, an unverified email and a tampered SAML assertion. For a deployment, also check
`GET /v1/auth/sso/providers` and one real login in a private browser window; that live tenant check
is not replaced by the local-provider tests.
