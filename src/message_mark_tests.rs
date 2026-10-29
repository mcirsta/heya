use super::*;
use crate::{
    archive::tests::{KEY, LID, PN, fixture, record},
    message_content::Notice,
};
use whatsapp_rust::{prelude::wa, waproto::codec};

async fn message(db: &Archive, chat: i32) -> domain::Message {
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .clone()
}

#[tokio::test]
async fn queued_mark_controls_change_flags_only_after_confirmation_and_survive_reopen() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![record("source")],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = message(&db, 1).await.id;
    db.queue_star(1, id, "star".into(), true).await.unwrap();
    let pending = message(&db, 1).await;
    assert!(!pending.content.metadata.starred);
    assert_eq!(pending.content.metadata.pending, "Star pending");
    assert!(matches!(
        db.queue_star(1, id, "duplicate".into(), true).await,
        Err(Error::Invalid)
    ));
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(job.control.unwrap().kind, ControlKind::Star);
    db.delivery(PN.into(), "star".into(), "failed")
        .await
        .unwrap();
    assert!(!message(&db, 1).await.content.metadata.starred);
    db.queue_star(1, id, "retry".into(), true).await.unwrap();
    db.delivery(PN.into(), "retry".into(), "accepted")
        .await
        .unwrap();
    assert!(message(&db, 1).await.content.metadata.starred);
    db.queue_pin(1, id, "pin".into(), 604800).await.unwrap();
    assert!(!message(&db, 1).await.content.metadata.pinned);
    db.delivery(PN.into(), "pin".into(), "accepted")
        .await
        .unwrap();
    assert!(message(&db, 1).await.content.metadata.pinned);
    assert_eq!(message(&db, 1).await.text, "Synthetic source");
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let saved = message(&db, 1).await;
    assert!(saved.content.metadata.starred && saved.content.metadata.pinned);
    db.queue_star(1, id, "unstar".into(), false).await.unwrap();
    db.queue_pin(1, id, "unpin".into(), 0).await.unwrap();
    db.delivery(PN.into(), "unstar".into(), "accepted")
        .await
        .unwrap();
    db.delivery(PN.into(), "unpin".into(), "accepted")
        .await
        .unwrap();
    let saved = message(&db, 1).await;
    assert!(!saved.content.metadata.starred && !saved.content.metadata.pinned);
    assert!(db.queue_pin(1, id, "invalid".into(), 123).await.is_err());
    assert!(db.queue_star(1, 999, "missing".into(), true).await.is_err());
}

