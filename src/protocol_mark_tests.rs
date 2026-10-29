use super::*;
use crate::archive::{
    Batch,
    tests::{PN, fixture, record},
};
use whatsapp_rust::{prelude::wa, wacore::types::events::StarUpdate};

#[tokio::test]
async fn remote_star_events_are_coalesced_and_do_not_create_visible_chats() {
    let (_temp, archive) = fixture();
    archive
        .ingest(Batch {
            messages: vec![record("source")],
            ..Default::default()
        })
        .await
        .unwrap();
    let state = AppState::default();
    let send = |wire: &str, stamp: i64, starred: bool| {
        let mut action = wa::sync_action_value::StarAction::default();
        action.starred = Some(starred);
        let stamp = chrono::DateTime::from_timestamp_millis(stamp).unwrap();
        state.handle_event(Arc::new(Event::StarUpdate(
            StarUpdate::builder()
                .chat_jid(PN.parse().unwrap())
                .message_id(wire.into())
                .from_me(false)
                .timestamp(stamp)
                .action_timestamp(stamp)
                .action(Box::new(action))
                .from_full_sync(false)
                .build(),
        )));
    };
    send("source", 300, true);
    send("source", 200, false);
    state.flush(&archive, false).await.unwrap();
    let view = archive
        .view(1, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(
        view.conversation.unwrap().messages[0]
            .content
            .metadata
            .starred
    );
    send("source", 400, false);
    state.flush(&archive, false).await.unwrap();
    let view = archive
        .view(1, 100, String::new(), false, false)
        .await
        .unwrap();
    assert!(
        !view.conversation.unwrap().messages[0]
            .content
            .metadata
            .starred
    );
    let mut unknown = wa::sync_action_value::StarAction::default();
    unknown.starred = Some(true);
    state.handle_event(Arc::new(Event::StarUpdate(
        StarUpdate::builder()
            .chat_jid("15550000099@s.whatsapp.net".parse().unwrap())
            .message_id("missing".into())
            .from_me(false)
            .timestamp(chrono::Utc::now())
            .action(Box::new(unknown))
            .from_full_sync(true)
            .build(),
    )));
    state.flush(&archive, false).await.unwrap();
    assert_eq!(
        archive
            .view(0, 100, String::new(), false, false)
            .await
            .unwrap()
            .chats
            .len(),
        1
    );
}
