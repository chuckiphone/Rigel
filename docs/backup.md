# Encrypted backups

`rigel backup` writes an encrypted, self-describing archive of the global data directory: the
session index, settings, snapshots, and the other local state Rigel needs to come back after a bad
upgrade, a lost disk or an accidental `rm`. The archive is local-first and has no account, network
call or hosted copy.

The command surface is:

```
rigel backup create  [--out FILE] [--passphrase-file FILE] [--json]
rigel backup verify  <ARCHIVE> [--passphrase-file FILE] [--json]
rigel backup restore <ARCHIVE> --into DIR [--passphrase-file FILE] [--json]
rigel backup rotate  <ARCHIVE> --out FILE [--passphrase-file FILE]
                     [--new-passphrase-file FILE] [--json]
rigel backup drill   <ARCHIVE> [--passphrase-file FILE] [--max-age-days DAYS] [--json]
```

`--data-dir DIR` (or `RIGEL_INDEX_DIR`) selects the directory being backed up. It defaults to the
per-user app data directory, the same directory the window and the CLI use.

## Creating a backup

```
rigel backup create --passphrase-file ~/.rigel-backup.pass
```

The default destination is `<data-dir>/backups/rigel-<timestamp>.rigel-backup`; `--out` writes
somewhere else. A backup is not overwritten: an existing destination is an error.

`create` refuses to write an archive inside the data directory unless it is under
`<data-dir>/backups`, because an archive left among the source files would otherwise be included in
the next backup.

The archive contains:

- every regular file under the data directory, at the same relative path;
- `index.db`, captured with SQLite `VACUUM INTO` so a live database is copied consistently rather
  than read page-by-page through a changing file;
- a JSON manifest at the end listing the creation time, source directory, every file's relative
  path, byte length and SHA-256, and the total size.

The source files still change while a backup runs. `create` decrypts its own finished archive and
checks every hash before renaming it into place, so a file that moved between hashing and copying
fails the command instead of producing a backup that only looks successful.

The following directories are deliberately skipped and named in the create report:

- `backups/` — the archives themselves;
- `updates/` — staged update downloads and the apply receipt;
- `diagnostics/` — generated diagnostics bundles;
- `agentchat/` — a live chat room's local state.

`index.db-wal` and `index.db-shm` are also skipped; the `VACUUM INTO` snapshot already includes
committed WAL content.

## Passphrases

The passphrase is read from `--passphrase-file`, then from `RIGEL_BACKUP_PASSPHRASE`. It is never
accepted as a command-line argument, so it does not appear in the process list or shell history.
The file may end in one newline; a CRLF file is accepted. A file that only contains whitespace is
refused.

Losing the passphrase loses the backup. There is no recovery key and no server-side copy.

`rotate` also reads `RIGEL_BACKUP_NEW_PASSPHRASE`, or `--new-passphrase-file`. The old and new
passphrases must differ.

## Verifying and detecting corruption

```
rigel backup verify <archive> --passphrase-file ~/.rigel-backup.pass
```

`verify` authenticates the age stream, reads every file out of the tar, checks each file's length
and SHA-256 against the manifest, and checks that the manifest's file count and total size agree
with what it read. A wrong passphrase, a truncated or modified archive, a missing manifest, a
duplicate entry or a path that tries to escape the restore root all fail with a non-zero exit.

Use `--json` for a machine-readable report (`path`, `files`, `bytes`, `created_at`, `source`).

## Restoring

```
rigel backup restore <archive> --into ~/Rigel-restored --passphrase-file ~/.rigel-backup.pass
```

Restore is deliberately not an in-place overwrite. `--into` must name a directory that does not
exist or is empty. The archive is extracted into a sibling staging directory, then renamed into
place only after every entry has passed verification. A failed restore removes its staging tree,
leaves the destination untouched and never deletes an existing non-empty directory.

To replace a live install: close Rigel, move the old data directory aside, restore into the old
path (or restore elsewhere and point `RIGEL_INDEX_DIR` at it), then reopen. Restoring while the
window is running is not supported because the database and settings files are live.

The manifest records the source path for provenance; restore does not recreate directory
permissions, symlinks or empty directories. Only regular files are captured.

## Rotation

```
rigel backup rotate old.rigel-backup --out new.rigel-backup \
  --passphrase-file old.pass --new-passphrase-file new.pass
```

Rotation decrypts the old archive into a new age stream under the new passphrase, preserving the
same tar and manifest. The old archive is left untouched; verify the new one with the new
passphrase before deleting the old one.

## The restore drill

```
rigel backup drill <archive> --passphrase-file ~/.rigel-backup.pass
```

A drill restores the archive into a temporary directory, runs SQLite's `PRAGMA integrity_check`
against the restored `index.db` when one is present, and removes the temporary directory. It writes
`<data-dir>/backups/last-drill.json` with the archive path, file count, bytes, database result and
check time.

`--max-age-days N` makes the command idempotent for a scheduler: if the receipt is for the same
archive and was written within `N` days, the command exits 0 and reports `"skipped": true`. The
default is `0`, which always runs the drill. A failed drill removes the receipt, so a later
scheduled run does not skip past a regression.

Run it from Task Scheduler or cron, for example weekly, against the archive you keep for the drill
(create one with a fixed, deliberate `--out` name, or pass the path of the archive you want
checked):

```
rigel backup drill C:\Users\me\AppData\Roaming\Rigel\backups\weekly.rigel-backup `
  --passphrase-file C:\Users\me\.rigel-backup.pass --max-age-days 7 --json
```

or, on macOS/Linux:

```
0 3 * * 0 rigel backup drill "$HOME/.local/share/rigel/backups/weekly.rigel-backup" \
  --passphrase-file "$HOME/.rigel-backup.pass" --max-age-days 7 --json
```

The receipt is advisory state, not a second backup: it lives under `backups/` and is excluded from
later archives.

## Format and limits

- Encryption is [age](https://age-encryption.org/) v1 with a passphrase and scrypt. The bytes on
  disk are an age file; plaintext file names and contents are not visible. `age`/`rage` can decrypt
  it with the same passphrase if Rigel is unavailable.
- The payload is a tar stream followed by `backup-manifest.json`; the manifest is the last entry,
  so a truncated archive has no manifest and fails verification.
- Limits are 50,000 files and 16 GiB per archive, checked before encryption. The archive is not
  compressed.
- Restore rejects absolute paths, `..` components and backslashes in manifest entries before
  writing anything.
- A source file whose name contains a backslash is refused, because a tar entry with one cannot
  be restored safely on Windows.

## Tests

`crates/rigel-core/src/backup.rs` covers path validation, a full create/verify/restore/rotate
round-trip, a tampered archive, a non-empty restore destination, and the default backup location.
`crates/rigel-cli/tests/backup.rs` drives the real binary through create, verification with a file
and with the environment variable, a wrong passphrase, restore, drill, skip and rotation.
