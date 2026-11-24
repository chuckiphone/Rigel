# Updates

Rigel checks its own release feed once at launch, verifies what it finds, and — only when a newer
signed release exists — replaces its executable and starts the new build. There is no update
dialog, no banner and no timer. A current build, an offline machine, a rate-limited GitHub and a
repository with no release yet all look identical to the user: nothing happens.

The release feed is `https://github.com/chuckiphone/Rigel/releases/latest/download/latest.json`. The
release workflow publishes that file beside the NSIS installer, the MSI and the raw executable.

## The launch check

1. **One request, off the window thread.** The check starts after the window is created and the
   app is already usable. It reads `latest.json` with a 10-second timeout; a slow or unresponsive
   host is abandoned rather than holding anything.
2. **Signature first.** The manifest is parsed only after its Ed25519 signature verifies against
   the trusted key: `update-pubkey.txt` in the data directory, or `RIGEL_UPDATE_PUBKEY`.
3. **Version.** The signed version is compared semantically with this build. Equal, older, a
   pre-release of the current version, a malformed tag and a downgrade are all “no update”.
4. **Artifact and platform.** The manifest names the installers and, for the self-replace path, a
   raw executable with its own signature and a platform tag (`windows-x86_64`). A binary for a
   different platform is not offered at all.
5. **Hash.** The download is hashed while it streams and compared with the signed SHA-256. A
   truncated body, a wrong length or a wrong hash deletes the partial file and stops.

Nothing visible happens in any of the failure cases. One line is appended to
`<data-dir>/update.log`; the install that is running is never touched.

## The deferred self-replace

A running Windows executable cannot be overwritten: the image section is locked. The app therefore
does not try. After downloading and verifying the replacement it writes
`<data-dir>/updates/apply.json` and spawns a second, headless copy of itself:

```
rigel-app --rigel-apply-update <data-dir>/updates/apply.json
```

That helper waits for the app's PID to disappear (up to five minutes), then:

1. re-hashes the staged file and refuses a mismatch;
2. copies the replacement to `<exe>.new` beside the target and hashes the copy;
3. renames the running executable to `<exe>.old`;
4. renames `<exe>.new` over the target;
5. starts the target with the original arguments and working directory, and sets
   `RIGEL_UPDATED_TO=<version>` for that one launch;
6. removes the backup and the staged download.

If step 3, 4 or 5 fails, the backup is restored and the helper exits non-zero. The app is left
runnable; a half-swapped install is never accepted. The one-launch `RIGEL_UPDATED_TO` guard means
a manifest that is wrong about its own binary cannot cause a relaunch loop.

The same entry point runs on macOS and Linux: the helper is a plain process, the swap uses
`rename`, and the executable bit is set on the new file. On those platforms the release feed
currently publishes Windows assets, so the platform tag makes the binary path a no-op there.

### Installed copies

A per-machine install under `Program Files` cannot write its own directory. The app probes that
before staging, and falls back to the signed installer path: download, verify the hash, start the
NSIS setup with `/S /R` (or the MSI through its detached relaunch helper), then exit so the
installer can replace the files. That is also what older manifests without a binary asset get.

## Switches

The file switches live in `<data-dir>/update.json` and are editable from Settings → Updates or
with `rigel update enable|disable`:

| Setting | Default | Meaning |
| --- | --- | --- |
| `enabled` | on | Make the one launch check |
| `auto_install` | on | Apply a verified newer release without a click |
| `manifest_url` | GitHub latest | A different signed feed |

Environment overrides win over the file and are the supported way to test a feed:

| Variable | Effect |
| --- | --- |
| `RIGEL_UPDATE_DISABLE=1` | No request at all; identical to turning the switch off |
| `RIGEL_UPDATE_MANIFEST_URL=<url>` | Use this signed manifest; loopback is allowed |
| `RIGEL_UPDATE_AUTO_INSTALL=0` | Check, but never stage or relaunch |
| `RIGEL_UPDATE_PUBKEY=<64 hex>` | Trust this Ed25519 key instead of the data-dir file |
| `RIGEL_UPDATE_LAUNCH=1` | Let a development build run the launch check (never by default) |

`RIGEL_UPDATED_TO` is internal: the helper sets it for the relaunched build, and the launch check
skips that one start.

## Log lines

`<data-dir>/update.log` records the launch check and the staging step:

```
up to date 0.3.0
no update: could not fetch the manifest: error sending request: operation timed out
no update: no published release yet at https://…/latest.json
staged 0.4.0 C:\Users\you\AppData\Local\Rigel\updates\apply.json
staged installer 0.4.0
update failed: the download ended at 1024 of 4096 bytes
```

`<data-dir>/updates/apply.log` records the helper:

```
apply 0.4.0: waiting for pid 21472
apply 0.4.0: swapped C:\Users\you\AppData\Local\Rigel\rigel-app.exe
apply 0.4.0: started C:\Users\you\AppData\Local\Rigel\rigel-app.exe
apply 0.4.0: failed: the artifact is …, but the signed manifest says …
```

The helper never shows a window and never writes to the app's UI. A failure appears once, in the
log, and the current build keeps running.

## The doctor receipt

`rigel doctor` includes one `updates` row that answers “which channel, feed and result am I on?”
without opening any file. It prints the switch, the release channel (`stable` or `prerelease`),
this build's version, the manifest URL, and the last non-empty line of `update.log`. When
`updates/apply.log` exists, its last line follows as `last install: …`. An absent log is reported
as `last check: none recorded` rather than invented; the row turns into a warning only when the
switch is on and the public key is missing.

```
ok     updates                on · prerelease v0.4.0-1 · https://github.com/chuckiphone/Rigel/releases/latest/download/latest.json · last check: up to date 0.4.0-1
```

## Proving it without publishing

`crates/rigel-core/tests/updater_apply_e2e.rs` serves a signed release from a loopback HTTP server
and runs a throwaway stub executable in a temp directory:

- **current:** the manifest matches the “running” version. Only `latest.json` is requested, the
  binary is byte-identical, no `updates/` directory is created and nothing is started.
- **newer:** the manifest is `9.9.9`. The asset downloads, the helper waits for the stub to exit,
  swaps the executable, starts the replacement with the original arguments and working directory,
  and the log shows the three apply lines.

Run it with:

```bash
cargo test -p rigel-core --test updater_apply_e2e -- --test-threads=1 --nocapture
```
