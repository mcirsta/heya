//! Pure protocol construction; no network calls, Slint values or application state.
use crate::{
    archive::{Error, Result},
    message_actions::{Operation, Prepared, Source},
    message_content::{self, Quote, ReactionEvent},
};
use whatsapp_rust::{
    prelude::{MessageBuilderExt, MessageExt, wa},
    waproto::{buffa::MessageField, codec},
};

pub fn editable(source: &Source) -> bool {
    if !source.outgoing {
        return false;
    }
    if source.raw.is_empty() {
        return true;
    }
    let Ok(message) = codec::message_decode(&source.raw) else {
        return false;
    };
    let base = message.get_base_message();
    !message.is_view_once()
        && (base.conversation.is_some() || base.extended_text_message.is_set())
        && crate::protocol_business::text(base).is_none()
}
pub fn revoke(jid: &str, wire: String) -> wa::Message {
    let mut key = wa::MessageKey::default();
    key.remote_jid = Some(jid.into());
    key.from_me = Some(true);
    key.id = Some(wire);
    let mut protocol = wa::message::ProtocolMessage::default();
    protocol.key = MessageField::some(key);
    protocol.r#type = Some(wa::message::protocol_message::Type::Revoke);
    let mut message = wa::Message::default();
    message.protocol_message = MessageField::some(protocol);
    message
}

/// Use the public canonical send API with our durable operation ID. The SDK's
/// pin convenience methods allocate their own ID, which cannot be ACK-correlated
/// with an already saved outbox control.
pub fn pin(key: wa::MessageKey, seconds: u32, stamp: i64) -> Option<wa::Message> {
    if !matches!(seconds, 0 | 86400 | 604800 | 2592000) {
        return None;
    }
    let mut pin = wa::message::PinInChatMessage::default();
    pin.key = MessageField::some(key);
    pin.r#type = Some(if seconds == 0 {
        wa::message::pin_in_chat_message::Type::UNPIN_FOR_ALL
    } else {
        wa::message::pin_in_chat_message::Type::PIN_FOR_ALL
    });
    pin.sender_timestamp_ms = Some(stamp);
    let mut context = wa::MessageContextInfo::default();
    context.message_add_on_duration_in_secs = Some(seconds);
    let mut message = wa::Message::default();
    message.pin_in_chat_message = MessageField::some(pin);
    message.message_context_info = MessageField::some(context);
    Some(message)
}

pub fn pin_update(record: &crate::archive::Record) -> Option<crate::message_actions::MarkUpdate> {
    let message = codec::message_decode(&record.raw).ok()?;
    let base = message.get_base_message();
    let pin = base.pin_in_chat_message.as_option()?;
    let key = pin.key.as_option()?;
    let wire = key.id.clone().filter(|id| !id.is_empty())?;
    let sender = if record.chat.ends_with("@g.us") {
        key.participant
            .clone()
            .filter(|sender| !sender.is_empty())
            .or_else(|| {
                (key.from_me == Some(true)).then(|| {
                    if record.outgoing {
                        "#self".into()
                    } else {
                        record.sender.clone()
                    }
                })
            })?
    } else if key.from_me? == record.outgoing {
        "#self".into()
    } else {
        record.chat.clone()
    };
    let stamp = pin
        .sender_timestamp_ms
        .unwrap_or_else(|| record.timestamp.saturating_mul(1000));
    let until = match pin.r#type? {
        wa::message::pin_in_chat_message::Type::PIN_FOR_ALL => {
            let seconds = base
                .message_context_info
                .as_option()?
                .message_add_on_duration_in_secs?;
            if seconds == 0 {
                return None;
            }
            (stamp / 1000).saturating_add(i64::from(seconds))
        }
        wa::message::pin_in_chat_message::Type::UNPIN_FOR_ALL => 0,
        _ => return None,
    };
    Some(crate::message_actions::MarkUpdate {
        jid: record.chat.clone(),
        sender,
        wire,
        starred: None,
        pinned_until: Some(until),
        stamp: Some(stamp),
    })
}