#[tokio::test]
async fn remote_marks_are_ordered_by_timestamp_scoped_to_author_and_follow_alias_merges() {
    let (_temp, db) = fixture();
    let mut photo = record("source");
    photo.chat = LID.into();
    photo.sender = LID.into();
    db.ingest(Batch {
        messages: vec![photo],
        ..Default::default()
    })
    .await
    .unwrap();
    let update = |stamp, starred| MarkUpdate {
        jid: LID.into(),
        sender: LID.into(),
        wire: "source".into(),
        starred: Some(starred),
        pinned_until: None,
        stamp,
    };
    db.save_message_marks(vec![
        update(Some(200), true),
        update(Some(100), false),
        update(None, false),
    ])
    .await
    .unwrap();
    assert!(message(&db, 1).await.content.metadata.starred);
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(message(&db, 1).await.content.metadata.starred);
    db.save_message_marks(vec![MarkUpdate {
        jid: PN.into(),
        sender: PN.into(),
        wire: "source".into(),
        starred: Some(false),
        pinned_until: Some(chrono::Utc::now().timestamp() + 10),
        stamp: Some(300),
    }])
    .await
    .unwrap();
    let current = message(&db, 1).await;
    assert!(!current.content.metadata.starred && current.content.metadata.pinned);
    db.call(|db| {
        db.execute("UPDATE message_marks SET pinned_until=unixepoch()-1", [])?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(!message(&db, 1).await.content.metadata.pinned);
    let mut other = record("source");
    other.chat = "120363000000001@g.us".into();
    other.sender = "15550000002@s.whatsapp.net".into();
    db.ingest(Batch {
        messages: vec![other],
        ..Default::default()
    })
    .await
    .unwrap();
    let group = db
        .call(|db| {
            Ok(db.query_row(
                "SELECT id FROM chats WHERE jid='120363000000001@g.us'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!message(&db, group).await.content.metadata.starred);
}

#[tokio::test]
async fn incoming_pin_and_unpin_envelopes_update_the_original_without_rewriting_it() {
    let (_temp, db) = fixture();
    db.bind_account("15550000000@s.whatsapp.net".into(), "Owner".into())
        .await
        .unwrap();
    db.ingest(Batch {
        messages: vec![record("source")],
        ..Default::default()
    })
    .await
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    for (seconds, expected) in [(604800, true), (0, false)] {
        let mut key = wa::MessageKey::default();
        key.id = Some("source".into());
        key.from_me = Some(true);
        let message =
            crate::protocol_actions::pin(key, seconds, now + if expected { 0 } else { 1 }).unwrap();
        assert_eq!(
            whatsapp_rust::wacore::types::message::EditAttribute::infer_from_message(&message),
            Some(whatsapp_rust::wacore::types::message::EditAttribute::PinInChat)
        );
        let mut notice = record(if expected { "pin-event" } else { "unpin-event" });
        notice.content.notice = Some(if expected {
            Notice::MessagePinned
        } else {
            Notice::MessageUnpinned
        });
        notice.raw = codec::message_to_vec(&message);
        db.ingest(Batch {
            messages: vec![notice],
            ..Default::default()
        })
        .await
        .unwrap();
        let original = db
            .view(1, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .messages
            .iter()
            .find(|m| m.content.metadata.wire == "source")
            .unwrap()
            .clone();
        assert_eq!(original.content.metadata.pinned, expected);
        assert_eq!(original.text, "Synthetic source");
    }
}

#[test]
fn migrating_existing_controls_keeps_old_payloads_and_adds_marker_controls() {
    let (_temp, db) = fixture();
    let files = db.0._files.clone();
    db.blocking(|c| {
        c.execute_batch("DROP TABLE message_marks; DROP TABLE message_controls;")?;
        c.execute_batch(include_str!("archive_controls.sql"))?;
        c.execute_batch("INSERT INTO message_controls VALUES('old-edit','edit','original','#self',1,100,'Keep this edit',200,0,0); PRAGMA user_version=18;")?;
        Ok(())
    }).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.blocking(|c| {
        let control = controls::payload(c, "old-edit")?.unwrap();
        assert_eq!(control.kind, ControlKind::Edit);
        assert_eq!(control.text, "Keep this edit");
        assert_eq!(
            c.pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))?,
            19
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn accepted_marker_commands_get_retained_results_even_if_storage_disappears() {
    use crate::{
        message_actions::Command as Action,
        messaging::{self, Command, Snapshot},
    };
    use tokio::sync::{mpsc, watch};
    let (_archives, archives) = watch::channel(None);
    let (commands, requested) = mpsc::channel(4);
    let (updates, mut views) = watch::channel(Snapshot::empty());
    let task = tokio::spawn(messaging::run(archives, requested, updates));
    for request in 1..=2 {
        let action = if request == 1 {
            Action::Star {
                chat: 1,
                message: 1,
                starred: true,
                request,
            }
        } else {
            Action::Pin {
                chat: 1,
                message: 1,
                seconds: 604800,
                request,
            }
        };
        commands.send(Command::Action(action)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if views
                    .borrow()
                    .operation
                    .as_ref()
                    .is_some_and(|r| r.request == request)
                {
                    break;
                }
                views.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let snapshot = views.borrow_and_update();
        let result = snapshot.operation.as_ref().unwrap();
        assert!(!result.saved && !result.error.is_empty());
        assert_eq!(result.kind, if request == 1 { 8 } else { 9 });
    }
    drop(commands);
    task.await.unwrap();
}

#[tokio::test]
async fn an_offpage_original_keeps_its_exact_pending_operation_status() {
    let (_temp, db) = fixture();
    let mut rows = vec![record("old-photo")];
    for n in 0..150 {
        let mut row = record(&format!("new-{n}"));
        row.timestamp += 1 + i64::from(n);
        rows.push(row);
    }
    db.ingest(Batch {
        messages: rows,
        ..Default::default()
    })
    .await
    .unwrap();
    db.queue_star(1, 1, "offpage-star".into(), true)
        .await
        .unwrap();
    db.blocking(|c| {
        assert!(!controls::pending(c, 1, 100)?.contains_key("old-photo"));
        assert_eq!(
            controls::pending_one(c, 1, PN, "old-photo")?,
            "Star pending"
        );
        assert!(controls::pending_one(c, 1, "other-author", "old-photo")?.is_empty());
        Ok(())
    })
    .unwrap();
    db.delivery(PN.into(), "offpage-star".into(), "failed")
        .await
        .unwrap();
    db.blocking(|c| {
        assert_eq!(controls::pending_one(c, 1, PN, "old-photo")?, "Star failed");
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn explicit_retry_after_uncertain_survives_an_older_operations_late_ack() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![record("source")],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = message(&db, 1).await.id;
    for pin in [false, true] {
        let (old, retry) = if pin {
            ("old-pin", "retry-unpin")
        } else {
            ("old-star", "retry-unstar")
        };
        if pin {
            db.queue_pin(1, id, old.into(), 604800).await.unwrap();
        } else {
            db.queue_star(1, id, old.into(), true).await.unwrap();
        }
        let first = db.claim_next().await.unwrap().unwrap();
        assert_eq!(first.id, old);
        let old_version = first.control.unwrap().version;
        db.delivery(PN.into(), old.into(), "uncertain")
            .await
            .unwrap();
        if pin {
            db.queue_pin(1, id, retry.into(), 0).await.unwrap();
        } else {
            db.queue_star(1, id, retry.into(), false).await.unwrap();
        }
        let next = db.claim_next().await.unwrap().unwrap();
        assert_eq!(
            next.id, retry,
            "uncertain markers must never retry automatically"
        );
        assert!(next.control.unwrap().version > old_version);
        db.delivery(PN.into(), retry.into(), "accepted")
            .await
            .unwrap();
        db.delivery(PN.into(), old.into(), "accepted")
            .await
            .unwrap();
        let saved = message(&db, 1).await;
        assert!(!saved.content.metadata.starred && !saved.content.metadata.pinned);
        assert!(saved.content.metadata.pending.is_empty());
        assert!(db.claim_next().await.unwrap().is_none());
    }
}
