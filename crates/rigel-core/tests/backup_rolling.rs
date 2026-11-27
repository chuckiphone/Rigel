use std::fs;

use rigel_core::backup::{self, ARCHIVE_EXTENSION};
use tempfile::TempDir;

const PASSPHRASE: &str = "rolling-backup integration passphrase";

#[tokio::test]
async fn daily_backups_are_verified_and_pruned_to_the_requested_retention() {
    let temp = TempDir::new().unwrap();
    let data = temp.path().join("data");
    let backups = data.join("backups");
    fs::create_dir_all(&backups).unwrap();
    fs::write(data.join("settings.toml"), "theme = 'paper'\n").unwrap();

    let oldest = backups.join(format!("rigel-day-100.{ARCHIVE_EXTENSION}"));
    let newer = backups.join(format!("rigel-day-200.{ARCHIVE_EXTENSION}"));
    backup::create(&data, &oldest, PASSPHRASE).await.unwrap();
    backup::create(&data, &newer, PASSPHRASE).await.unwrap();

    let report = backup::create_daily(&data, &backups, PASSPHRASE, 2)
        .await
        .unwrap();

    assert!(report.created);
    assert_eq!(report.kept, 2);
    assert_eq!(report.pruned, vec![oldest.clone()]);
    assert!(!oldest.exists());
    assert!(backup::verify(&newer, PASSPHRASE).is_ok());
    assert!(backup::verify(&report.path, PASSPHRASE).is_ok());

    let repeated = backup::create_daily(&data, &backups, PASSPHRASE, 2)
        .await
        .unwrap();
    assert!(!repeated.created);
    assert!(repeated.pruned.is_empty());
}

#[tokio::test]
async fn restore_requires_successful_scratch_rehearsal_before_installing_files() {
    let temp = TempDir::new().unwrap();
    let data = temp.path().join("data");
    fs::create_dir_all(data.join("snapshots")).unwrap();
    fs::write(data.join("settings.toml"), "theme = 'paper'\n").unwrap();
    fs::write(data.join("snapshots/keep.txt"), "restore this\n").unwrap();
    let archive = temp.path().join("backup.rigel-backup");
    backup::create(&data, &archive, PASSPHRASE).await.unwrap();
    assert_eq!(backup::verify(&archive, PASSPHRASE).unwrap().files, 2);

    let failed_target = temp.path().join("failed-restore");
    assert!(
        backup::restore(&archive, &failed_target, "wrong passphrase")
            .await
            .is_err()
    );
    assert!(!failed_target.exists());

    let target = temp.path().join("restored-project");
    let report = backup::restore(&archive, &target, PASSPHRASE)
        .await
        .unwrap();
    assert!(report.rehearsal.ok);
    assert_eq!(report.rehearsal.files, report.files);
    assert_eq!(report.rehearsal.bytes, report.bytes);
    assert_eq!(report.rehearsal.database, "not present");
    assert_eq!(
        fs::read_to_string(target.join("snapshots/keep.txt")).unwrap(),
        "restore this\n"
    );
}
