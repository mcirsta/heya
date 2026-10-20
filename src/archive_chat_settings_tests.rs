use super::*;
use crate::archive::tests::{KEY, LID, PN, fixture, record};
use crate::chat_management::Action;

async fn seed(db: &Archive) -> i32 {
    db.ingest(Batch {
        messages: vec![record("incoming")],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    db.view(0, 100, String::new(), false, false)
        .await
        .unwrap()
        .chats[0]
        .id
}
fn update(setting: Setting, value: i64, stamp: Option<i64>) -> Update {
    Update {
        jid: PN.into(),
        setting,
        value,
        stamp,
    }
}

#[tokio::test]
async fn mute_expiry_archive_folder_and_badge_keep_the_real_unread_count() {
    let (_temp, db) = fixture();
    let chat = seed(&db).await;
    let now = chrono::Utc::now().timestamp_millis();
    db.save_chat_settings(vec![update(Setting::Mute, now + 3_600_000, Some(10))])
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(view.chats[0].muted);
    assert_eq!(view.chats[0].unread, 1);
    assert_eq!(view.unread_chats, 0);
    db.save_chat_settings(vec![update(Setting::Mute, now - 1000, Some(20))])
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(!view.chats[0].muted);
    assert_eq!(view.unread_chats, 1);
    db.save_chat_settings(vec![update(Setting::Archive, 1, Some(30))])
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(view.chats.is_empty());
    assert!(view.conversation.is_none());
    assert_eq!(view.archived_count, 1);
    assert_eq!(view.unread_chats, 0);
    let archived = db
        .view_in(chat, 100, String::new(), false, false, true)
        .await
        .unwrap();
    assert!(archived.archived_view);
    assert!(archived.chats[0].archived);
    assert_eq!(archived.conversation.unwrap().chat.unread, 1);
    db.save_chat_settings(vec![update(Setting::Archive, 0, Some(40))])
        .await
        .unwrap();
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .unread_chats,
        1
    );
}

#[tokio::test]
async fn locked_chats_disappear_without_losing_drafts_messages_or_unread_state() {
    let (_temp, db) = fixture();
    let chat = seed(&db).await;
    db.draft(chat, "Keep this synthetic draft".into())
        .await
        .unwrap();
    let message = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap()
        .conversation
        .unwrap()
        .messages[0]
        .id;
    db.save_chat_settings(vec![update(Setting::Lock, 1, Some(10))])
        .await
        .unwrap();
    for archived in [false, true] {
        let view = db
            .view_in(chat, 100, "Synthetic".into(), false, false, archived)
            .await
            .unwrap();
        assert!(view.chats.is_empty());
        assert!(view.conversation.is_none());
        assert!(!view.can_send);
        assert_eq!(view.unread_chats, 0);
    }
    assert!(db.chat_management_target(chat).await.is_err());
    assert!(
        db.queue(chat, "outgoing".into(), "Synthetic text".into())
            .await
            .is_err()
    );
    assert!(db.set_reply(chat, message).await.is_err());
    assert!(db.attachment_destination(chat).await.is_err());
    assert!(db.mark_visible_read(chat, message).await.is_err());
    assert!(
        db.view(chat, 100, String::new(), false, true)
            .await
            .is_err()
    );
    assert!(db.destinations().await.unwrap().is_empty());
    assert!(db.presence_target(chat).await.unwrap().is_none());
    db.save_chat_settings(vec![update(Setting::Lock, 0, Some(20))])
        .await
        .unwrap();
    let view = db
        .view(chat, 100, String::new(), false, false)
        .await
        .unwrap();
    assert_eq!(view.unread_chats, 1);
    let conversation = view.conversation.unwrap();
    assert_eq!(conversation.draft, "Keep this synthetic draft");
    assert_eq!(conversation.messages.len(), 1);
}

#[tokio::test]
async fn archive_actions_reference_a_confirmed_tail_instead_of_a_queued_local_send() {
    let (_temp, db) = fixture();
    let chat = seed(&db).await;
    db.queue(chat, "queued-local".into(), "Synthetic outgoing".into())
        .await
        .unwrap();
    let range = db
        .chat_management_target(chat)
        .await
        .unwrap()
        .range
        .unwrap();
    assert_eq!(range.message.unwrap().wire, "incoming");
    let mut confirmed = record("queued-local");
    confirmed.outgoing = true;
    confirmed.timestamp = 1_700_000_100;
    db.ingest(Batch {
        messages: vec![confirmed],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let range = db
        .chat_management_target(chat)
        .await
        .unwrap()
        .range
        .unwrap();
    assert_eq!(range.through, 1_700_000_100);
    assert_eq!(range.message.unwrap().wire, "queued-local");
}

#[tokio::test]
async fn versioned_flags_survive_stale_history_alias_merge_and_restart() {
    let (_temp, db) = fixture();
    let chat = seed(&db).await;
    let files = db.0._files.clone();
    db.save_chat_settings(vec![Update {
        jid: LID.into(),
        setting: Setting::Mute,
        value: -1,
        stamp: Some(200),
    }])
    .await
    .unwrap();
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .len(),
        1,
        "metadata must not create chats"
    );
    db.ingest(Batch {
        aliases: vec![(LID.into(), PN.into())],
        settings: vec![update(Setting::Mute, 0, None)],
        ..Default::default()
    })
    .await
    .unwrap();
    db.save_chat_settings(vec![update(Setting::Mute, 0, Some(100))])
        .await
        .unwrap();
    assert!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .muted
    );
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats[0]
            .muted
    );
    db.save_chat_settings(vec![update(Setting::Mute, 0, Some(300))])
        .await
        .unwrap();
    assert_eq!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .unread_chats,
        1
    );
}

#[tokio::test]
async fn schema_fifteen_upgrade_keeps_data_and_requests_a_snapshot_after_an_unclean_exit() {
    let (_temp, db) = fixture();
    let chat = seed(&db).await;
    db.draft(chat, "Preserved draft".into()).await.unwrap();
    let files = db.0._files.clone();
    db.blocking(|c| {
        c.execute_batch("DROP TABLE chat_settings; PRAGMA user_version=15;")?;
        Ok(())
    })
    .unwrap();
    drop(db);
    let db = Archive::open(files, KEY).unwrap();
    assert_eq!(
        db.view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap()
            .draft,
        "Preserved draft"
    );
    assert!(db.begin_chat_settings().await.unwrap());
    db.finish_chat_settings().await.unwrap();
    assert!(!db.begin_chat_settings().await.unwrap());
    assert!(db.begin_chat_settings().await.unwrap());
    db.save_chat_settings(vec![Action::MuteAlways.update(PN.into(), 10)])
        .await
        .unwrap();
}
