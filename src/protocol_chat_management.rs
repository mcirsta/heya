//! Public app-state API; explicit timestamps keep acknowledgement and echo ordered.
use crate::chat_management::{Action, Failure, Setting, Source, Target, Update};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use whatsapp_rust::{
    Client,
    prelude::wa,
    schemas,
    wacore_binary::{Jid, JidExt},
};

pub struct Chats(pub Arc<Client>, pub Arc<AtomicBool>);
#[async_trait::async_trait]
impl Source for Chats {
    async fn change(&self, target: Target, action: Action) -> Result<Update, Failure> {
        let jid: Jid = target.jid.parse().map_err(|_| Failure::Failed)?;
        if !(jid.is_pn() || jid.is_lid() || jid.is_group()) {
            return Err(Failure::Failed);
        }
        self.0
            .wait_for_session_ready(std::time::Duration::from_secs(20))
            .await
            .map_err(|_| Failure::Offline)?;
        // A write may outlive its cancelled UI future. Keep recovery dirty
        // for the next connection, including after a normal window close.
        self.1.store(true, Ordering::Release);
        let update = action.update(target.jid, chrono::Utc::now().timestamp_millis());
        let range = target
            .range
            .as_ref()
            .and_then(crate::protocol_reads::read_range);
        let (schema, value) = value(&update, range);
        self.0
            .send_app_state_action(schema, &[jid.to_string().as_str()], &value)
            .await
            .map_err(|_| Failure::Unconfirmed)?;
        Ok(update)
    }
}

fn value(
    update: &Update,
    range: Option<whatsapp_rust::SyncActionMessageRange>,
) -> (&'static whatsapp_rust::schemas::Schema, wa::SyncActionValue) {
    let mut value = wa::SyncActionValue::default();
    value.timestamp = update.stamp;
    let schema = match update.setting {
        Setting::Mute => {
            let mut action = wa::sync_action_value::MuteAction::default();
            action.muted = Some(update.value != 0);
            action.mute_end_timestamp = (update.value != 0).then_some(update.value);
            value.mute_action = action.into();
            &schemas::MUTE
        }
        Setting::Archive => {
            let mut action = wa::sync_action_value::ArchiveChatAction::default();
            action.archived = Some(update.value != 0);
            action.message_range = range.into();
            value.archive_chat_action = action.into();
            &schemas::ARCHIVE
        }
        Setting::Lock => {
            let mut action = wa::sync_action_value::LockChatAction::default();
            action.locked = Some(update.value != 0);
            value.lock_chat_action = action.into();
            &schemas::LOCK_CHAT
        }
    };
    (schema, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_action_payloads_use_the_registered_collections_and_millisecond_units() {
        for (action, name, expected) in [
            (Action::MuteHours(8), "mute", 28_800_100),
            (Action::MuteAlways, "mute", -1),
            (Action::Unmute, "mute", 0),
            (Action::Archive(true), "archive", 1),
            (Action::Archive(false), "archive", 0),
            (Action::Lock, "lock", 1),
        ] {
            let update = action.update("15550000001@s.whatsapp.net".into(), 100);
            let (schema, value) = value(&update, None);
            assert_eq!(schema.name, name);
            assert_eq!(value.timestamp, Some(100));
            match update.setting {
                Setting::Mute => {
                    let a = value.mute_action.as_option().unwrap();
                    assert_eq!(a.muted, Some(expected != 0));
                    assert_eq!(a.mute_end_timestamp, (expected != 0).then_some(expected));
                }
                Setting::Archive => assert_eq!(
                    value.archive_chat_action.as_option().unwrap().archived,
                    Some(expected != 0)
                ),
                Setting::Lock => assert_eq!(
                    value.lock_chat_action.as_option().unwrap().locked,
                    Some(true)
                ),
            }
        }
    }
}
