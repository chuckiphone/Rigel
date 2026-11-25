# Hosted or self-hosted Rigel API

The `rigel` model door and the room relay call one Rigel API. A machine can use the hosted API,
or run the same server from this repository and point Rigel at it. The choice is one saved value,
not an environment variable the user has to remember.

Settings → **Rigel API** edits it. The choice controls:

- the base URL for the `rigel` provider's relay turn; and
- the base URL for the desktop relay bridge that posts room messages and reads Muse's inbox.

It does **not** change the model provider used by ordinary native turns. A self-hosted Rigel API
still asks OpenRouter, Anthropic, Gemini or whichever provider is configured for inference.
Firebase **Cloud** sync (shelf and session mirror) is also separate and remains the user's own
project.

## Hosted

The default. No URL is stored; requests go to `https://api.askrigel.com`. The API authenticates
the `rigel` role with the bearer credential in the OS keychain (`rigel/relay`). The account that
issues that credential controls its rotation.

## Self-hosted

The deployment in [`deploy/`](../deploy/README.md) is Caddy, the Node API and Postgres.

1. Run `node deploy/init-env.mjs` once. It writes `deploy/.env.production` with random database,
   JWT and relay secrets. Never commit or paste that file.
2. Run `./deploy/deploy.sh --host <server>` (or the smaller compose commands in the runbook) to
   build and start the stack.
3. In Settings → Rigel API choose **Self-hosted**, enter the origin Caddy serves — for example
   `https://api.example.com` — and save.
4. Paste this installation's `RIGEL_RELAY_TOKEN` as the relay credential. The separate
   `MUSE_RELAY_TOKEN` belongs in Muse's secure store, not here.

Use HTTPS for anything reachable off the machine. The relay credential is a bearer secret; over
plain HTTP anyone on the path can replay it.

## Where the choice lives

- `backend.json` in the app data folder (`%APPDATA%\Rigel` on Windows,
  `$XDG_DATA_HOME/rigel` or `~/.local/share/rigel` elsewhere, and `RIGEL_INDEX_DIR` when set).
- `{"mode":"hosted"}` or `{"mode":"self_hosted","api_url":"https://api.example.com"}`.
- The relay token is in the OS keychain, never in `backend.json`.

Two environment variables override the saved choice for a headless run:

| Variable | Meaning |
|---|---|
| `RIGEL_API_URL` | The preferred override; must be `http(s)` with no credentials, query or fragment. |
| `RIGEL_RELAY_BASE_URL` | The older name, still read so a machine configured before this setting keeps working. |

When an override is active, Settings says so and shows the URL requests actually use. The saved
choice remains on disk for the next launch without the variable.

## Enterprise SSO

The self-hosted API can accept OIDC or SAML browser logins, enforce SSO for a domain and keep a
listed break-glass admin on password login. Provider configuration, redirect/ACS URLs and the
security checks are in [sso.md](sso.md). The OIDC and signed-SAML round trips plus their failure
paths are exercised by `server/test/sso.test.js` against local providers; a real tenant login is
still required during deployment.

## Server-side security assumptions

The hosted and self-hosted server share the same code, so the same boundaries apply:

- User routes authenticate a JWT **and** check that its session is still active. Logout-all, a
  password change and a revoked device session invalidate the access token immediately instead
  of waiting for its remaining minutes.
- Account deletion requires the current password as well as the typed `DELETE` confirmation.
- Published sessions and turns are queried with `WHERE user_id = $1`; a session id from another
  account is a `404`.
- The relay is **single-tenant by construction**: two machine credentials, `rigel` and `muse`,
  and each role can read only its own inbox. Do not point two unrelated Rigel installations at
  one deployment unless they are meant to share the same relay.
- The API trusts one proxy hop in production. Run it behind Caddy or another reverse proxy that
  appends the client address; exposing the Node port directly makes the rate-limit address the
  socket peer instead of the caller.

`rigel serve` (the local coordinator API) is a different surface. It is loopback by default,
refuses browser `Origin` headers, and writes its bearer token to `api.json` with owner-only
permissions on Unix. See [api.md](api.md).
