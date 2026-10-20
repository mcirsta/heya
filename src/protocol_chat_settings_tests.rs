use super::*;
use crate::{
    archive::{
        Batch,
        tests::{PN, fixture, record},
    },
    chat_management::Setting,
};
use whatsapp_rust::{
    prelude::wa,
    wacore::types::events::{ArchiveUpdate, CoreEventBus, LockChatUpdate, MuteUpdate},
};

fn event(setting: Setting, value: Option<i64>, stamp: Option<i64>) -> Event {
    let time = chrono::DateTime::from_timestamp_millis(stamp.unwrap_or(0)).unwrap();
    match setting {
        Setting::Mute => {
            let mut action = wa::sync_action_value::MuteAction::default();
            action.muted = value.map(|v| v != 0);
            action.mute_end_timestamp = value;
            Event::MuteUpdate(
                MuteUpdate::builder()
                    .jid(PN.parse().unwrap())
                    .timestamp(time)
                    .maybe_action_timestamp(stamp.map(|_| time))
                    .action(Box::new(action))
                    .from_full_sync(true)
                    .build(),
            )
        }
        Setting::Archive => {
            let mut action = wa::sync_action_value::ArchiveChatAction::default();
            action.archived = value.map(|v| v != 0);
            Event::ArchiveUpdate(
                ArchiveUpdate::builder()
                    .jid(PN.parse().unwrap())
                    .timestamp(time)
                    .maybe_action_timestamp(stamp.map(|_| time))
                    .action(Box::new(action))
                    .from_full_sync(true)
                    .build(),
            )
        }
        Setting::Lock => {
            let mut action = wa::sync_action_value::LockChatAction::default();
            action.locked = value.map(|v| v != 0);
            Event::LockChatUpdate(
                LockChatUpdate::builder()
                    .jid(PN.parse().unwrap())
                    .timestamp(time)
                    .maybe_action_timestamp(stamp.map(|_| time))
                    .action(Box::new(action))
                    .from_full_sync(true)
                    .build(),
            )
        }
    }
}

#[tokio::test]
async fn actual_event_interest_coalesces_chat_flags_and_ignores_missing_or_stale_values() {
    let (_temp, db) = fixture();
    db.ingest(Batch {
        messages: vec![record("one")],
        live: true,
        ..Default::default()
    })
    .await
    .unwrap();
    let updates = AppState::default();
    let bus = CoreEventBus::new();
    bus.subscribe_handler(Arc::new(updates.clone())).detach();
    for _ in 0..1000 {
        bus.dispatch(event(Setting::Mute, Some(-1), Some(200)));
    }
    bus.dispatch(event(Setting::Mute, Some(0), Some(100)));
    bus.dispatch(event(Setting::Mute, Some(0), None));
    bus.dispatch(event(Setting::Archive, None, Some(300)));
    assert_eq!(updates.0.pending.lock().unwrap().chats.len(), 1);
    updates.flush(&db, false).await.unwrap();
    let view = db.view(0, 100, String::new(), false, false).await.unwrap();
    assert!(view.chats[0].muted);
    assert_eq!(view.unread_chats, 0);
    bus.dispatch(event(Setting::Archive, Some(1), Some(300)));
    updates.flush(&db, false).await.unwrap();
    assert_eq!(
        db.view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .archived_count,
        1
    );
    bus.dispatch(event(Setting::Lock, Some(1), Some(400)));
    updates.flush(&db, false).await.unwrap();
    assert!(
        db.view_in(0, 100, String::new(), false, false, true)
            .await
            .unwrap()
            .chats
            .is_empty()
    );
    bus.dispatch(event(Setting::Lock, Some(0), Some(500)));
    updates.flush(&db, false).await.unwrap();
    assert_eq!(
        db.view_in(0, 100, String::new(), false, false, true)
            .await
            .unwrap()
            .chats
            .len(),
        1
    );
}
