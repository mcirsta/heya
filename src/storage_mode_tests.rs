use super::*;
use crate::storage::tests::MemoryVault;
use rusqlite::Connection;

struct NoVault;
impl SecretVault for NoVault {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, Failure> {
        panic!("plaintext must not access the key store")
    }
    fn write(&self, _: &[u8]) -> Result<(), Failure> {
        panic!("plaintext must not access the key store")
    }
}

fn populate(files: &SessionFiles) {
    for (name, key) in [
        ("session.db", &files.key_sql),
        ("archive.db", &files.archive_key_sql),
    ] {
        let db = Connection::open(files.path.with_file_name(name)).unwrap();
        db.execute_batch(key).unwrap();
        db.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA user_version=18;
            CREATE TABLE content(id INTEGER PRIMARY KEY, body TEXT, media BLOB);
            CREATE INDEX body_index ON content(body);
            INSERT INTO content VALUES(1, 'Synthetic draft 👋\nsecond line', x'0001ff');",
        )
        .unwrap();
    }
}
fn verify(files: &SessionFiles, encrypted: bool) {
    for (name, key) in [
        ("session.db", &files.key_sql),
        ("archive.db", &files.archive_key_sql),
    ] {
        let path = files.path.with_file_name(name);
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.starts_with(b"SQLite format 3\0"), !encrypted);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(key).unwrap();
        let (text, media): (String, Vec<u8>) = db
            .query_row("SELECT body, media FROM content WHERE id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(text, "Synthetic draft 👋\nsecond line");
        assert_eq!(media, [0, 1, 255]);
        assert_eq!(
            db.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
                .unwrap(),
            18
        );
    }
}

#[test]
fn first_run_plaintext_and_repeated_bidirectional_conversion_preserve_both_databases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(!read(root).unwrap().configured);
    configure_at(root, Mode::Unencrypted).unwrap();
    let files = prepare_at(root, &NoVault).unwrap();
    populate(&files);
    verify(&files, false);
    assert!(matches!(
        change_at(root, Mode::Encrypted, &NoVault),
        Err(Failure::AlreadyRunning)
    ));
    drop(files);
    let vault = MemoryVault::default();
    for _ in 0..2 {
        change_at(root, Mode::Encrypted, &vault).unwrap();
        let files = prepare_at(root, &vault).unwrap();
        verify(&files, true);
        assert!(
            Connection::open(&files.path)
                .unwrap()
                .query_row("SELECT count(*) FROM content", [], |r| r.get::<_, i64>(0))
                .is_err()
        );
        drop(files);
        change_at(root, Mode::Unencrypted, &vault).unwrap();
        let files = prepare_at(root, &NoVault).unwrap();
        verify(&files, false);
        let state = read(root).unwrap();
        for slot in [Slot::A, Slot::B] {
            assert_eq!(
                Status { slot, ..state }.directory(root).exists(),
                slot == state.slot
            );
        }
        assert!(!root.join("session.db").exists());
        assert!(!root.join("archive.db").exists());
        drop(files);
    }
}

#[test]
fn encrypted_legacy_profile_and_failed_export_keep_the_active_data_and_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = MemoryVault::default();
    let files = prepare_at(root, &vault).unwrap();
    populate(&files);
    let archive = files.path.with_file_name("archive.db");
    drop(files);
    assert!(read(root).unwrap().configured);
    assert_eq!(read(root).unwrap().mode, Mode::Encrypted);
    let archive_bytes = fs::read(&archive).unwrap();
    fs::write(&archive, b"not a database").unwrap();
    assert_eq!(
        change_at(root, Mode::Unencrypted, &vault).err(),
        Some(Failure::StorageConversion)
    );
    assert!(!root.join("storage.json").exists());
    assert!(!root.join("data-a").exists());
    fs::write(archive, archive_bytes).unwrap();
    let files = prepare_at(root, &vault).unwrap();
    verify(&files, true);
    drop(files);
    change_at(root, Mode::Unencrypted, &vault).unwrap();
    verify(&prepare_at(root, &NoVault).unwrap(), false);
}

