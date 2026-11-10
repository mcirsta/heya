//! Non-message events and phone-only display text. Never fetches missing content.
use crate::message_content::{Content, Notice};
use whatsapp_rust::{
    prelude::{MessageExt, wa},
    waproto::codec,
};

pub const PHONE: &str = "Open WhatsApp on your phone to view this message.";
pub const CODE: &str = "One-time code. Open WhatsApp on your phone to view it.";

pub fn content(message: &wa::Message) -> Option<(String, Content)> {
    let text = if message.placeholder_message.is_set() {
        if message.placeholder_message.r#type
            == Some(wa::message::placeholder_message::PlaceholderType::MASK_LINKED_DEVICES)
        {
            CODE
        } else {
            PHONE
        }
    } else if message.message_history_notice.is_set() {
        return Some((
            Notice::HistoryShared.description().into(),
            Content {
                notice: Some(Notice::HistoryShared),
                ..Default::default()
            },
        ));
    } else {
        return None;
    };
    Some((
        text.into(),
        Content {
            body: Some(text.into()),
            ..Default::default()
        },
    ))
}

pub fn hidden(message: &wa::Message) -> bool {
    message.secret_encrypted_message.is_set() || message.poll_update_message.is_set()
}

/// A history-share notice is visible to other group members, not its sender
/// or the accounts receiving the history. JID comparison stays in storage.
pub fn history_receivers(raw: &[u8]) -> Option<Vec<String>> {
    let message = codec::message_decode(raw).ok()?;
    let metadata = message
        .get_base_message()
        .message_history_notice
        .message_history_metadata
        .as_option()?;
    metadata.oldest_message_timestamp_in_window?;
    Some(metadata.history_receivers.clone())
}

pub fn repairable(message: &wa::Message) -> bool {
    let base = message.get_base_message();
    !message.is_view_once()
        && (base.protocol_message.is_set()
            || hidden(base)
            || base.message_history_notice.is_set()
            || base.placeholder_message.is_set()
            || base.live_location_message.is_set())
}
