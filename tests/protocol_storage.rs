//! Offline contract checks for the actual Slint + WhatsApp + SQLCipher graph.
use diesel::{RunQueryDsl, connection::SimpleConnection};
use whatsapp_rust::store::{SqliteDatabase, SqliteDatabaseConfig};

#[tokio::test]
async fn protocol_store_uses_encrypted_connections_and_rejects_wrong_keys() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic-session.db");
    // Synthetic test key only. Production keys will come from the OS keyring.
    let config = SqliteDatabaseConfig::default()
        .with_read_pool_size(2)
        .with_connection_init(|conn| {
            conn.batch_execute("PRAGMA key = 'heya-synthetic-test-key';")?;
            diesel::sql_query("SELECT count(*) FROM sqlite_master").execute(conn)?;
            Ok(())
        });
    let database = SqliteDatabase::open(path.to_str().unwrap(), config)
        .await
        .unwrap();
    let device = database.create_device().await.unwrap();
    assert!(database.device_exists(device.device_id()).await.unwrap());
    assert_eq!(database.list_devices().await.unwrap().len(), 1);

    let reader = rusqlite::Connection::open(&path).unwrap();
    let cipher: String = reader
        .query_row("PRAGMA cipher_version", [], |r| r.get(0))
        .unwrap();
    assert!(!cipher.is_empty());
    reader
        .pragma_update(None, "key", "heya-synthetic-test-key")
        .unwrap();
    let tables: i64 = reader
        .query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
        .unwrap();
    assert!(tables > 0);
    let provider: String = reader
        .query_row("PRAGMA cipher_provider", [], |r| r.get(0))
        .unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(provider, "commoncrypto");
    #[cfg(target_os = "linux")]
    assert_eq!(provider, "openssl");
    reader.execute_batch("CREATE VIRTUAL TABLE temp.search_probe USING fts5(text); INSERT INTO search_probe VALUES ('Hello Heya');").unwrap();
    let hits: i64 = reader
        .query_row(
            "SELECT count(*) FROM search_probe WHERE search_probe MATCH 'Heya'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1);

    let wrong = rusqlite::Connection::open(&path).unwrap();
    wrong
        .pragma_update(None, "key", "a-different-test-key")
        .unwrap();
    assert!(
        wrong
            .query_row::<i64, _, _>("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .is_err()
    );
    let unkeyed = rusqlite::Connection::open(&path).unwrap();
    assert!(
        unkeyed
            .query_row::<i64, _, _>("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
            .is_err()
    );
    assert_ne!(&std::fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
}
