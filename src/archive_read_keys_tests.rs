//! Schema recovery uses synthetic metadata only, without an account connection.
use super::*;
use crate::archive::tests::{KEY, PN, fixture, record};

#[tokio::test]
async fn old_delete_keys_recover_only_unique_pairs_and_keep_pending_reads_immutable() {
    for ambiguous in [false, true] {
        let (_temp, db) = fixture();
        let original = record("original");
        let mut revoke = record("delete-envelope");
        revoke.timestamp += 100;
        revoke.visible = false;
        revoke.raw.clear();
        revoke.change = Some(Change {
            id: original.id.clone(),
            sender: PN.into(),
            text: None,
            version: revoke.timestamp * 1000,
            raw: Vec::new(),
        });
        db.ingest(Batch {
            messages: vec![original],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        db.mark_visible_read(1, 1).await.unwrap();
        db.ingest(Batch {
            messages: vec![revoke.clone()],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        if ambiguous {
            revoke.id = "other-empty-event".into();
            revoke.change = None;
            db.ingest(Batch {
                messages: vec![revoke],
                ..Default::default()
            })
            .await
            .unwrap();
        }
        let files = db.0._files.clone();
        db.blocking(|c| {
            c.execute_batch("DROP INDEX messages_read_wire; ALTER TABLE messages DROP COLUMN read_wire; ALTER TABLE outgoing_reads DROP COLUMN keys; PRAGMA user_version=19;")?;
            Ok(())
        })
        .unwrap();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        let old = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(old.messages[0].wire, "original");
        db.blocking(|c| {
            let read: String = c.query_row(
                "SELECT read_wire FROM messages WHERE wire_id='original'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(read, if ambiguous { "" } else { "delete-envelope" });
            Ok(())
        })
        .unwrap();
        db.mark_visible_read(1, 1).await.unwrap();
        let current = db.next_read_sync().await.unwrap().unwrap();
        assert!(current.version > old.version);
        assert_eq!(current.through, 1_700_000_000);
        assert_eq!(
            current.messages[0].wire,
            if ambiguous {
                "original"
            } else {
                "delete-envelope"
            }
        );
        db.finish_read_sync(old).await.unwrap();
        assert!(db.next_read_sync().await.unwrap().unwrap() == current);
    }
}

#[tokio::test]
async fn migration_never_treats_an_expired_edit_as_a_revoke() {
    let (_temp, db) = fixture();
    let original = record("expired-original");
    let mut edit = record("edit-envelope");
    edit.timestamp += 100;
    edit.visible = false;
    edit.raw.clear();
    db.ingest(Batch {
        messages: vec![original, edit],
        ..Default::default()
    })
    .await
    .unwrap();
    let files = db.0._files.clone();
    db.blocking(|c| {
        // An older expiry retained its last edit revision in mutations.
        c.execute_batch("UPDATE messages SET deleted=1,expires=1,body='[Disappearing message expired]' WHERE wire_id='expired-original';
            INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted) SELECT chat_id,sender,wire_id,NULL,1700000100000,1 FROM messages WHERE wire_id='expired-original';
            DROP INDEX messages_read_wire; ALTER TABLE messages DROP COLUMN read_wire; ALTER TABLE outgoing_reads DROP COLUMN keys; PRAGMA user_version=19;")?;
        Ok(())
    }).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.blocking(|c| {
        let read: String = c.query_row(
            "SELECT read_wire FROM messages WHERE wire_id='expired-original'",
            [],
            |r| r.get(0),
        )?;
        assert!(read.is_empty());
        Ok(())
    })
    .unwrap();
}