pub fn prepare(source: &Source, operation: Operation, owner: &str, now: i64) -> Result<Prepared> {
    let original = if source.raw.is_empty() {
        wa::Message::text(source.body.clone())
    } else {
        codec::message_decode(&source.raw).map_err(|_| Error::Invalid)?
    };
    if original.is_view_once() {
        return Err(Error::Invalid);
    }
    let base = original.get_base_message();
    let message = match operation {
        Operation::Reply(text) => {
            if text.trim().is_empty() || text.len() > 64 * 1024 {
                return Err(Error::Invalid);
            }
            let mut message = wa::Message::text(text);
            let mut context = wa::ContextInfo::default();
            context.stanza_id = Some(source.wire.clone());
            context.participant = Some(if source.outgoing {
                owner.into()
            } else {
                source.sender.clone()
            });
            context.quoted_message = MessageField::from_box(base.prepare_for_quote());
            if !message.set_context_info(context) {
                return Err(Error::Invalid);
            }
            message
        }
        Operation::React(emoji) => {
            if !emoji.is_empty() && emojis::get(&emoji).is_none() {
                return Err(Error::Invalid);
            }
            let mut key = wa::MessageKey::default();
            key.remote_jid = Some(source.jid.clone());
            key.id = Some(source.wire.clone());
            key.from_me = Some(source.outgoing);
            if source.jid.ends_with("@g.us") && !source.outgoing {
                key.participant = Some(source.sender.clone());
            }
            whatsapp_rust::wacore::proto_helpers::build_reaction_message(key, emoji, now)
        }
        Operation::Forward => {
            if base.protocol_message.is_set() || base.reaction_message.is_set() {
                return Err(Error::Invalid);
            }
            if crate::protocol_business::text(base).is_some() {
                wa::Message::text(source.body.clone()).prepare_for_forward()
            } else {
                base.prepare_for_forward()
            }
        }
    };
    let (business, content, thumbnail) = crate::protocol_content::extract(&message);
    let text = message
        .text_content()
        .map(str::to_owned)
        .or(business)
        .unwrap_or_else(|| {
            if message.image_message.is_set() {
                format!(
                    "[Photo] {}",
                    message.image_message.caption.as_deref().unwrap_or("")
                )
            } else if message.audio_message.is_set() {
                "[Audio message]".into()
            } else if message.reaction_message.is_set() {
                String::new()
            } else {
                source.body.clone()
            }
        });
    Ok(Prepared {
        raw: codec::message_to_vec(&message),
        text,
        visible: !message.reaction_message.is_set(),
        content,
        thumbnail,
    })
}

pub fn decorate(message: &wa::Message, content: &mut message_content::Content) {
    let base = message.get_base_message();
    if let Some(context) = base.context_info() {
        content.forwarded = context.is_forwarded.unwrap_or(false);
        if let Some(quoted) = context.quoted_message.as_option() {
            let body = if quoted.is_view_once() {
                "[View-once message]".into()
            } else {
                quoted
                    .text_content()
                    .map(str::to_owned)
                    .or_else(|| crate::protocol_business::text(quoted))
                    .unwrap_or_else(|| {
                        if quoted.image_message.is_set() {
                            "[Photo]".into()
                        } else if quoted.audio_message.is_set() {
                            "[Audio message]".into()
                        } else {
                            "[Message]".into()
                        }
                    })
            };
            content.quote = Some(Quote {
                sender: context.participant.clone().unwrap_or_default(),
                text: message_content::caption(&body, 256),
            });
        }
    }
    if let Some(reaction) = base.reaction_message.as_option()
        && let Some(target) = reaction.key.id.as_ref()
    {
        let emoji = reaction.text.as_deref().unwrap_or_default();
        // The local picker catalogue must not decide which received Unicode
        // survives. Keep future emoji/sequences intact, with bounded display data.
        if target.len() <= 256 && emoji.len() <= 256 && !emoji.chars().any(char::is_control) {
            content.reaction = Some(ReactionEvent {
                target: target.clone(),
                emoji: emoji.into(),
                at: reaction.sender_timestamp_ms.unwrap_or(0),
            });
        }
    }
}

/// Original quote identity, including legacy archived envelopes. SDK types stay here.
pub fn quoted_key(raw: &[u8]) -> Option<(String, String, String)> {
    let message = codec::message_decode(raw).ok()?;
    let context = message.get_base_message().context_info()?;
    let address = |value: Option<&str>| -> Option<String> {
        match value.filter(|s| !s.is_empty()) {
            Some(value) => Some(crate::protocol_messages::address(&value.parse().ok()?)),
            None => Some(String::new()),
        }
    };
    Some((
        context.stanza_id.clone().filter(|s| !s.is_empty())?,
        address(context.participant.as_deref())?,
        address(context.remote_jid.as_deref())?,
    ))
}
