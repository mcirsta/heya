use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture, record};
use whatsapp_rust::{
    prelude::{MessageBuilderExt, MessageExt, wa},
    waproto::{buffa::MessageField, codec},
};
const OWNER: &str = "15550000009@s.whatsapp.net";

fn text_record(id: &str) -> Record {
    let mut r = record(id);
    r.text = "Synthetic original 👩🏽‍💻\nSecond line".into();
    r.raw = codec::message_to_vec(&wa::Message::text(r.text.clone()));
    r.timestamp = chrono::Utc::now().timestamp();
    r
}
async fn load(db: &Archive, chat: i32) -> domain::Conversation {
    db.view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
}

#[tokio::test]
async fn replies_and_forwarding_keep_the_target_drafts_and_durable_wire_content() {
    let (_temp, db) = fixture();
    db.bind_account(OWNER.into(), "Owner".into()).await.unwrap();
    db.ingest(Batch {
        messages: vec![text_record("original")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let target = db
        .create_chat("15550000002@s.whatsapp.net".into())
        .await
        .unwrap();
    let original = load(&db, chat).await.messages[0].id;
    db.draft(chat, "Reply draft".into()).await.unwrap();
    db.draft(target, "Unrelated destination draft".into())
        .await
        .unwrap();
    db.set_reply(chat, original).await.unwrap();
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    let restored = load(&db, chat).await;
    assert_eq!(restored.reply.as_ref().unwrap().id, original);
    assert_eq!(restored.draft, "Reply draft");
    db.queue(chat, "reply-operation".into(), "Quoted answer".into())
        .await
        .unwrap();
    let reply = db.claim_next().await.unwrap().unwrap();
    assert_eq!(reply.jid, PN);
    let decoded = codec::message_decode(&reply.raw).unwrap();
    let context = decoded.context_info().unwrap();
    assert_eq!(context.stanza_id.as_deref(), Some("original"));
    assert_eq!(context.participant.as_deref(), Some(PN));
    assert_eq!(decoded.text_content(), Some("Quoted answer"));
    assert_eq!(
        context.quoted_message.text_content(),
        Some("Synthetic original 👩🏽‍💻\nSecond line")
    );
    db.delivery(PN.into(), reply.id, "accepted").await.unwrap();
    let view = load(&db, chat).await;
    assert!(view.reply.is_none());
    assert!(view.draft.is_empty());
    assert!(view.messages.last().unwrap().content.quote.is_some());
    db.queue_operation(
        chat,
        original,
        target,
        "forward-operation".into(),
        Operation::Forward,
    )
    .await
    .unwrap();
    let forwarded = db.claim_next().await.unwrap().unwrap();
    assert_eq!(forwarded.chat, target);
    let decoded = codec::message_decode(&forwarded.raw).unwrap();
    assert_eq!(decoded.context_info().unwrap().is_forwarded, Some(true));
    assert!(!decoded.context_info().unwrap().quoted_message.is_set());
    assert_eq!(load(&db, target).await.draft, "Unrelated destination draft");
    assert!(
        load(&db, target)
            .await
            .messages
            .last()
            .unwrap()
            .content
            .forwarded
    );
    assert!(
        db.queue_operation(
            target,
            original,
            chat,
            "wrong-source".into(),
            Operation::Forward
        )
        .await
        .is_err()
    );
    db.blocking(move |c| {
        c.execute("UPDATE chats SET read_only=1 WHERE id=?1", [target])?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.queue_operation(
            chat,
            original,
            target,
            "readonly".into(),
            Operation::Forward
        )
        .await
        .is_err()
    );
    assert!(
        !db.destinations()
            .await
            .unwrap()
            .iter()
            .any(|d| d.id == target)
    );
}

#[tokio::test]
async fn reaction_replace_remove_failure_and_restart_do_not_create_message_bubbles() {
    let (_temp, db) = fixture();
    db.bind_account(OWNER.into(), "Owner".into()).await.unwrap();
    db.ingest(Batch {
        messages: vec![text_record("original")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let id = load(&db, chat).await.messages[0].id;
    for (wire, emoji) in [
        ("react-one", "👍"),
        ("react-two", "❤️"),
        ("react-remove", ""),
    ] {
        db.queue_operation(chat, id, chat, wire.into(), Operation::React(emoji.into()))
            .await
            .unwrap();
        let view = load(&db, chat).await;
        assert_eq!(view.messages.len(), 1);
        assert_eq!(view.messages[0].content.own_reaction, emoji);
        let job = db.claim_next().await.unwrap().unwrap();
        let decoded = codec::message_decode(&job.raw).unwrap();
        assert_eq!(decoded.reaction_message.key.id.as_deref(), Some("original"));
        assert_eq!(decoded.reaction_message.text.as_deref(), Some(emoji));
        db.delivery(PN.into(), wire.into(), "accepted")
            .await
            .unwrap();
    }
    db.queue_operation(
        chat,
        id,
        chat,
        "reaction-failed".into(),
        Operation::React("😮".into()),
    )
    .await
    .unwrap();
    let _ = db.claim_next().await.unwrap();
    db.delivery(PN.into(), "reaction-failed".into(), "failed")
        .await
        .unwrap();
    assert!(
        load(&db, chat).await.messages[0]
            .content
            .reaction_status
            .contains("failed")
    );
    db.queue_operation(
        chat,
        id,
        chat,
        "reaction-uncertain".into(),
        Operation::React("🙏".into()),
    )
    .await
    .unwrap();
    let _ = db.claim_next().await.unwrap();
    let files = db.0._files.clone();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert!(db.claim_next().await.unwrap().is_none());
    assert!(
        load(&db, chat).await.messages[0]
            .content
            .reaction_status
            .contains("unconfirmed")
    );
    assert!(
        db.queue_operation(
            chat,
            id,
            chat,
            "not-emoji".into(),
            Operation::React("arbitrary text".into())
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn expired_and_view_once_sources_cannot_be_replied_to_or_forwarded() {
    let (_temp, db) = fixture();
    db.bind_account(OWNER.into(), "Owner".into()).await.unwrap();
    let mut once = text_record("once");
    once.raw.clear();
    once.text = "[View-once message]".into();
    db.ingest(Batch {
        messages: vec![text_record("original"), once],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let view = load(&db, chat).await;
    let id = view.messages[0].id;
    assert!(db.set_reply(chat, view.messages[1].id).await.is_err());
    db.set_reply(chat, id).await.unwrap();
    db.draft(chat, "Keep this text".into()).await.unwrap();
    db.blocking(move |c| {
        c.execute("UPDATE messages SET expires=1 WHERE id=?1", [id])?;
        Ok(())
    })
    .unwrap();
    assert!(
        db.queue(chat, "expired-quote".into(), "Keep this text".into())
            .await
            .is_err()
    );
    assert!(
        db.queue_operation(chat, id, chat, "expired-forward".into(), Operation::Forward)
            .await
            .is_err()
    );
    let view = load(&db, chat).await;
    assert_eq!(view.draft, "Keep this text");
    assert!(view.reply.is_none());
    assert!(view.messages.iter().all(|m| m.content.unavailable));
}

#[test]
fn protocol_forwarding_preserves_media_and_strips_quote_chain() {
    let mut image = wa::message::ImageMessage::default();
    image.url = Some("https://example.invalid/synthetic".into());
    image.media_key = Some(vec![42; 32]);
    image.caption = Some("Synthetic photo".into());
    let mut context = wa::ContextInfo::default();
    context.stanza_id = Some("old-quote".into());
    context.participant = Some(PN.into());
    context.quoted_message = MessageField::some(wa::Message::text("Nested quote"));
    image.context_info = MessageField::some(context);
    let mut message = wa::Message::default();
    message.image_message = MessageField::some(image);
    let source = Source {
        id: 1,
        chat: 1,
        jid: PN.into(),
        sender: PN.into(),
        wire: "photo".into(),
        outgoing: false,
        body: "Synthetic photo".into(),
        raw: codec::message_to_vec(&message),
    };
    let result = crate::protocol_actions::prepare(&source, Operation::Forward, OWNER, 1).unwrap();
    let decoded = codec::message_decode(&result.raw).unwrap();
    assert_eq!(decoded.image_message.media_key, Some(vec![42; 32]));
    assert_eq!(decoded.image_message.url, message.image_message.url);
    let context = decoded.context_info().unwrap();
    assert_eq!(context.is_forwarded, Some(true));
    assert!(!context.quoted_message.is_set());
    assert!(context.stanza_id.is_none());
}

#[tokio::test]
async fn reactions_arriving_before_originals_merge_actor_aliases_and_ignore_stale_events() {
    let (_temp, db) = fixture();
    let mut event = text_record("reaction-early");
    event.visible = false;
    event.sender = LID.into();
    event.content.reaction = Some(crate::message_content::ReactionEvent {
        target: "original".into(),
        emoji: "👍".into(),
        at: 20,
    });
    db.ingest(Batch {
        messages: vec![event.clone()],
        ..Default::default()
    })
    .await
    .unwrap();
    db.ingest(Batch {
        messages: vec![text_record("original")],
        aliases: vec![(LID.into(), PN.into())],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    assert_eq!(
        load(&db, chat).await.messages[0].content.reaction_summary,
        "👍"
    );
    event.id = "new-reaction".into();
    event.sender = PN.into();
    event.content.reaction.as_mut().unwrap().at = 30;
    event.content.reaction.as_mut().unwrap().emoji = "❤️".into();
    db.ingest(Batch {
        messages: vec![event.clone()],
        ..Default::default()
    })
    .await
    .unwrap();
    event.id = "stale-reaction".into();
    event.content.reaction.as_mut().unwrap().at = 10;
    event.content.reaction.as_mut().unwrap().emoji = "😮".into();
    db.ingest(Batch {
        messages: vec![event],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        load(&db, chat).await.messages[0].content.reaction_summary,
        "❤️"
    );
}

#[tokio::test]
async fn incoming_uncatalogued_reactions_survive_archive_reopen_replacement_and_removal() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![text_record("original")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let mut db = db;
    // Synthetic future scalar and ZWJ sequence, deliberately outside the local
    // catalogue. Font support must not decide what is stored or shown.
    for (at, emoji) in [(10, "\u{1faff}"), (20, "😀\u{200d}🛸"), (30, "")] {
        assert!(emoji.is_empty() || emojis::get(emoji).is_none());
        let mut key = wa::MessageKey::default();
        key.id = Some("original".into());
        let message = whatsapp_rust::wacore::proto_helpers::build_reaction_message(key, emoji, at);
        let mut event = text_record(&format!("unknown-reaction-{at}"));
        event.visible = false;
        event.text.clear();
        event.raw = codec::message_to_vec(&message);
        event.content = crate::protocol_content::extract(&message).1;
        assert_eq!(event.content.reaction.as_ref().unwrap().emoji, emoji);
        db.ingest(Batch {
            messages: vec![event],
            ..Default::default()
        })
        .await
        .unwrap();
        let files = db.0._files.clone();
        drop(db);
        db = Archive::open(files, KEY).unwrap();
        let view = load(&db, chat).await;
        assert_eq!(view.messages.len(), 1, "a reaction is not a message bubble");
        assert_eq!(view.messages[0].content.reaction_summary, emoji);
    }
}

#[test]
fn incoming_reactions_still_reject_oversized_or_control_character_payloads() {
    for emoji in ["😀".repeat(65), "👍\n".into(), "\0".into()] {
        let mut key = wa::MessageKey::default();
        key.id = Some("original".into());
        let message = whatsapp_rust::wacore::proto_helpers::build_reaction_message(key, emoji, 1);
        assert!(
            crate::protocol_content::extract(&message)
                .1
                .reaction
                .is_none()
        );
    }
}

#[tokio::test]
async fn edited_quotes_use_current_text_and_deleted_targets_reject_without_losing_drafts() {
    let (_temp, db) = fixture();
    db.bind_account(OWNER.into(), "Owner".into()).await.unwrap();
    db.ingest(Batch {
        messages: vec![text_record("original")],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let id = load(&db, chat).await.messages[0].id;
    let mut edit = text_record("edit");
    edit.visible = false;
    edit.change = Some(Change {
        id: "original".into(),
        sender: PN.into(),
        text: Some("Edited text".into()),
        version: 20,
        raw: Vec::new(),
    });
    db.ingest(Batch {
        messages: vec![edit.clone()],
        ..Default::default()
    })
    .await
    .unwrap();
    db.set_reply(chat, id).await.unwrap();
    db.queue(chat, "reply-to-edit".into(), "Answer".into())
        .await
        .unwrap();
    let job = db.claim_next().await.unwrap().unwrap();
    assert_eq!(
        codec::message_decode(&job.raw)
            .unwrap()
            .context_info()
            .unwrap()
            .quoted_message
            .text_content(),
        Some("Edited text")
    );
    db.set_reply(chat, id).await.unwrap();
    db.draft(chat, "Keep draft on failure".into())
        .await
        .unwrap();
    edit.id = "revoke".into();
    edit.change.as_mut().unwrap().text = None;
    db.ingest(Batch {
        messages: vec![edit],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        db.queue(chat, "invalid-reply".into(), "Keep draft on failure".into())
            .await
            .is_err()
    );
    assert!(
        db.queue_operation(chat, id, chat, "invalid-forward".into(), Operation::Forward)
            .await
            .is_err()
    );
    let view = load(&db, chat).await;
    assert_eq!(view.draft, "Keep draft on failure");
    assert!(
        view.messages
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .content
            .unavailable
    );
}

#[tokio::test]
async fn schema_seven_upgrade_repairs_action_metadata_without_discarding_media_cache() {
    let (_temp, db) = fixture();
    let mut r = text_record("original");
    let mut message = wa::Message::default();
    message.audio_message = MessageField::some(wa::message::AudioMessage::default());
    let mut context = wa::ContextInfo::default();
    context.participant = Some(PN.into());
    context.quoted_message = MessageField::some(wa::Message::text("Quoted original"));
    assert!(message.set_context_info(context));
    r.raw = codec::message_to_vec(&message);
    let (_, mut legacy_content, _) = crate::protocol_content::extract(&message);
    legacy_content.quote = None;
    r.content = legacy_content;
    db.ingest(Batch {
        messages: vec![r],
        ..Default::default()
    })
    .await
    .unwrap();
    let chat = db.create_chat(PN.into()).await.unwrap();
    let id = load(&db, chat).await.messages[0].id;
    let files = db.0._files.clone();
    db.blocking(move|c| {
        // Model a v7 cached envelope: metadata-only upgrades must keep opaque
        // encrypted media bytes; changes to the actual envelope must purge them.
        c.execute("INSERT INTO message_audio(message_id,original) VALUES(?1,X'010203')",[id])?;
        c.execute_batch("DROP TABLE reply_drafts; DROP TABLE reactions; DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE outgoing_reads; DROP INDEX messages_action_wire;
            UPDATE metadata SET value='3' WHERE key='content_decoder_revision'; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; DROP TABLE outgoing_receipts; DROP TABLE chat_settings; PRAGMA user_version=7;")?;
        Ok(())
    }).unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    db.repair_message_content(|raw| {
        let message = codec::message_decode(raw).ok()?;
        let (_, content, thumbnail) = crate::protocol_content::extract(&message);
        Some((content, thumbnail))
    })
    .unwrap();
    assert_eq!(
        load(&db, chat).await.messages[0]
            .content
            .quote
            .as_ref()
            .unwrap()
            .text,
        "Quoted original"
    );
    db.blocking(move |c| {
        assert_eq!(
            c.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))?,
            16
        );
        assert_eq!(
            c.query_row(
                "SELECT original FROM message_audio WHERE message_id=?1",
                [id],
                |r| r.get::<_, Vec<u8>>(0)
            )?,
            [1, 2, 3]
        );
        c.execute("UPDATE messages SET raw=X'04' WHERE id=?1", [id])?;
        assert_eq!(
            c.query_row("SELECT count(*) FROM message_audio", [], |r| r
                .get::<_, i32>(0))?,
            0
        );
        Ok(())
    })
    .unwrap();
}

#[tokio::test]
async fn alias_merge_preserves_duplicate_quote_but_clears_ambiguous_quote_targets() {
    for (wire, should_keep) in [("same", true), ("different", false)] {
        let (_temp, db) = fixture();
        let mut pn = text_record("same");
        let mut lid = text_record(wire);
        pn.chat = PN.into();
        pn.sender = PN.into();
        lid.chat = LID.into();
        lid.sender = LID.into();
        db.ingest(Batch {
            messages: vec![pn, lid],
            ..Default::default()
        })
        .await
        .unwrap();
        let a = db.create_chat(PN.into()).await.unwrap();
        let b = db.create_chat(LID.into()).await.unwrap();
        let first = load(&db, a).await.messages[0].id;
        let second = load(&db, b).await.messages[0].id;
        db.set_reply(a, first).await.unwrap();
        db.set_reply(b, second).await.unwrap();
        db.draft(a, "Identical draft".into()).await.unwrap();
        db.draft(b, "Identical draft".into()).await.unwrap();
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        let view = load(&db, a).await;
        assert_eq!(view.draft, "Identical draft");
        assert_eq!(view.reply.is_some(), should_keep);
        if should_keep {
            assert_eq!(view.reply.unwrap().id, first);
        }
    }
}
