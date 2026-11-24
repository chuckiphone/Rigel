# Security audit — 2026-11-24

This pass applied the supplied 67-check desktop/API list and the Cloudflare
`security-audit-skill` guidance to Rigel's real surfaces: the loopback control API, the relay
bridge, the update path, the room/agent boundary and the desktop webview. It used source review
and the existing bounded unit/integration tests; it did not probe a deployed Tailscale endpoint,
the vendor room server or a production relay.

The audit is intentionally **not** a claim of complete coverage. The last section names the checks
that need deployment or vendor evidence.

## Fixed in this pass

| Check | Finding | Fix | Regression |
|---|---|---|---|
| 25 — DNS rebinding | The local API accepted any `Host` header. A page on an attacker-controlled name can resolve that name to `127.0.0.1` and reach a loopback listener while the browser still sends the attacker's name. | `Host` must name the bound listener, `localhost`/loopback, or the `--public-url` host. Anything else is `403 forbidden_host`. | `serve::tests::host_allowlist_rejects_rebinding_names_and_accepts_the_configured_listener` |
| 14 — timing attack | The bearer comparison returned early on a length mismatch. | Both sides are SHA-256 hashed first, then compared over fixed-length digests with a constant-time loop. | `serve::tests::bearer_tokens_are_compared_through_fixed_length_digests` |
| 24 — auth brute force | Auth failures had no cost. | Ten failed tokens from one TCP peer in a minute return `429 too_many_attempts`; a success clears the peer's window. The table is bounded and the token check still runs for peers beyond the cap. | `serve::tests::failed_bearer_attempts_are_limited_per_peer_and_clear_on_success` |
| 13 / 23 — predictable ids | Swarm jobs used `SystemTime::now()` nanoseconds plus the PID, which a peer can estimate. | `new_job_id()` is `swarm-` plus a v4 UUID (122 random bits); both the core queue and the desktop command use it. | `swarm::tests::job_ids_are_unpredictable_uuids` |
| 8 / 14 — JWT alg and relay timing | The cloud API verified tokens without pinning an algorithm, and the relay's token comparison returned early on a length mismatch. | `jwtVerify` accepts only HS256; `safeEqual` hashes both sides to fixed-length digests before `timingSafeEqual`. | `auth-security.test.js::an HS384 token is refused…` and `…relay tokens compare through fixed-length digests…` |
| 33 — nested relay JSON | A 256 KiB relay payload could nest deeply enough to exhaust the recursive canonicaliser. | Payloads are capped at 32 levels and 10,000 nodes as well as 256 KiB; the rejection is `413 invalid_payload`. | `relay.test.js::a deeply nested payload is rejected before canonicalisation` |
| 45 — whitespace secret | The cloud API's 10-character minimum admitted a password made only of spaces. | `validPassword` requires at least one non-whitespace character after the length check. | `auth-security.test.js::a whitespace-only password is never a valid credential` |
| 52 — mail transfer hang | The API's Resend call had no timeout, so a stalled provider could hold a reset/signup request open. | The mail fetch carries a 10-second `AbortSignal.timeout`. | Source review; the provider is not called by the test suite |
| 30 — committed secrets | No committed secret was found, but the scan and review found live-shaped values in the gitignored `deploy/.env.production` and local `cms/.env`; the production file is not in git history. | The files stay ignored. The new CI step below adds a verified TruffleHog scan beside gitleaks. The values still need rotation if this checkout or its backups were shared. | `.github/workflows/ci.yml` supply-chain lane |

`SECURITY.md` and [api.md](api.md) now describe the Host allowlist, the digest comparison and the
per-peer failure window.

## Already enforced, with evidence

- **Origin and CORS (19, 31).** A request with any `Origin` is refused before auth; the server
  never emits `Access-Control-Allow-Origin`, and responses carry `cache-control: no-store`.
  `serve.rs::handle`.
- **Bearer scope (56).** Auth runs before every route, including `/v1/health`,
  `/v1/openapi.json`, lists and searches. There is no unauthenticated enumeration endpoint.