#[test]
fn interrupted_export_is_discarded_and_committed_export_is_retained_on_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = MemoryVault::default();
    let files = prepare_at(root, &vault).unwrap();
    populate(&files);
    drop(files);
    let old = read(root).unwrap();
    let next = Status {
        mode: Mode::Unencrypted,
        slot: Slot::A,
        configured: true,
    };
    let master = master_key(true, &vault).unwrap();
    for committed in [false, true] {
        let dest = next.directory(root);
        fs::create_dir(&dest).unwrap();
        for (name, context) in [
            ("session.db", b"session-v1".as_slice()),
            ("archive.db", b"archive-v1".as_slice()),
        ] {
            if !committed && name == "archive.db" {
                continue;
            }
            export(
                &old.directory(root).join(name),
                &dest.join(name),
                &database_key(&master, context).unwrap(),
                "",
            )
            .unwrap();
        }
        if committed {
            publish(root, next).unwrap();
        }
        let files = if committed {
            prepare_at(root, &NoVault)
        } else {
            prepare_at(root, &vault)
        }
        .unwrap();
        verify(&files, !committed);
        assert_eq!(dest.exists(), committed);
        if committed {
            assert!(!root.join("session.db").exists());
        }
        drop(files);
    }
}

#[test]
fn an_empty_profile_can_change_its_choice_without_accessing_a_key_store() {
    let temp = tempfile::tempdir().unwrap();
    configure_at(temp.path(), Mode::Unencrypted).unwrap();
    change_at(temp.path(), Mode::Encrypted, &NoVault).unwrap();
    change_at(temp.path(), Mode::Unencrypted, &NoVault).unwrap();
    assert_eq!(read(temp.path()).unwrap().mode, Mode::Unencrypted);
    assert!(!databases_exist(temp.path()).unwrap());
    fs::write(
        temp.path().join("session.db-wal"),
        b"synthetic orphan journal",
    )
    .unwrap();
    assert_eq!(
        change_at(temp.path(), Mode::Encrypted, &MemoryVault::default()).err(),
        Some(Failure::StorageConversion)
    );
    assert_eq!(read(temp.path()).unwrap().mode, Mode::Unencrypted);
    assert!(temp.path().join("session.db-wal").exists());
}

#[test]
fn corrupt_manifest_and_missing_key_never_reinterpret_or_replace_existing_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = MemoryVault::default();
    let files = prepare_at(root, &vault).unwrap();
    populate(&files);
    drop(files);
    *vault.0.borrow_mut() = None;
    assert_eq!(
        change_at(root, Mode::Unencrypted, &vault).err(),
        Some(Failure::MissingKey)
    );
    assert!(!root.join("data-a").exists());
    for contents in [
        "broken",
        "{\"version\":2,\"mode\":\"encrypted\",\"slot\":\"legacy\"}",
        "{\"version\":1,\"mode\":\"unencrypted\",\"slot\":\"../other\"}",
    ] {
        fs::write(root.join("storage.json"), contents).unwrap();
        assert!(read(root).is_err());
        assert!(prepare_at(root, &NoVault).is_err());
        assert!(root.join("session.db").exists());
    }
}

#[tokio::test]
async fn real_archive_reopens_with_history_draft_outbox_and_cached_media_after_conversion() {
    use crate::archive::{
        Archive, Batch,
        tests::{PN, record},
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let vault = MemoryVault::default();
    let files = prepare_at(root, &vault).unwrap();
    let db = Archive::open(files.clone(), &files.archive_key_sql).unwrap();
    db.bind_account(PN.into(), "Synthetic account".into())
        .await
        .unwrap();
    db.ingest(Batch {
        messages: vec![record("incoming")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.queue(chat, "outgoing".into(), "Synthetic unsent text".into())
        .await
        .unwrap();
    db.draft(chat, "Draft preserved 👋\nnext line".into())
        .await
        .unwrap();
    db.call(|c| {
        c.execute("INSERT INTO message_photos(message_id,state,original) VALUES((SELECT id FROM messages WHERE wire_id='incoming'),2,?1)", [vec![0_u8, 1, 255]])?;
        Ok(())
    }).await.unwrap();
    drop(db);
    drop(files);
    for mode in [Mode::Unencrypted, Mode::Encrypted] {
        change_at(root, mode, &vault).unwrap();
        let files = if mode == Mode::Encrypted {
            prepare_at(root, &vault)
        } else {
            prepare_at(root, &NoVault)
        }
        .unwrap();
        let db = Archive::open(files.clone(), &files.archive_key_sql).unwrap();
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap();
        let conversation = view.conversation.unwrap();
        assert_eq!(conversation.draft, "Draft preserved 👋\nnext line");
        assert_eq!(conversation.messages.len(), 2);
        db.call(|c| {
            let bytes: Vec<u8> =
                c.query_row("SELECT original FROM message_photos", [], |r| r.get(0))?;
            assert_eq!(bytes, [0, 1, 255]);
            let queued: i64 = c.query_row(
                "SELECT count(*) FROM messages WHERE wire_id='outgoing' AND delivery='queued'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(queued, 1);
            Ok(())
        })
        .await
        .unwrap();
        drop(db);
        drop(files);
    }
}
