use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture, record};
use whatsapp_rust::{
    prelude::{MessageBuilderExt, wa},
    waproto::codec,
};
const OWNER: &str = "15550000000@s.whatsapp.net";
fn own(wire: &str) -> Record {
    let mut r = record(wire);
    r.outgoing = true;
    r.sender = OWNER.into();
    r.timestamp = chrono::Utc::now().timestamp() - 30;
    r.raw = codec::message_to_vec(&wa::Message::text(r.text.clone()));
    r
}
async fn seed(db: &Archive, r: Record) -> (i32, i32) {
    db.bind_account(OWNER.into(), "Owner".into()).await.unwrap();
    db.ingest(Batch {
        messages: vec![r],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    (chat, view.conversation.unwrap().messages[0].id)
}
async fn view(db: &Archive, chat: i32) -> domain::Conversation {
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[test]
fn windows_and_sender_revoke_keys_are_exact() {
    assert!(action::within_window(
        100,
        100 + action::EDIT_WINDOW,
        action::EDIT_WINDOW
    ));
    assert!(!action::within_window(
        100,
        101 + action::EDIT_WINDOW,
        action::EDIT_WINDOW
    ));
    assert!(!action::within_window(101, 100, action::EDIT_WINDOW));
    assert!(!action::within_window(0, 100, action::EDIT_WINDOW));
    for jid in [PN, "120363000000001@g.us"] {
        let m = crate::protocol_actions::revoke(jid, "original-id".into());
        let key = &m.protocol_message.key;
        assert_eq!(key.id.as_deref(), Some("original-id"));
        assert_eq!(key.from_me, Some(true));
        assert_eq!(key.remote_jid.as_deref(), Some(jid));
        assert!(key.participant.is_none());
        assert_eq!(
            whatsapp_rust::wacore::types::message::EditAttribute::infer_from_message(&m),
            Some(whatsapp_rust::wacore::types::message::EditAttribute::SenderRevoke)
        );
    }
}
#[tokio::test]
async fn hiding_or_expiry_cancels_unclaimed_edits_and_purges_their_payload() {
    let (_dir, db) = fixture();
    let mut original = own("ephemeral");
    original.expires = Some(chrono::Utc::now().timestamp() + 60);
    let before = original.text.clone();
    let (chat, id) = seed(&db, original).await;
    db.queue_edit(
        chat,
        id,
        "edit-hidden".into(),
        before.clone(),
        "Private replacement".into(),
    )
    .await
    .unwrap();
    db.queue_hides(chat, vec![id], vec!["hide".into()])
        .await
        .unwrap();
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(job.control.unwrap().kind, ControlKind::Hide);
    assert!(db.claim_next().await.unwrap().is_none());
    db.blocking(|c| {
        assert_eq!(
            c.query_row(
                "SELECT text FROM message_controls WHERE event_wire='edit-hidden'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            ""
        );
        Ok(())
    })
    .unwrap();
    let mut original = own("expired");
    original.expires = Some(chrono::Utc::now().timestamp() + 60);
    let before = original.text.clone();
    let (_, id) = seed(&db, original).await;
    db.queue_edit(
        chat,
        id,
        "edit-expired".into(),
        before,
        "Private expired text".into(),
    )
    .await
    .unwrap();
    db.blocking(|c| {
        c.execute(
            "UPDATE messages SET expires=unixepoch()-1 WHERE wire_id IN ('expired','edit-expired')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(db.claim_next().await.unwrap().is_none());
    db.blocking(|c| {
        assert_eq!(
            c.query_row(
                "SELECT text FROM message_controls WHERE event_wire='edit-expired'",
                [],
                |r| r.get::<_, String>(0)
            )?,
            ""
        );
        Ok(())
    })
    .unwrap();
}
#[tokio::test]
async fn sent_edit_echo_confirms_the_control_and_releases_its_retained_text_without_an_ack() {
    let (_dir, db) = fixture();
    let source = own("original");
    let before = source.text.clone();
    let stamp = source.timestamp;
    let (chat, id) = seed(&db, source).await;
    db.queue_edit(
        chat,
        id,
        "edit-echo".into(),
        before,
        "Edited by echo".into(),
    )
    .await
    .unwrap();
    db.claim_next().await.unwrap().unwrap();
    db.delivery(PN.into(), "edit-echo".into(), "uncertain")
        .await
        .unwrap();
    let mut echo = own("edit-echo");
    echo.visible = false;
    echo.change = Some(Change {
        id: "original".into(),
        sender: "#self".into(),
        text: Some("Edited by echo".into()),
        version: chrono::Utc::now().timestamp_millis() + 10,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![echo],
        ..Default::default()
    })
    .await
    .unwrap();
    let m = &view(&db, chat).await.messages[0];
    assert_eq!(m.id, id);
    assert_eq!(m.text, "Edited by echo");
    assert_eq!(m.content.metadata.stamp, stamp);
    assert!(m.content.metadata.pending.is_empty());
    db.blocking(|c| {
        let value: (i32, String) = c.query_row(
            "SELECT applied,text FROM message_controls WHERE event_wire='edit-echo'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(value, (1, String::new()));
        Ok(())
    })
    .unwrap();
}
#[tokio::test]
async fn edits_commit_only_after_ack_preserve_drafts_and_reject_stale_or_foreign_targets() {
    let (_dir, db) = fixture();
    let original = own("original");
    let before = original.text.clone();
    let (chat, id) = seed(&db, original).await;
    db.draft(chat, "Unsent draft 👩🏽‍💻".into()).await.unwrap();
    assert!(
        db.queue_edit(
            chat,
            id,
            "stale".into(),
            "stale text".into(),
            "replacement".into()
        )
        .await
        .is_err()
    );
    db.queue_edit(
        chat,
        id,
        "edit-event".into(),
        before.clone(),
        "New text\nȘtiință 🦀".into(),
    )
    .await
    .unwrap();
    let pending = view(&db, chat).await;
    assert_eq!(pending.messages[0].text, before);
    assert_eq!(pending.messages[0].content.metadata.pending, "Edit pending");
    assert!(
        db.queue_edit(
            chat,
            id,
            "double".into(),
            before.clone(),
            "Other edit".into()
        )
        .await
        .is_err()
    );
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(job.id, "edit-event");
    let c = job.control.unwrap();
    assert_eq!(c.kind, ControlKind::Edit);
    assert_eq!(c.wire, "original");
    assert_ne!(job.id, c.wire);
    db.delivery(PN.into(), job.id.clone(), "uncertain")
        .await
        .unwrap();
    assert_eq!(view(&db, chat).await.messages[0].text, before);
    db.delivery(PN.into(), job.id, "accepted").await.unwrap();
    let after = view(&db, chat).await;
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].id, id);
    assert_eq!(after.messages[0].text, "New text\nȘtiință 🦀");
    assert_eq!(after.draft, "Unsent draft 👩🏽‍💻");
    assert!(after.messages[0].content.metadata.pending.is_empty());
    let mut foreign = own("foreign");
    foreign.outgoing = false;
    foreign.sender = PN.into();
    db.ingest(Batch {
        messages: vec![foreign],
        ..Default::default()
    })
    .await
    .unwrap();
    let foreign = view(&db, chat)
        .await
        .messages
        .iter()
        .find(|m| !m.outgoing)
        .unwrap()
        .clone();
    assert!(
        db.queue_edit(
            chat,
            foreign.id,
            "bad".into(),
            foreign.text,
            "not ours".into()
        )
        .await
        .is_err()
    );
}
#[tokio::test]
async fn edit_failure_expiry_and_remote_revocation_never_replace_or_restore_content() {
    let (_dir, db) = fixture();
    let original = own("original");
    let before = original.text.clone();
    let (chat, id) = seed(&db, original.clone()).await;
    db.queue_edit(
        chat,
        id,
        "edit-event".into(),
        before.clone(),
        "Replacement".into(),
    )
    .await
    .unwrap();
    let _ = db.claim_next().await.unwrap();
    db.delivery(PN.into(), "edit-event".into(), "failed")
        .await
        .unwrap();
    assert_eq!(view(&db, chat).await.messages[0].text, before);
    let mut revoke = own("remote-revoke");
    revoke.visible = false;
    revoke.change = Some(Change {
        id: "original".into(),
        sender: "#self".into(),
        text: None,
        version: chrono::Utc::now().timestamp_millis() + 10,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![revoke],
        ..Default::default()
    })
    .await
    .unwrap();
    db.delivery(PN.into(), "edit-event".into(), "accepted")
        .await
        .unwrap();
    assert_eq!(view(&db, chat).await.messages[0].text, "[Message deleted]");
    let mut old = own("old");
    old.timestamp -= action::EDIT_WINDOW + 1;
    let body = old.text.clone();
    db.ingest(Batch {
        messages: vec![old],
        ..Default::default()
    })
    .await
    .unwrap();
    let old = view(&db, chat)
        .await
        .messages
        .iter()
        .find(|m| m.text == body)
        .unwrap()
        .id;
    assert!(
        db.queue_edit(chat, old, "late".into(), body, "Expired".into())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn confirmed_revoke_purges_media_and_history_replay_cannot_resurrect_the_message() {
    let (_dir, db) = fixture();
    let original = own("original");
    let (chat, id) = seed(&db, original.clone()).await;
    db.call(move |db| {
        db.execute(
            "INSERT INTO message_audio(message_id,original) VALUES(?1,X'010203')",
            [id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.queue_revoke(chat, id, "revoke-event".into())
        .await
        .unwrap();
    assert_eq!(view(&db, chat).await.messages[0].text, original.text);
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(job.control.unwrap().kind, ControlKind::Revoke);
    db.delivery(PN.into(), job.id, "accepted").await.unwrap();
    db.ingest(Batch {
        messages: vec![original],
        ..Default::default()
    })
    .await
    .unwrap();
    let after = view(&db, chat).await;
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].text, "[Message deleted]");
    db.call(move |c| {
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM message_audio WHERE message_id=?1",
                [id],
                |r| r.get::<_, i32>(0)
            )?,
            0
        );
        Ok(())
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn delete_for_me_is_durable_purges_reply_and_original_and_retries_only_its_sync() {
    let (_dir, db) = fixture();
    let original = own("original");
    let (chat, id) = seed(&db, original.clone()).await;
    db.set_reply(chat, id).await.unwrap();
    db.draft(chat, "Keep this draft".into()).await.unwrap();
    db.queue_hides(chat, vec![id], vec!["hide-event".into()])
        .await
        .unwrap();
    assert!(view(&db, chat).await.messages.is_empty());
    assert!(view(&db, chat).await.reply.is_none());
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(job.id, "hide-event");
    assert_eq!(job.control.unwrap().kind, ControlKind::Hide);
    db.delivery(PN.into(), job.id, "uncertain").await.unwrap();
    assert!(db.claim_next().await.unwrap().is_none());
    db.call(|c| {
        c.execute("UPDATE message_controls SET retry_at=0", [])?;
        Ok(())
    })
    .await
    .unwrap();
    let retry = db.claim_next().await.unwrap().unwrap();
    assert_eq!(retry.id, "hide-event");
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.ingest(Batch {
        messages: vec![original],
        ..Default::default()
    })
    .await
    .unwrap();
    let after = view(&db, chat).await;
    assert!(after.messages.is_empty());
    assert_eq!(after.draft, "Keep this draft");
    db.call(move |c| {
        let row: (i32, i32, i32) = c.query_row(
            "SELECT length(body),length(raw),visible FROM messages WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(row, (0, 0, 0));
        Ok(())
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn early_delete_and_alias_merges_do_not_reveal_messages_or_retain_unread_counts() {
    let (_dir, db) = fixture();
    db.hide_remote(LID.into(), LID.into(), "hidden".into(), false)
        .await
        .unwrap();
    db.recipient_receipts(
        PN.into(),
        vec!["not-in-history".into()],
        PN.into(),
        "read",
        200,
    )
    .await
    .unwrap();
    assert!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .is_empty(),
        "delete/receipt sync cannot invent sidebar entries"
    );
    let mut r = record("hidden");
    r.chat = LID.into();
    r.sender = LID.into();
    db.ingest(Batch {
        messages: vec![r.clone()],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.create_chat(PN.into()).await.unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    r.chat = PN.into();
    r.sender = PN.into();
    db.ingest(Batch {
        messages: vec![r],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let after = view(&db, chat).await;
    assert!(after.messages.is_empty());
    assert_eq!(after.chat.unread, 0);
}
#[tokio::test]
async fn forwarding_and_deleting_selections_are_atomic_when_one_target_is_stale() {
    let (_dir, db) = fixture();
    let r = own("original");
    let (chat, id) = seed(&db, r).await;
    let target = db
        .create_chat("15550000002@s.whatsapp.net".into())
        .await
        .unwrap();
    assert!(
        db.queue_forwards(
            chat,
            vec![id, id + 999],
            target,
            vec!["fwd-1".into(), "fwd-2".into()]
        )
        .await
        .is_err()
    );
    assert!(db.claim_next().await.unwrap().is_none());
    assert!(
        db.queue_hides(
            chat,
            vec![id, id + 999],
            vec!["hide-1".into(), "hide-2".into()]
        )
        .await
        .is_err()
    );
    assert_eq!(view(&db, chat).await.messages.len(), 1);
    assert!(db.claim_next().await.unwrap().is_none());
    db.queue_forwards(chat, vec![id], target, vec!["fwd".into()])
        .await
        .unwrap();
    assert_eq!(db.claim_next().await.unwrap().unwrap().id, "fwd");
}
#[tokio::test]
async fn receipt_evidence_keeps_real_times_and_merges_aliases_without_inventing_missing_times() {
    let (_dir, db) = fixture();
    let (chat, id) = seed(&db, own("original")).await;
    assert!(db.message_info(chat, id).await.unwrap().rows.is_empty());
    db.recipient_receipts(PN.into(), vec!["original".into()], LID.into(), "read", 200)
        .await
        .unwrap();
    db.recipient_receipts(PN.into(), vec!["original".into()], PN.into(), "read", 300)
        .await
        .unwrap();
    db.recipient_receipts(
        PN.into(),
        vec!["original".into()],
        PN.into(),
        "delivered",
        150,
    )
    .await
    .unwrap();
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        profiles: vec![(PN.into(), "Recipient 🦀".into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let info = db.message_info(chat, id).await.unwrap();
    assert_eq!(info.rows.len(), 1);
    assert_eq!(info.rows[0].name, "Recipient 🦀");
    assert!(info.rows[0].detail.contains(&time(200)));
    assert!(info.rows[0].detail.contains(&time(150)));
    assert!(!info.rows[0].detail.contains("Played"));
}