- **Cloud auth (2, 8, 13, 14, 21, 24, 45, 55, 59, 60).** The Fastify API normalises the email,
  stores scrypt (N=32768) hashes, pins HS256 and issuer/audience on every access token, checks a
  live, unrevoked session row on every authenticated request, rotates a hashed 384-bit refresh
  token, and rejects whitespace-only passwords. Login runs a dummy scrypt verify for an unknown
  email and returns the same `invalid_credentials`; reset always returns `{ok:true}` and mails
  only the registered address. Password change revokes every other session; reset revokes all.
  Login is capped at 10/minute per IP and the global limiter at 240/minute. `server/test/*.js`
  covers the session, deletion, origin, JWT, digest and whitespace cases.
- **Cloud request boundary (9, 11, 24, 31, 32, 66).** CORS allows only the exact configured
  origins, `trustProxy` is one Caddy hop, request bodies are capped at 6 MiB, responses carry
  `nosniff`/`DENY`/`no-referrer`, `/v1/` is `no-store`, and the error handler returns
  `internal_error` without a message in production while logging the detail server-side.
- **Token entropy (13).** `new_token()` concatenates two v4 UUIDs (244 random bits, 64 hex
  characters). `existing_token()` refuses a stored value shorter than 32 characters.
- **Token file (21, 34).** `api.json` is written owner-only on Unix (`0600`); the token is never
  logged by the API. `--rotate` replaces the stored token before serving. A running process keeps
  its in-memory token, so revoke by stopping it and starting with `--rotate`.
- **Request framing (9).** `Transfer-Encoding` is refused (`411`), `Content-Length` is required
  and capped at 4 MiB, any body must be `application/json` (`415`), and the request head is capped
  at 16 KiB (`431`). The server never forwards to another HTTP parser.
- **Paths (28).** Project reads/writes go through `PatchPipeline::resolve`, which refuses `..`,
  absolute paths and symlink escapes. Swarm transitions accept only `[A-Za-z0-9_-]` ids. The
  opt-in `--full-access` mode is a deliberate whole-machine capability.
- **Commands (29).** A command runs through the coordinator approval gate; the exact text, cwd
  and timeout are shown before approval, output is capped and the process tree is killed at the
  timeout. The command endpoint is not a second shell.
- **Prompt injection (22, 61).** Repo files, relay payloads, skill bodies, mail and fetched pages
  are returned as `untrusted: true` data; no model output becomes a Rigel permission. The relay
  bridge surfaces a summary as data and does not grant a tool. An agent can still be persuaded to
  ask for an action; the code gate decides whether it runs.
- **Deserialization (33).** API bodies are typed `serde` objects; unknown/mistyped fields are
  rejected, sizes are capped, and Swarm job ids are checked before a path is built.
- **Update authenticity (27).** `latest.json` is Ed25519-verified over its canonical bytes before
  parsing; the binary/MSI artifacts must match the signed SHA-256 before launch. Authenticode is
  a separate Windows signing gate.
- **Secret handling (21, 30).** Provider keys live in the OS keychain, `Secret` has a redacted
  `Debug`, child processes get a scrubbed environment, and `redaction.rs` / `secret_scan.rs`
  cover tool output, diagnostics and proposed writes.
- **Dependencies (35).** Cargo and npm lockfiles are committed; the CI supply-chain lane runs
  `cargo audit`, `cargo-deny`, gitleaks, the new verified TruffleHog step, and an SBOM.
- **Relay boundary (13, 14, 16, 23, 33).** The relay has one role per token, sender and recipient
  are derived server-side, inbox and acknowledgement queries are scoped to the role, payloads are
  capped at 256 KiB / 32 levels, ids are validated before Postgres, and an idempotency key cannot
  be replayed with a different body.

## The 67 checks

The list mixes Rigel's actual surface with browser/mobile/account checks. Grouping the checks by
disposition is more honest than marking a not-applicable check as "passed":

