use rigel_core::store::cipher::{database_state, decrypt_database};
use rigel_core::{CipherError, DatabaseState, DbKey, SessionService, StorageError, Store};

fn key(byte: u8) -> DbKey {
    DbKey::parse(&format!("{byte:02x}").repeat(32)).unwrap()
}

async fn session_count(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_new_project_is_encrypted_when_a_key_is_configured_and_needs_the_right_key() {
    let tmp = tempfile::tempdir().unwrap();
    let right = key(0x11);
    let wrong = key(0x22);

    let store = Store::open_project_with_key(tmp.path(), Some(&right))
        .await
        .unwrap();
    let path = store.path().to_path_buf();
    assert_eq!(database_state(&path).unwrap(), DatabaseState::Encrypted);
    let cipher_version: String = sqlx::query_scalar("PRAGMA cipher_version")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert!(
        cipher_version.starts_with("4."),
        "SQLCipher should be linked, saw {cipher_version:?}"
    );

    let service = SessionService::from_store(&store, tmp.path());
    service.start(Some("encrypted"), None).await.unwrap();
    assert_eq!(session_count(&store).await, 1);
    store.pool().close().await;

    let missing = match Store::open_project_with_key(tmp.path(), None).await {
        Ok(_) => panic!("an encrypted database must not open without a key"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &missing,
            StorageError::Cipher(CipherError::MissingKey { path: found })
                if found == &path
        ),
        "missing key: {missing:?}"
    );

    let wrong_error = match Store::open_project_with_key(tmp.path(), Some(&wrong)).await {
        Ok(_) => panic!("a wrong key must not open the database"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &wrong_error,
            StorageError::Cipher(CipherError::WrongKey { path: found, .. })
                if found == &path
        ),
        "wrong key: {wrong_error:?}"
    );

    let reopened = Store::open_project_with_key(tmp.path(), Some(&right))
        .await
        .unwrap();
    assert_eq!(session_count(&reopened).await, 1);
    reopened.pool().close().await;
}

#[tokio::test]
async fn a_plaintext_project_migrates_when_a_key_is_configured_and_decrypts_back() {
    let tmp = tempfile::tempdir().unwrap();
    let key = key(0x33);

    let plaintext = Store::open_project_with_key(tmp.path(), None)
        .await
        .unwrap();
    let path = plaintext.path().to_path_buf();
    assert_eq!(database_state(&path).unwrap(), DatabaseState::Plaintext);
    let service = SessionService::from_store(&plaintext, tmp.path());
    service.start(Some("plaintext"), None).await.unwrap();
    plaintext.pool().close().await;
    drop(service);
    drop(plaintext);

    let encrypted = Store::open_project_with_key(tmp.path(), Some(&key))
        .await
        .unwrap();
    assert_eq!(database_state(&path).unwrap(), DatabaseState::Encrypted);
    assert_eq!(session_count(&encrypted).await, 1);
    encrypted.pool().close().await;

    decrypt_database(&path, &key).await.unwrap();
    assert_eq!(database_state(&path).unwrap(), DatabaseState::Plaintext);

    let reopened = Store::open_project_with_key(tmp.path(), None)
        .await
        .unwrap();
    assert_eq!(session_count(&reopened).await, 1);
    reopened.pool().close().await;
}

#[tokio::test]
async fn an_encrypted_daily_backup_does_not_expose_plaintext() {
    let tmp = tempfile::tempdir().unwrap();
    let key = key(0x44);
    let store = Store::open_project_with_key(tmp.path(), Some(&key))
        .await
        .unwrap();
    let backup = store
        .backup_daily()
        .await
        .unwrap()
        .expect("the project database has a daily backup");
    assert_eq!(
        database_state(&backup).unwrap(),
        DatabaseState::Encrypted,
        "backups of an encrypted database must stay encrypted"
    );
    store.pool().close().await;
}
