# Database encryption

Rigel's project database (`.rigel/rigel.db`) can be encrypted at rest with SQLCipher 4. The
global index (`index.db`) and preferences stay plaintext; they contain rebuildable index data,
not the session transcript.

## Turning it on

Encryption is opt-in. With no key configured a project opens as ordinary SQLite, so existing
projects keep working unchanged.

```sh
rigel db status  --project .
rigel db encrypt --project .
```

`rigel db encrypt` uses the configured key if one exists. If none does, it generates a random
32-byte key, stores it in the OS keychain (Windows Credential Manager, macOS Keychain or Secret
Service under `rigel/db-key`) and prints only the database path. The key is never printed.

For a headless machine, set `RIGEL_DB_KEY` to 64 hexadecimal characters (32 bytes) instead. The
environment wins over the keychain. Do not put this value in `.env`, a project file, a commit or
a log. A blank or malformed value is an error; Rigel does not fall back to plaintext.

## What happens on open

- **No key, plaintext file:** opens normally.
- **No key, encrypted file:** fails with an actionable error. It never creates a new empty
  plaintext database over the encrypted one.
- **Key, plaintext file:** migrates the database in place before opening it.
- **Key, encrypted file:** decrypts and opens it. A wrong key fails closed with a wrong-key
  error, not a plaintext fallback.
- **Key, new file:** creates an encrypted database.

The migration uses SQLCipher's `sqlcipher_export` into a side file, runs `PRAGMA integrity_check`
on the result, and only then swaps it into place. The original is kept until that check passes.
Daily copies under `.rigel/backups/` are written with the same key.

## Turning it off

```sh
rigel db decrypt --project .
```

This exports a plaintext copy, verifies it, and swaps it in. It does not delete the keychain
entry; `rigel db forget-key` does that. A database decrypted this way is exposed to anything that
can read the user's files, exactly like a project that never enabled encryption.

## Recovery limits

Encryption protects the database file at rest. It does not protect a running process, its memory,
the OS keychain, or the user's account. Full-disk encryption is still the control for the rest of
the repository and the machine.

There is no key rotation command yet. Losing the key makes the database unrecoverable; there is
no recovery phrase or vendor-held escrow. Keep the keychain backed up according to the OS's own
guidance, or keep an offline copy of a `RIGEL_DB_KEY` value.

The in-place migration is not a backup system. If the process is interrupted while the file is
being swapped, the previous file is left beside the database as
`rigel.db.pre-encryption-<pid>` or `rigel.db.pre-decryption-<pid>`; rename it back before opening
the project. The normal daily backup is the first recovery choice.

SQLCipher encrypts the database and its WAL/journal content. Temporary files, crash dumps,
diagnostics exported by the user, and source files in the repository are outside this boundary.
See [privacy.md](privacy.md) and [threat-model.md](threat-model.md).