| Items | Disposition |
|---|---|
| 3, 4, 5, 6, 12, 15, 18, 20, 48, 57, 58, 62, 63, 67 | **Not applicable today.** No SAML, OAuth/social login, GraphQL, TOTP, Postgres RLS, CBC cookies, XML auth parser, account usernames, payments, Apple relay addresses, or public files/upload route. Roles are read from the database, never from a request body. Room deletion is vendor state, not a Rigel read path. |
| 1, 10, 11, 19, 26 | **Deployment or vendor evidence needed.** Rigel's own server is HTTP/1.1 with no cache; Tailscale Serve's HTTP/2, cache and ACL configuration is not in this repository. The API never trusts `X-Forwarded-For`; its limiter keys on the TCP peer. The room WebSocket is hosted by the vendor agentchattr sidecar. |
| 2, 7, 16, 17, 32, 37, 38, 40, 41, 42, 46, 49, 50, 51, 64, 65, 66 | **Source-reviewed; no exploitable boundary found.** Rust `regex` is linear-time; no XML parser is used for untrusted input; no JS deep merge of untrusted objects; React escapes displayed text; API bodies/fields are capped; the cloud error handler hides detail in production and local errors go to an authenticated caller; uploads are local to the operator; mail-domain SPF/DKIM/DMARC is external; shipped client code contains no secret. The UI-only resize/theme/RTL/timezone checks belong to the UI test lane, not a security boundary. |
| 8, 13, 14, 21, 22, 23, 24, 25, 27, 28, 29, 30, 31, 33, 34, 35, 45, 52, 55, 56, 59, 60, 61 | **Covered or fixed above.** |
| 36, 39, 43, 44, 47, 53, 54 | **Hardening backlog, not a demonstrated boundary failure.** Double-submit/idempotency, delete-then-recreate, permission denial and cross-session refresh are product-state checks. The API has request timeouts and atomic writes; the relay caps payloads and has idempotency keys. |

## Secret scan

TruffleHog 3.100.0 (Windows amd64) was downloaded from its GitHub release and its SHA-256 matched
the published checksum. It was run with `--no-verification` so the audit did not send candidates to
third-party APIs:

```text
trufflehog filesystem . --no-verification --results=unverified --json \
  --concurrency=4 -x .security-tools/exclude.txt
trufflehog git file://. --no-verification --results=unverified --json \
  --concurrency=4 --max-depth=10
```

The tracked tree produced no live credential. The only non-fixture result was the Firebase web API
key in the gitignored generated `.deploy`/`deploy` configuration; it is public by design and
Firestore rules are the boundary. Test fixtures, commented `.env.example` placeholders, the vendored
Python sidecar, the generated preview, the handoff zip and the plugin catalogue produced unverified
detector noise and were excluded from the second pass. The last ten commits produced no finding.
The full local history scan was stopped after the repository's 1.5 GB `.git` (including a 584 MB
unreachable temporary pack) made it impractically slow; CI checks out full history and runs
gitleaks there, so a historical commit is still covered by the lane rather than by this local run.
The local `deploy/.env.production` is not tracked and contains production-shaped values; it is
outside the git inventory but still needs rotation if the checkout was copied.

## Remaining validation

1. **Tailscale ACL (26).** Confirm only the operator's devices can reach the Serve URL, and keep
   the desktop and API paths on separate rules. This is configuration, not source.
2. **Room WebSocket (19).** The agentchattr sidecar is vendored; verify its upgrade checks Origin
   and requires its token. Rigel's bridge is a client, not the upgrade server.
3. **Signing (27).** The Ed25519 update path is in-tree; Authenticode/MSI signing depends on the
   release certificate and the release lane. Do not claim a signed installer until that lane
   produces one.
4. **Production secrets.** Rotate the relay tokens, Payload secret and admin password in
   `deploy/.env.production` (and the local `cms/.env` values) after any copy or share, and keep
   them in the deployment secret store rather than the working tree.
