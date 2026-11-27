//! `rigel backup` as a program: encrypted create, verification, restore, rotation and drill.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn output_json(mut command: Command) -> (i32, Value) {
    let output = command.output().expect("rigel runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "not json: {error}\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        json["schema_version"], 1,
        "CLI JSON schema version changed; review the shape contract"
    );
    (output.status.code().unwrap_or(-1), json)
}

fn run(data: &Path, args: &[&str]) -> (i32, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rigel"));
    command.arg("--data-dir").arg(data).args(args);
    output_json(command)
}

fn run_env(data: &Path, args: &[&str], secret: &str) -> (i32, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rigel"));
    command
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env("RIGEL_BACKUP_PASSPHRASE", secret);
    output_json(command)
}

#[tokio::test]
async fn encrypted_backups_verify_restore_rotate_and_drill_through_the_cli() {
    let work = TempDir::new().unwrap();
    let data = work.path().join("data");
    fs::create_dir_all(data.join("snapshots/one/files")).unwrap();
    fs::write(data.join("settings.toml"), "theme = \"paper\"\n").unwrap();
    fs::write(
        data.join("snapshots/one/files/only-copy.txt"),
        "the only copy\n",
    )
    .unwrap();

    let created = Command::new(env!("CARGO_BIN_EXE_rigel"))
        .arg("--data-dir")
        .arg(&data)
        .args(["skills", "stats", "--json"])
        .output()
        .expect("rigel creates the index");
    assert!(
        created.status.success(),
        "skills stats failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let old_pass = work.path().join("old.pass");
    let new_pass = work.path().join("new.pass");
    fs::write(&old_pass, "correct horse battery staple\n").unwrap();
    fs::write(&new_pass, "another long passphrase\n").unwrap();
    let archive = work.path().join("one.rigel-backup");

    let (code, created) = run(
        &data,
        &[
            "backup",
            "create",
            "--out",
            archive.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "create: {created}");
    assert_eq!(created["files"], 3);
    assert!(
        !fs::read(&archive)
            .unwrap()
            .windows(15)
            .any(|window| window == b"the only copy\n"),
        "the archive must not contain plaintext"
    );

    let (code, verified) = run(
        &data,
        &[
            "backup",
            "verify",
            archive.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "verify: {verified}");
    assert_eq!(verified["files"], 3);

    let (code, via_env) = run_env(
        &data,
        &["backup", "verify", archive.to_str().unwrap(), "--json"],
        "correct horse battery staple",
    );
    assert_eq!(code, 0, "env passphrase: {via_env}");

    let wrong = work.path().join("wrong.pass");
    fs::write(&wrong, "wrong passphrase").unwrap();
    let (code, rejected) = run(
        &data,
        &[
            "backup",
            "verify",
            archive.to_str().unwrap(),
            "--passphrase-file",
            wrong.to_str().unwrap(),
            "--json",
        ],
    );
    assert_ne!(code, 0);
    assert_eq!(rejected["ok"], false);

    let restored = work.path().join("restored");
    let (code, restored_report) = run(
        &data,
        &[
            "backup",
            "restore",
            archive.to_str().unwrap(),
            "--into",
            restored.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "restore: {restored_report}");
    assert_eq!(
        fs::read_to_string(restored.join("snapshots/one/files/only-copy.txt")).unwrap(),
        "the only copy\n"
    );
    assert!(restored.join("index.db").is_file());

    let (code, drill) = run(
        &data,
        &[
            "backup",
            "drill",
            archive.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "drill: {drill}");
    assert_eq!(drill["database"], "ok");
    let (code, skipped) = run(
        &data,
        &[
            "backup",
            "drill",
            archive.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--max-age-days",
            "7",
            "--json",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(skipped["skipped"], true);

    let rotated = work.path().join("rotated.rigel-backup");
    let (code, rotated_report) = run(
        &data,
        &[
            "backup",
            "rotate",
            archive.to_str().unwrap(),
            "--out",
            rotated.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--new-passphrase-file",
            new_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "rotate: {rotated_report}");
    assert_eq!(rotated_report["files"], 3);

    let (code, _) = run(
        &data,
        &[
            "backup",
            "verify",
            rotated.to_str().unwrap(),
            "--passphrase-file",
            new_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "the rotated archive opens with the new key");
    let (code, _) = run(
        &data,
        &[
            "backup",
            "verify",
            rotated.to_str().unwrap(),
            "--passphrase-file",
            old_pass.to_str().unwrap(),
            "--json",
        ],
    );
    assert_ne!(code, 0, "the old key no longer opens it");
}
