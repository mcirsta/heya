use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture, record};
use crate::storage::{prepare_at, tests::MemoryVault};

async fn incoming(db: &Archive, id: &str, stamp: i64) {
    let mut message = record(id);
    message.timestamp = stamp;
    db.ingest(Batch {
        messages: vec![message],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
}
async fn summary(db: &Archive) -> domain::ChatSummary {
    db.view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .clone()
}
fn read(through: i64) -> ReadUpdate {
    ReadUpdate {
        message: None,
        jid: PN.into(),
        through: Some(through),
        marked: false,
        version: Some(through * 1000),
    }
}

#[tokio::test]
async fn taskbar_counts_chats_before_filters_and_tracks_manual_marks_and_reads() {
    let (_temp, db) = fixture();
    for i in 1..=17 {
        incoming(&db, &format!("message-{i}"), i * 10).await;
    }
    let mut other = record("other");
    other.chat = "15550000002@s.whatsapp.net".into();
    other.sender = other.chat.clone();
    let mut outgoing = record("outgoing");
    outgoing.chat = "15550000003@s.whatsapp.net".into();
    outgoing.outgoing = true;
    db.ingest(Batch {
        messages: vec![other, outgoing],
        chats: vec![ChatMeta {
            jid: PN.into(),
            name: "Alpha".into(),
            ..Default::default()
        }],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let view = db.view(0, 100, "Alpha".into(), false, false).await.unwrap();
    assert_eq!(view.chats.len(), 1);
    assert_eq!(view.chats[0].unread, 17);
    assert_eq!(view.unread_chats, 2, "17 messages in one chat count once");
    db.read_updates(vec![ReadUpdate {
        message: None,
        jid: "15550000003@s.whatsapp.net".into(),
        through: None,
        marked: true,
        version: Some(1),
    }])
    .await
    .unwrap();
    let hidden = db
        .view(0, 100, "no matching chats".into(), true, false)
        .await
        .unwrap();
    assert!(hidden.chats.is_empty());
    assert_eq!(
        hidden.unread_chats, 3,
        "search/unread filters never change the badge"
    );
    let after_read = db
        .view(view.chats[0].id, 100, String::new(), false, true)
        .await
        .unwrap();
    assert_eq!(after_read.unread_chats, 2);
    db.read_updates(vec![
        ReadUpdate {
            message: None,
            jid: "15550000003@s.whatsapp.net".into(),
            through: None,
            marked: false,
            version: Some(2),
        },
        ReadUpdate {
            jid: "15550000002@s.whatsapp.net".into(),
            ..read(1_700_000_000)
        },
    ])
    .await
    .unwrap();
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .unread_chats,
        0
    );
}

#[tokio::test]
async fn phone_read_repairs_seventeen_stale_messages_without_clearing_new_arrivals() {
    let (_temp, db) = fixture();
    for i in 1..=17 {
        incoming(&db, &format!("message-{i}"), i * 10).await;
    }
    assert_eq!(summary(&db).await.unread, 17);
    db.read_updates(vec![read(170)]).await.unwrap();
    assert_eq!(summary(&db).await.unread, 0);
    incoming(&db, "new", 180).await;
    incoming(&db, "late-offline", 100).await;
    db.read_updates(vec![read(170), read(100)]).await.unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    db.read_receipts(PN.into(), vec!["new".into()])
        .await
        .unwrap();
    assert_eq!(summary(&db).await.unread, 0);
    assert!(
        db.view(0, 100, String::new(), true, false)
            .await
            .unwrap()
            .chats
            .is_empty()
    );
}

#[tokio::test]
async fn receipts_preserve_same_second_arrivals_and_wait_for_missing_messages() {
    let (_temp, db) = fixture();
    incoming(&db, "a", 100).await;
    let chat = summary(&db).await.id;
    db.view(chat, 100, String::new(), false, true)
        .await
        .unwrap();
    incoming(&db, "b", 100).await;
    incoming(&db, "c", 100).await;
    db.read_receipts(PN.into(), vec!["a".into(), "missing".into()])
        .await
        .unwrap();
    assert_eq!(summary(&db).await.unread, 2);
    db.read_receipts(PN.into(), vec!["b".into()]).await.unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    incoming(&db, "later", 300).await;
    incoming(&db, "missing", 200).await;
    assert_eq!(summary(&db).await.unread, 1);
    // Replayed history/live envelopes do not restore an already read message.
    incoming(&db, "c", 100).await;
    assert_eq!(summary(&db).await.unread, 1);
}

#[tokio::test]
async fn history_totals_are_authoritative_only_within_their_range_and_do_not_resurrect_reads() {
    let (_temp, db) = fixture();
    for i in 1..=3 {
        incoming(&db, &format!("m{i}"), i * 100).await;
    }
    db.history_unread(PN.into(), 200, 0, None).unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    db.history_unread(PN.into(), 200, 2, None).unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    db.history_unread(PN.into(), 300, 3, None).unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    db.read_updates(vec![read(400)]).await.unwrap();
    incoming(&db, "late", 350).await;
    db.history_unread(PN.into(), 400, 4, None).unwrap();
    assert_eq!(summary(&db).await.unread, 0);

    let (_temp2, partial) = fixture();
    incoming(&partial, "last", 100).await;
    partial.history_unread(PN.into(), 100, 7, None).unwrap();
    assert_eq!(
        summary(&partial).await.unread,
        7,
        "history need not contain every unread body"
    );
    partial.history_unread(PN.into(), 100, 7, None).unwrap();
    assert_eq!(summary(&partial).await.unread, 7);
    partial
        .read_receipts(PN.into(), vec!["last".into()])
        .await
        .unwrap();
    assert_eq!(summary(&partial).await.unread, 0);
}

#[tokio::test]
async fn marked_unread_is_a_dot_not_a_fabricated_message_and_actions_keep_their_order() {
    let (_temp, db) = fixture();
    incoming(&db, "a", 100).await;
    let mut update = read(100);
    update.coalesce(ReadUpdate {
        message: None,
        jid: PN.into(),
        through: None,
        marked: true,
        version: Some(200_000),
    });
    db.read_updates(vec![update]).await.unwrap();
    let state = summary(&db).await;
    assert_eq!(state.unread, 0);
    assert!(state.marked_unread);
    assert_eq!(
        db.view(0, 100, String::new(), true, false)
            .await
            .unwrap()
            .chats
            .len(),
        1
    );
    db.read_updates(vec![read(100)]).await.unwrap();
    assert!(
        summary(&db).await.marked_unread,
        "stale snapshot cannot undo a newer mark-unread"
    );
    db.read_updates(vec![ReadUpdate {
        message: None,
        jid: PN.into(),
        through: None,
        marked: false,
        version: Some(300_000),
    }])
    .await
    .unwrap();
    assert!(!summary(&db).await.marked_unread);
}

#[tokio::test]
async fn privacy_alias_merge_preserves_reads_deduplicates_counts_and_resolves_early_receipts() {
    let (_temp, db) = fixture();
    incoming(&db, "a", 100).await;
    let mut alternate = record("a");
    alternate.chat = LID.into();
    alternate.sender = LID.into();
    alternate.timestamp = 100;
    db.ingest(Batch {
        messages: vec![alternate],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.read_updates(vec![ReadUpdate {
        message: None,
        jid: LID.into(),
        ..read(100)
    }])
    .await
    .unwrap();
    db.read_receipts(LID.into(), vec!["b".into()])
        .await
        .unwrap();
    incoming(&db, "b", 200).await;
    incoming(&db, "c", 300).await;
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert_eq!(view.chats.len(), 1);
    assert_eq!(view.chats[0].unread, 1);
    db.read_receipts(LID.into(), vec!["c".into()])
        .await
        .unwrap();
    assert_eq!(summary(&db).await.unread, 0);
}

#[tokio::test]
async fn group_alias_deduplication_counts_one_message_and_keeps_explicit_read_evidence() {
    let (_temp, db) = fixture();
    let group = "15550000001-42@g.us";
    let mut message = record("same-group-message");
    message.chat = group.into();
    message.sender = LID.into();
    message.timestamp = 100;
    db.ingest(Batch {
        messages: vec![message.clone()],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    message.sender = PN.into();
    // History overlap is not proof that the live copy was read.
    db.ingest(Batch {
        messages: vec![message],
        ..Default::default()
    })
    .await
    .unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(summary(&db).await.unread, 1);
    db.read_receipts(group.into(), vec!["same-group-message".into()])
        .await
        .unwrap();
    assert_eq!(summary(&db).await.unread, 0);
    // A later history total cannot resurrect explicitly read message identities.
    db.history_unread(group.into(), 200, 1, None).unwrap();
    assert_eq!(summary(&db).await.unread, 0);
}

#[tokio::test]
async fn schema_three_upgrade_and_reopen_preserve_drafts_and_request_read_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let old = Connection::open(files.path.with_file_name("archive.db")).unwrap();
    old.execute_batch(KEY).unwrap();
    old.execute_batch(include_str!("archive_schema.sql"))
        .unwrap();
    old.execute_batch(include_str!("archive_contacts.sql"))
        .unwrap();
    old.execute_batch(include_str!("archive_avatars.sql"))
        .unwrap();
    old.execute_batch("PRAGMA user_version=3; INSERT INTO chats(jid,unread,draft) VALUES('15550000001@s.whatsapp.net',17,'Keep draft');").unwrap();
    drop(old);
    let db = Archive::open(files.clone(), KEY).unwrap();
    assert_eq!(summary(&db).await.unread, 17);
    assert!(db.begin_reads().await.unwrap());
    db.read_updates(vec![read(200)]).await.unwrap();
    db.read_receipts(PN.into(), vec!["early".into()])
        .await
        .unwrap();
    db.finish_reads().await.unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    assert!(!db.begin_reads().await.unwrap());
    incoming(&db, "early", 300).await;
    assert_eq!(summary(&db).await.unread, 0);
    let chat = summary(&db).await.id;
    assert_eq!(
        db.view(chat, 10, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .draft,
        "Keep draft"
    );
    db.blocking(|c| {c.execute_batch("CREATE TRIGGER fail_read BEFORE UPDATE OF read_stamp ON chats BEGIN SELECT RAISE(ABORT,'synthetic disk full'); END;")?;Ok(())}).unwrap();
    incoming(&db, "new", 400).await;
    assert!(db.read_updates(vec![read(400)]).await.is_err());
    assert_eq!(summary(&db).await.unread, 1);
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert!(
        db.begin_reads().await.unwrap(),
        "unclean session must replay app state"
    );
}

#[tokio::test]
async fn deleted_tail_can_be_read_without_a_sender_receipt() {
    for jid in [PN, "15550000001-42@g.us"] {
        let (_temp, db) = fixture();
        let mut original = record("deleted-tail");
        original.chat = jid.into();
        original.timestamp = 100;
        let mut revoke = original.clone();
        revoke.id = "delete-event".into();
        revoke.timestamp = 200;
        revoke.visible = false;
        revoke.change = Some(Change {
            id: original.id.clone(),
            sender: original.sender.clone(),
            text: None,
            version: 200,
            raw: Vec::new(),
        });
        db.ingest(Batch {
            messages: vec![original, revoke],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let chat = summary(&db).await.id;
        let view = db
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(view.chat.unread, 1);
        let tail = view.messages.last().unwrap();
        assert_eq!(tail.text, "This message was deleted");
        db.mark_visible_read_with_receipts(chat, tail.id, true)
            .await
            .unwrap();
        assert_eq!(
            summary(&db).await.unread,
            0,
            "viewing a deleted tail must clear its unread badge"
        );
        let sync = db.next_read_sync().await.unwrap().unwrap();
        assert_eq!(
            sync.through, 100,
            "read the observed message, not the later deletion event"
        );
        let key = sync.message.unwrap();
        assert_eq!(key.wire, "deleted-tail");
        assert_eq!(key.sender, PN);
        assert!(!key.outgoing);
        assert!(
            db.next_receipt().await.unwrap().is_none(),
            "a deleted message must not emit a sender-facing receipt"
        );
        assert!(
            db.view(0, 100, String::new(), true, false)
                .await
                .unwrap()
                .chats
                .is_empty()
        );
    }
}

#[tokio::test]
async fn deleted_tail_read_preserves_newer_same_second_arrivals() {
    let (_temp, db) = fixture();
    incoming(&db, "earlier", 90).await;
    incoming(&db, "deleted-tail", 100).await;
    let chat = summary(&db).await.id;
    let mut revoke = record("delete-event");
    revoke.timestamp = 200;
    revoke.visible = false;
    revoke.change = Some(Change {
        id: "deleted-tail".into(),
        sender: PN.into(),
        text: None,
        version: 200,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![revoke],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    let observed = view.messages.last().unwrap().id;
    incoming(&db, "not-yet-seen", 100).await;
    db.mark_visible_read_with_receipts(chat, observed, true)
        .await
        .unwrap();
    assert_eq!(
        summary(&db).await.unread,
        1,
        "clear only the observed range, including its deletion placeholder"
    );
    let sync = db.next_read_sync().await.unwrap().unwrap();
    assert_eq!(sync.message.unwrap().wire, "deleted-tail");
    let receipt = db.next_receipt().await.unwrap().unwrap();
    assert_eq!(
        receipt
            .messages
            .iter()
            .map(|(_, wire)| wire.as_str())
            .collect::<Vec<_>>(),
        vec!["earlier"]
    );
}

#[tokio::test]
async fn deleted_tail_from_expiry_is_a_readable_placeholder() {
    let (_temp, db) = fixture();
    let mut message = record("expired-tail");
    message.timestamp = 100;
    message.expires = Some(chrono::Utc::now().timestamp() - 1);
    db.ingest(Batch {
        messages: vec![message],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = summary(&db).await.id;
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap();
    assert_eq!(view.chat.unread, 1);
    let tail = view.messages.last().unwrap();
    assert_eq!(tail.text, "[Disappearing message expired]");
    db.mark_visible_read_with_receipts(chat, tail.id, true)
        .await
        .unwrap();
    assert_eq!(
        summary(&db).await.unread,
        0,
        "a visible expiry placeholder must not trap the unread badge"
    );
    assert_eq!(db.next_read_sync().await.unwrap().unwrap().through, 100);
    assert!(db.next_receipt().await.unwrap().is_none());
}
