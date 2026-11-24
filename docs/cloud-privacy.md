# Cloud and outbound privacy receipt (roadmap 173)

Scope: the 0.3.0 desktop build. Cloud is **off by default**: no Firebase project is contacted
until the user configures one and signs in. Rigel sends no telemetry, analytics or crash reports.
This receipt lists every outbound call the app can make, why it makes it, and who retains the
data. The narrative summary is in [privacy.md](privacy.md); the beta terms are in
[terms.md](terms.md).

## Operator and billing

Cloud sync remains the user's own Firebase project: that project's owner is the operator, Google
Cloud bills that project's usage to its owner, and Rigel adds no fee. A build with no such
project configured keeps cloud off.

The `rigel` model door and the room relay are separate from cloud sync. They use a Rigel API:
by default the hosted API at `https://api.askrigel.com`, or a deployment the user runs from
`deploy/`. Settings → Rigel API records the choice in `backend.json`; the relay credential is in
the OS keychain, never in that file. Self-hosting keeps relay payloads on the operator's server.
There is no hosted plan to buy and no billing account in either placement.

| Destination | When | Data sent | Purpose | Retention and control |
|---|---|---|---|---|
| Selected model provider | Every turn | Prompt, conversation context, tool results the turn needs | Inference | The provider's own policy and account settings; Rigel keeps only local spend and transcript rows |
| Rigel API (hosted or self-hosted) | When the `rigel` model is selected, or the room relay is running | Relay envelopes: relay-turn prompts and replies, room messages, and Swarm job notices | Remote agent and room bridge | The deployment's retention; a self-hosted operator controls it. The relay credential stays in the OS keychain |
| Firebase Identity Toolkit | Sign-in only | Email and the refresh token exchanged for ID tokens | Authentication | The Firebase project the user configured; the refresh token stays in the OS keychain |
| Cloud Firestore | **Publish to phone**, or after each turn only when automatic publishing is on | Session title, user and assistant text, timestamps | Phone sync | Until the user deletes the row; deletion writes a tombstone. The project owner controls backups and retention |
| Firebase App Check endpoint | Requests to Firebase when App Check is configured | Per-install app token | Attestation | Google and the user's Firebase project; the client token is in the OS keychain |
| GitHub releases | Launch and on demand when the update check is on | Current version and platform | Update manifest | GitHub's logs; no account data. Auto-install downloads only after the signed manifest verifies |
| GitHub, SkillsMP, SkillHub | At launch and on the window's background checks while source auto-update is on; `rigel skills watch`; on demand from `rigel skills index` or `rigel skills sync` | Skill names and search terms | Skill discovery | Each source's policy; no prompt or project file is sent |
| AgentMail | Only when the user reads or sends mail | Mailbox query or message body | Mail | The AgentMail account's policy; no mailbox is polled |
| GitHub forge and pull requests | Only when the user invokes the action | Repository, branch, pull-request text | Forge action | GitHub's policy; the action is never scheduled |
| Configured MCP servers | Only when that server's tool is used | Tool arguments and the result the tool returns | Tool execution | The server operator's policy; a stdio server is a local child process |
| Update installer download | Only when an update is offered and auto-install is on | Request for the published installer | Update | GitHub or the release CDN; the artifact's SHA-256 must match the signed manifest |

## What is not sent

- The local SQLite database, command output, patches and file changes are not uploaded.
- A cloud session document contains title, prompts and assistant text only; tool calls, command
  output, file changes and the project path are excluded.
- Secrets in the OS keychain are never included in diagnostics; the diagnostics export runs the
  redaction corpus before the bundle is written.

## Retention and deletion

Local rows stay until archived or deleted; archiving never destroys rows. Cloud rows are deleted
by the user, with a tombstone so another device does not resurrect them. Account deletion is a
typed confirmation and offers an export first. Provider and Firebase retention are controlled by
the accounts the user configured, not by Rigel.

## Review

Reviewed 2026-11-06 by the security owner; the hosted/self-hosted API placement was added on
2026-11-22. The receipt is reviewed on every minor release and whenever a new outbound
destination is added; a new call without a row here is a release blocker.
