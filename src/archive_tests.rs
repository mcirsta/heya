use super::*;
use crate::storage::{prepare_at, tests::MemoryVault};

pub(crate) const PN: &str = "15550000001@s.whatsapp.net";
pub(crate) const LID: &str = "10000000001@lid";
pub(crate) const KEY: &str =
    "PRAGMA key=\"x'2929292929292929292929292929292929292929292929292929292929292929'\";";
pub(crate) fn fixture() -> (tempfile::TempDir, Archive) {
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    let db = Archive::open(files, KEY).unwrap();
    (temp, db)
}
pub(crate) fn record(id: &str) -> Record {
    Record {
        chat: PN.into(),
        sender: PN.into(),
        id: id.into(),
        timestamp: 1_700_000_000,
        outgoing: false,
        sender_name: "Synthetic contact".into(),
        text: format!("Synthetic {id}"),
        raw: vec![1, 2, 3],
        content: Default::default(),
        album: None,
        thumbnail: Vec::new(),
        visible: true,
        expires: None,
        change: None,
    }
}
/// These repair tests compare content payloads. Wire/time/outbox metadata is a
/// transient archive projection, covered separately by message-control tests.
pub(crate) fn assert_content_payload(
    actual: &crate::message_content::Content,
    expected: &crate::message_content::Content,
) {
    let mut payload = actual.clone();
    payload.metadata = Default::default();
    assert_eq!(&payload, expected);
}
fn batch(messages: Vec<Record>) -> Batch {
    Batch {
        messages,
        live: true,
        ..Default::default()
    }
}
async fn conversation(db: &Archive, chat: i32, limit: usize) -> domain::Conversation {
    db.view(chat, limit, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}
fn status(db: &Archive, id: &str) -> String {
    db.blocking(|c| {
        Ok(c.query_row(
            "SELECT delivery FROM messages WHERE wire_id=?1",
            [id],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

#[tokio::test]
async fn view_keeps_local_dates_separate_from_clock_and_transcript() {
    let (_temp, db) = fixture();
    let a = record("dated");
    let mut b = record("unknown-time");
    b.timestamp = 0;
    db.ingest(batch(vec![a.clone(), b])).await.unwrap();
    let view = conversation(&db, 1, 100).await;
    let when = crate::message_time::local(a.timestamp).unwrap();
    let message = view.messages.last().unwrap();
    assert_eq!(message.date, Some(when.date_naive()));
    assert_eq!(message.time, when.format("%H:%M").to_string());
    assert!(
        view.transcript
            .contains(&when.format("%Y-%m-%d %H:%M").to_string())
    );
    assert_eq!(view.messages[0].date, None);
    assert!(view.messages[0].time.is_empty());
}

#[tokio::test]
async fn contacts_migrate_reopen_and_require_repair_after_unclean_exit() {
    let temp = tempfile::tempdir().unwrap();
    let files = prepare_at(temp.path(), &MemoryVault::default()).unwrap();
    // Construct the previous released schema, including a chat and its draft.
    let path = files.path.with_file_name("archive.db");
    let old = Connection::open(&path).unwrap();
    old.execute_batch(KEY).unwrap();
    old.execute_batch(include_str!("archive_schema.sql"))
        .unwrap();
    old.execute_batch("PRAGMA user_version=1; INSERT INTO chats(jid,name,draft) VALUES('15550000001@s.whatsapp.net','Historical name','Keep this draft');").unwrap();
    drop(old);
    let db = Archive::open(files.clone(), KEY).unwrap();
    assert!(db.begin_contacts().await.unwrap());
    db.save_contacts(
        vec![Contact {
            jid: PN.into(),
            name: "Saved name".into(),
            timestamp: Some(42),
        }],
        vec![],
        false,
    )
    .await
    .unwrap();
    db.finish_contacts().await.unwrap();
    drop(db);
    let db = Archive::open(files.clone(), KEY).unwrap();
    assert!(
        !db.begin_contacts().await.unwrap(),
        "clean shutdown needs no full contact replay"
    );
    let view = conversation(&db, 1, 10).await;
    assert_eq!(view.chat.title, "Saved name");
    assert_eq!(view.draft, "Keep this draft");
    drop(db); // Simulated crash before marking a clean SDK/contact shutdown.
    let db = Archive::open(files, KEY).unwrap();
    assert!(db.begin_contacts().await.unwrap());
    assert_eq!(conversation(&db, 1, 10).await.chat.title, "Saved name");
    db.finish_contacts().await.unwrap();
    db.set_needs_link(true).await.unwrap();
    assert!(
        db.begin_contacts().await.unwrap(),
        "relink must refresh phonebook too"
    );
}

#[tokio::test]
async fn multiline_sidebar_previews_preserve_full_archived_messages_and_drafts() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let mut message = record("multiline");
    message.text = "Synthetic 👩🏽‍💻\r\n\nمرحبا\u{2028}こんにちは\n".repeat(1000);
    db.ingest(batch(vec![message.clone()])).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats[0].preview,
        domain::single_line_summary(&message.text)
    );
    let before = view.chats.clone();
    assert_eq!(view.conversation.unwrap().messages[0].text, message.text);
    let draft = "Draft first line\nSecond line\n".repeat(1000);
    db.draft(chat, draft.clone()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats, before,
        "autosave must not invalidate the open chat's sidebar row"
    );
    let current = view.conversation.unwrap();
    assert_eq!(current.draft, draft);
    assert_eq!(current.messages[0].text, message.text);

    let other = db
        .create_chat("15550000002@s.whatsapp.net".into())
        .await
        .unwrap();
    let view = db
        .view(other, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats.iter().find(|c| c.id == chat).unwrap().preview,
        domain::single_line_summary(&format!("Draft: {draft}"))
    );
    // Reopening restores the full saved text independently of preview visibility.
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let reopened = conversation(&db, chat, 100).await;
    assert_eq!(reopened.draft, draft);
    assert_eq!(
        reopened.chat.preview,
        domain::single_line_summary(&message.text)
    );

    // Even a chat with no messages shows no draft while open. Switching between
    // two drafted chats must move the preview without moving or losing the text.
    db.draft(other, "Second 👋\nDraft".into()).await.unwrap();
    let view = db
        .view(other, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(view.conversation.unwrap().chat.preview.is_empty());
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats.iter().find(|c| c.id == other).unwrap().preview,
        "Draft: Second 👋 Draft"
    );
    db.draft(chat, String::new()).await.unwrap();
    let view = db
        .view(other, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.chats.iter().find(|c| c.id == chat).unwrap().preview,
        domain::single_line_summary(&message.text)
    );
}

#[tokio::test]
async fn photo_sidebar_uses_metadata_and_current_group_sender_without_changing_content() {
    let (temp, db) = fixture();
    let mut photo = record("group-photo");
    photo.chat = "10000000000-1@g.us".into();
    photo.sender = LID.into();
    photo.sender_name = "Fallback Profile".into();
    let caption = "First caption 👩🏽‍💻\r\nSecond paragraph, retained in full.";
    photo.text = format!("[Photo] {caption}");
    photo.content = crate::message_content::Content {
        body: Some(caption.into()),
        photo: Some(crate::message_content::Photo {
            width: 640,
            height: 480,
            size: Some(42),
        }),
        ..Default::default()
    };
    let mut direct = photo.clone();
    direct.id = "direct-photo".into();
    direct.chat = PN.into();
    db.ingest(batch(vec![photo.clone(), direct])).await.unwrap();
    let group = db
        .view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats
        .iter()
        .find(|c| c.preview.starts_with('~'))
        .unwrap()
        .id;
    let view = db
        .view(group, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(
        view.conversation.as_ref().unwrap().chat.preview,
        "~Fallback: Photo: First caption 👩🏽‍💻"
    );
    assert_eq!(
        view.chats.iter().find(|c| c.id != group).unwrap().preview,
        "Photo: First caption 👩🏽‍💻"
    );
    let message = &view.conversation.unwrap().messages[0];
    assert_eq!(message.sender, "~Fallback Profile");
    assert_eq!(message.text, photo.text);
    assert_eq!(message.content.body.as_deref(), Some(caption));

    // Profile sync, PN/LID merging, saved-name changes and removal all update
    // the preview without rewriting message bodies or requiring a new message.
    db.ingest(Batch {
        profiles: vec![(LID.into(), "Sasha Profile".into())],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        conversation(&db, group, 100)
            .await
            .chat
            .preview
            .starts_with("~Sasha: Photo:")
    );
    assert_eq!(
        conversation(&db, group, 100).await.messages[0].sender,
        "~Sasha Profile"
    );
    db.save_contacts(
        vec![Contact {
            jid: PN.into(),
            name: "Saved Contact".into(),
            timestamp: Some(1),
        }],
        vec![(LID.into(), PN.into())],
        false,
    )
    .await
    .unwrap();
    assert!(
        conversation(&db, group, 100)
            .await
            .chat
            .preview
            .starts_with("Saved: Photo:")
    );
    assert_eq!(
        conversation(&db, group, 100).await.messages[0].sender,
        "Saved Contact"
    );
    db.save_contacts(
        vec![Contact {
            jid: PN.into(),
            name: "".into(),
            timestamp: Some(2),
        }],
        vec![],
        false,
    )
    .await
    .unwrap();
    assert!(
        conversation(&db, group, 100)
            .await
            .chat
            .preview
            .starts_with("~Sasha: Photo:")
    );
    assert_eq!(
        conversation(&db, group, 100).await.messages[0].sender,
        "~Sasha Profile"
    );
    db.draft(group, "Keep this draft\nWith its second line".into())
        .await
        .unwrap();
    assert_eq!(
        conversation(&db, group, 100).await.chat.preview,
        "~Sasha: Photo: First caption 👩🏽‍💻"
    );
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .iter()
            .find(|c| c.id == group)
            .unwrap()
            .preview,
        "Draft: Keep this draft With its second line"
    );
    db.draft(group, String::new()).await.unwrap();

    let mut sent = photo.clone();
    sent.id = "own-photo".into();
    sent.sender = PN.into();
    sent.outgoing = true;
    sent.timestamp += 1;
    sent.text = "[Photo]".into();
    sent.content.body = Some(String::new());
    db.ingest(batch(vec![sent.clone()])).await.unwrap();
    assert_eq!(conversation(&db, group, 100).await.chat.preview, "Photo");
    let mut hidden = sent.clone();
    hidden.id = "hidden-event".into();
    hidden.timestamp += 1;
    hidden.visible = false;
    hidden.text = "Must not become the preview".into();
    db.ingest(batch(vec![hidden])).await.unwrap();
    assert_eq!(conversation(&db, group, 100).await.chat.preview, "Photo");

    let mut revoked = sent.clone();
    revoked.id = "revoke-event".into();
    revoked.visible = false;
    revoked.change = Some(Change {
        id: sent.id,
        sender: "#self".into(),
        text: None,
        version: sent.timestamp + 1,
        raw: Vec::new(),
    });
    db.ingest(batch(vec![revoked])).await.unwrap();
    assert_eq!(
        conversation(&db, group, 100).await.chat.preview,
        "This message was deleted"
    );
    let mut literal = photo;
    literal.id = "literal-text".into();
    literal.timestamp += 10;
    literal.text = "[Photo] is ordinary text here".into();
    literal.content = Default::default();
    db.ingest(batch(vec![literal])).await.unwrap();
    assert_eq!(
        conversation(&db, group, 100).await.chat.preview,
        "~Sasha: [Photo] is ordinary text here"
    );

    // Latest-message lookup stays indexed, not a full-history scan per chat.
    let plan = db
        .blocking(|c| {
            let mut statement = c.prepare(&format!(
                "EXPLAIN QUERY PLAN {}",
                include_str!("archive_sidebar.sql")
            ))?;
            Ok(statement
                .query_map(params![257, chrono::Utc::now().timestamp_millis()], |r| {
                    r.get::<_, String>(3)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap();
    assert!(
        plan.iter()
            .any(|line| line.starts_with("SEARCH latest USING") && line.contains("chat_id=?"))
    );
    assert!(!plan.iter().any(|line| line.contains("SCAN latest")));
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(
        conversation(&db, group, 100).await.chat.preview,
        "~Sasha: [Photo] is ordinary text here"
    );
    drop(db);
    drop(temp);
}

#[tokio::test]
async fn archive_encrypts_with_its_own_key_and_rejects_another_account() {
    let (temp, db) = fixture();
    db.bind_account(PN.into(), String::new()).await.unwrap();
    assert_eq!(
        db.bind_account("15550000002@s.whatsapp.net".into(), String::new())
            .await,
        Err(Error::Identity)
    );
    db.ingest(batch(vec![record("one")])).await.unwrap();
    let path = temp.path().join("archive.db");
    let files = db.0._files.clone();
    drop(db);
    assert_ne!(&std::fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
    let plain = Connection::open(&path).unwrap();
    assert!(
        plain
            .query_row::<i64, _, _>("SELECT count(*) FROM messages", [], |r| r.get(0))
            .is_err()
    );
    drop(plain);
    assert!(matches!(
        Archive::open(files.clone(), &files.key_sql),
        Err(Error::Database)
    ));
    let db = Archive::open(files, KEY).unwrap();
    db.bind_account(PN.into(), String::new()).await.unwrap();
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert_eq!(view.chats.len(), 1);
    assert_eq!(view.chats[0].preview, "Synthetic one");
}

#[tokio::test]
async fn self_chat_uses_account_profile_across_aliases_renames_and_offline_reopen() {
    let (_temp, db) = fixture();
    let other = "15550000002@s.whatsapp.net";
    let self_chat = db.create_chat(LID.into()).await.unwrap();
    let other_chat = db.create_chat(other.into()).await.unwrap();
    db.bind_account(PN.into(), "Ada 🌿 Example".into())
        .await
        .unwrap();
    db.save_contacts(
        vec![
            Contact {
                jid: PN.into(),
                name: "My local phonebook alias".into(),
                timestamp: Some(1),
            },
            Contact {
                jid: other.into(),
                name: "Other saved contact".into(),
                timestamp: Some(1),
            },
        ],
        vec![(LID.into(), PN.into())],
        false,
    )
    .await
    .unwrap();
    // Historical and live profile hints must not rename the account itself.
    db.ingest(Batch {
        profiles: vec![(PN.into(), "Older push name".into())],
        chats: vec![ChatMeta {
            jid: PN.into(),
            name: "Old history title".into(),
            ..Default::default()
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let own = conversation(&db, self_chat, 10).await;
    assert_eq!(own.chat.title, "Ada 🌿 Example (You)");
    assert_eq!(
        own.chat.initials, "AY",
        "the suffix contributes Y, not a parenthesis"
    );
    assert_eq!(
        conversation(&db, other_chat, 10).await.chat.title,
        "Other saved contact"
    );
    for query in ["ada", "you"] {
        let view = db
            .view(self_chat, 10, query.into(), false, false)
            .await
            .unwrap();
        assert_eq!(view.chats.len(), 1);
        assert_eq!(view.chats[0].title, own.chat.title);
        assert_eq!(view.conversation.unwrap().chat.title, own.chat.title);
    }
    let changes = db.subscribe();
    db.bind_account(PN.into(), "Ada 🌿 Example".into())
        .await
        .unwrap();
    assert!(
        !changes.has_changed().unwrap(),
        "unchanged profile does not wake the UI on every message"
    );
    db.bind_account(PN.into(), "Renamed owner".into())
        .await
        .unwrap();
    assert!(changes.has_changed().unwrap());
    assert_eq!(
        db.bind_account(other.into(), "Wrong account".into()).await,
        Err(Error::Identity)
    );
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(
        conversation(&db, self_chat, 10).await.chat.title,
        "Renamed owner (You)"
    );
    db.bind_account(PN.into(), "  \n ".into()).await.unwrap();
    assert_eq!(conversation(&db, self_chat, 10).await.chat.title, "You");
    assert_eq!(
        conversation(&db, other_chat, 10).await.chat.title,
        "Other saved contact"
    );
}

#[tokio::test]
async fn replay_pagination_and_transaction_rollback_use_full_message_identity() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let messages: Vec<_> = (0..125).map(|i| record(&format!("id-{i}"))).collect();
    db.ingest(batch(messages.clone())).await.unwrap();
    db.ingest(batch(messages)).await.unwrap();
    let page = conversation(&db, chat, 100).await;
    assert_eq!(page.messages.len(), 100);
    assert_eq!(page.older_count, 25);
    assert_eq!(page.messages[0].text, "Synthetic id-25");
    assert_eq!(page.chat.unread, 125);
    let all = conversation(&db, chat, 200).await;
    assert_eq!(all.messages[25].id, page.messages[0].id);
    assert!(all.messages.windows(2).all(|w| w[0].id < w[1].id));
    let mut bad = record("");
    bad.text = "must not commit".into();
    assert_eq!(
        db.ingest(batch(vec![record("rollback"), bad])).await,
        Err(Error::Invalid)
    );
    assert_eq!(conversation(&db, chat, 200).await.messages.len(), 125);
    let mut other = record("id-0");
    other.chat = "123@g.us".into();
    other.sender = "15550000002@s.whatsapp.net".into();
    db.ingest(batch(vec![other])).await.unwrap();
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .len(),
        2
    );
}

#[tokio::test]
async fn aliases_merge_tombstones_drafts_and_late_group_edits_without_resurrection() {
    let (_temp, db) = fixture();
    let pn = db.create_chat(PN.into()).await.unwrap();
    let lid = db.create_chat(LID.into()).await.unwrap();
    db.draft(pn, "PN draft".into()).await.unwrap();
    db.draft(lid, "LID draft".into()).await.unwrap();
    let base = record("original");
    let mut delete = record("delete");
    delete.chat = LID.into();
    delete.sender = LID.into();
    delete.visible = false;
    delete.change = Some(Change {
        id: "original".into(),
        sender: LID.into(),
        text: None,
        version: 2,
        raw: Vec::new(),
    });
    let mut lid_copy = base.clone();
    lid_copy.chat = LID.into();
    lid_copy.sender = LID.into();
    let mut group = record("group");
    group.chat = "123@g.us".into();
    let mut edit = group.clone();
    edit.id = "edit".into();
    edit.sender = LID.into();
    edit.visible = false;
    edit.change = Some(Change {
        id: "group".into(),
        sender: LID.into(),
        text: Some("Edited group text".into()),
        version: 5,
        raw: Vec::new(),
    });
    db.ingest(batch(vec![base.clone(), lid_copy, delete, group, edit]))
        .await
        .unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    db.ingest(batch(vec![base])).await.unwrap();
    let view = conversation(&db, lid, 100).await;
    assert_eq!(view.chat.id, pn);
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.messages[0].text, "This message was deleted");
    assert!(view.draft.contains("PN draft") && view.draft.contains("LID draft"));
    db.draft(lid, "new draft through old selection".into())
        .await
        .unwrap();
    assert_eq!(
        conversation(&db, pn, 100).await.draft,
        "new draft through old selection"
    );
    let group = db.create_chat("123@g.us".into()).await.unwrap();
    assert_eq!(
        conversation(&db, group, 100).await.messages[0].text,
        "Edited group text"
    );
    assert_eq!(
        db.ingest(Batch {
            aliases: vec![(LID.into(), "other@s.whatsapp.net".into())],
            ..Default::default()
        })
        .await,
        Err(Error::Identity)
    );
}

#[tokio::test]
async fn outbox_commit_ack_races_and_restart_never_blindly_resend() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    db.draft(chat, "first".into()).await.unwrap();
    db.queue(chat, "out-1".into(), "first".into())
        .await
        .unwrap();
    assert!(conversation(&db, chat, 100).await.draft.is_empty());
    db.queue(chat, "out-2".into(), "older".into())
        .await
        .unwrap();
    db.draft(chat, "newer".into()).await.unwrap();
    assert_eq!(conversation(&db, chat, 100).await.draft, "newer");
    assert_eq!(db.claim_next().await.unwrap().unwrap().id, "out-1");
    db.delivery(PN.into(), "out-1".into(), "read")
        .await
        .unwrap();
    db.delivery(PN.into(), "out-1".into(), "uncertain")
        .await
        .unwrap();
    assert_eq!(status(&db, "out-1"), "read");
    assert_eq!(db.claim_next().await.unwrap().unwrap().id, "out-2");
    db.queue(chat, "out-3".into(), "not attempted yet".into())
        .await
        .unwrap();
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(status(&db, "out-2"), "uncertain");
    assert_eq!(db.claim_next().await.unwrap().unwrap().id, "out-3");
    assert!(db.claim_next().await.unwrap().is_none());
    db.delivery(PN.into(), "out-2".into(), "accepted")
        .await
        .unwrap();
    db.delivery(PN.into(), "out-2".into(), "failed")
        .await
        .unwrap();
    assert_eq!(status(&db, "out-2"), "accepted");
    let mut echo = record("out-3");
    echo.outgoing = true;
    db.ingest(batch(vec![echo])).await.unwrap();
    assert_eq!(conversation(&db, chat, 100).await.messages.len(), 3);
    assert_eq!(status(&db, "out-3"), "accepted");
}

#[tokio::test]
async fn expiration_purges_content_and_edits_cannot_resurrect_it() {
    let (_temp, db) = fixture();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let mut base = record("expiry");
    base.expires = Some(1);
    db.ingest(batch(vec![base.clone()])).await.unwrap();
    let mut edit = record("edit");
    edit.visible = false;
    edit.change = Some(Change {
        id: "expiry".into(),
        sender: PN.into(),
        text: Some("stale edited text".into()),
        version: 20,
        raw: Vec::new(),
    });
    db.ingest(batch(vec![base, edit])).await.unwrap();
    let view = conversation(&db, chat, 100).await;
    assert_eq!(view.messages[0].text, "[Disappearing message expired]");
    db.blocking(|c| {
        let (raw,body):(i64,Option<String>)=c.query_row("SELECT length(raw),(SELECT body FROM mutations WHERE wire_id='expiry') FROM messages WHERE wire_id='expiry'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert_eq!(raw,0);assert!(body.is_none());Ok(())
    }).unwrap();
    for seconds in [86_400, 0] {
        db.ingest(Batch {
            chats: vec![ChatMeta {
                jid: PN.into(),
                ephemeral: Some(seconds),
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    }
    db.queue(chat, "normal".into(), "no longer disappearing".into())
        .await
        .unwrap();
    assert_eq!(db.claim_next().await.unwrap().unwrap().ephemeral, 0);
}

#[tokio::test]
async fn group_runs_use_canonical_participant_identity_not_display_names() {
    let (_temp, db) = fixture();
    let group = "120000000000001@g.us";
    let mut rows = Vec::new();
    for (index, sender) in [PN, "15550000002@s.whatsapp.net", LID, PN]
        .into_iter()
        .enumerate()
    {
        let mut row = record(&format!("identity-{index}"));
        row.chat = group.into();
        row.sender = sender.into();
        row.sender_name = "Alex".into();
        row.timestamp += index as i64;
        rows.push(row);
    }
    db.ingest(batch(rows)).await.unwrap();
    let chat = db.create_chat(group.into()).await.unwrap();
    let view = conversation(&db, chat, 100).await;
    assert_eq!(view.messages[0].sender, view.messages[1].sender);
    assert_ne!(view.messages[0].sender_id, view.messages[1].sender_id);
    assert!(view.messages[1].starts_run(Some(&view.messages[0])));
    assert!(view.messages[3].starts_run(Some(&view.messages[2])));
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let view = conversation(&db, chat, 100).await;
    assert_eq!(view.messages[2].sender_id, view.messages[3].sender_id);
    assert!(!view.messages[3].starts_run(Some(&view.messages[2])));
}
