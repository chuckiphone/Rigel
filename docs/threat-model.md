# Threat model (roadmap 79)

Scope: the Rigel desktop app and its native agent. Reviewed 2026-11-06 and extended
2026-11-22 for the hosted/self-hosted API boundary against the code in this tree. The local-first data inventory is in [privacy.md](privacy.md); the vulnerability route is in
[SECURITY.md](../SECURITY.md).

## Assets

- The user's source tree and the contents of `.rigel/rigel.db` (prompts, tool output, patches).
- Provider, harness, Firebase, AgentMail and MCP credentials.
- The user's provider spend and the integrity of commands the agent runs.
- The user's cloud-published session documents.

## 1. Someone with the user's OS account, or a stolen unlocked machine

**Mitigation.** Provider keys, the Firebase refresh token, the App Check client token and the
AgentMail key are stored in the OS keychain, not in `settings.toml` or `cloud.json`. The local
database is readable only through the user's account. Signing out removes the local cloud
credentials, and account deletion is a typed confirmation with an export offered first.

**Mitigation (at rest).** The project database can be encrypted with SQLCipher; the key lives in
the OS keychain or `RIGEL_DB_KEY`, and a missing or wrong key fails closed. See
[encryption.md](encryption.md).

**Non-goal: defending against an administrator or kernel-level attacker.** Rigel runs as an
ordinary desktop process and cannot protect memory or the keychain from the account that owns
them. Encryption is opt-in and does not cover a plaintext project, the index database, crash
dumps or files outside `.rigel/rigel.db`; full-disk encryption remains the user's control.

## 2. A malicious repository the user opens

**Mitigation.** File tools resolve through the patch pipeline's jail, which refuses `..` and
symlink escapes. Repository hooks are not run automatically, and a project MCP configuration is
shown and approved before its server is started. Skill scripts are surfaced as script approvals
with the path the user can inspect.

**Non-goal: treating repository text as trusted.** `AGENTS.md`, README files, issue text and tool
output can all contain instructions aimed at the model. Rigel does not claim to classify them
reliably. The boundary is that model text cannot approve a command, write outside the jail, or
spend without the same gates a user-initiated turn uses.

## 3. A compromised or malicious model provider

**Mitigation.** A provider response is data. Every command, write, network fetch, mail send and
skill script still passes its own approval or policy gate, and the provider has no way to answer
an approval. A cancelled stream is billed at most once and closes as cancelled.

**Non-goal: content-filtering provider output for prompt injection.** Instructions and the data
they refer to arrive on one channel, and a filter that removes "ignore previous instructions"
also removes legitimate code and prose. The reason for the non-goal is that the enforceable
boundary is the tool gate, not a classifier. Residual risk: a user who approves a command the
model composed can still run it.

## 4. A malicious MCP server or MCP tool

**Mitigation.** Adding a project server requires seeing its real `tools/list` before approval.
Each tool call is presented with its arguments and is approved according to the same scopes as a
native call. A server that fails its handshake produces an error rather than a silent empty tool
list.

**Non-goal: sandboxing a stdio MCP server.** The server is a child process running as the user,
so it can do anything the user can. There is no OS sandbox in this release; the reason is that a
portable Windows sandbox is not part of the current runtime. Users should only approve servers
whose code they trust.

## 5. A stolen cloud refresh token

**Mitigation.** Cloud is opt-in and runs against the user's own Firebase project. Firestore rules
scope documents to the authenticated user id, so a token for one account cannot read another
account's shelf. Sign-out clears the local credential; deleting a shelf row writes a tombstone;
account deletion is explicit and offers export first.

**Non-goal: revoking a token after it has left the machine.** Revocation is a Firebase project
operation, not something the desktop app can perform offline. The project owner can rotate or
disable the account in the Firebase console, and provider-side session revocation remains the
project's control.

## 6. A web page or another local account reaching the local API

**Mitigation.** `rigel serve` binds loopback by default and refuses `0.0.0.0`; it accepts one
named interface only when the operator asks for it. Every route needs a 244-bit bearer token
compared in constant time, any request carrying an `Origin` header is refused, and no CORS
headers are sent. On Unix the token file is written `0600`, so another account cannot read it.
With `--full-access` the API can reach the whole computer; that is an explicit flag, and the
same bearer and Origin gates still apply.

**Non-goal: isolation from another process running as the same user.** Such a process can read
the token file and the project database. The API separates the coordinator from browsers and
other machines, not from the account that owns them.

## 7. A stolen API token, or a session that should have ended

**Mitigation.** Hosted accounts authenticate with a short-lived JWT, but every request also
checks the `auth_sessions` row: a revoked, expired or logged-out session is refused even while
its JWT has minutes left. Logout-all and a password change revoke every other session. Account
deletion requires the current password in addition to the typed confirmation. Published session
and turn reads are scoped by `user_id` in SQL, and the tests cover a second account receiving a
`404` rather than another user's row. The relay has two role-specific machine credentials and
cannot read the other role's inbox.

**Non-goal: hiding an account's existence at signup.** Signup answers `409 email_taken`, so an
attacker can test whether an address has an account. The answer is deliberate: the alternative
silently discards a real registration. Password reset does not reveal account existence.

## Residual risks

- The database is plaintext unless the user enables SQLCipher, and backups are the user's
  responsibility.
- A user can approve a destructive command; approvals show exact text, cwd and timeout, but do
  not judge intent.
- Third-party providers, Firebase, AgentMail and MCP servers apply their own retention and
  security policies.
- The hosted/self-hosted relay is single-tenant: its two relay credentials are deployment-wide,
  not per user. A self-hoster who needs unrelated teams isolated should run separate deployments.
