//! Typed edit projection. Media references stay intact when only a caption changes.
use crate::message_actions::Prepared;
use whatsapp_rust::{
    prelude::{MessageBuilderExt, MessageExt, wa},
    waproto::{buffa::Message as _, codec},
};

pub struct Applied {
    pub message: Prepared,
    pub same_media: bool,
}

pub fn text(message: &wa::Message) -> Option<String> {
    if message.is_view_once() {
        return None;
    }
    let base = message.get_base_message();
    if let Some(photo) = base.image_message.as_option() {
        Some(format!(
            "[Photo] {}",
            photo.caption.as_deref().unwrap_or("")
        ))
    } else if base.document_message.is_set() {
        crate::protocol_content::extract(base).0
    } else if let Some(video) = base.video_message.as_option() {
        Some(format!(
            "[Video] {}",
            video.caption.as_deref().unwrap_or("")
        ))
    } else {
        base.text_content().map(str::to_owned)
    }
}

/// Legacy edits have only their text. New edits carry the typed envelope, which
/// can omit unchanged transfer fields. Preserve those fields and the old quote.
pub fn apply(original: &[u8], edited: &[u8], legacy: &str) -> Option<Applied> {
    let original = codec::message_decode(original).ok();
    if original.as_ref().is_some_and(MessageExt::is_view_once) {
        return None;
    }
    let old = original.as_ref().map(MessageExt::get_base_message);
    let incoming = if edited.is_empty() {
        None
    } else {
        let message = codec::message_decode(edited).ok()?;
        text(&message)?;
        Some(message)
    };
    let new = incoming.as_ref().map(MessageExt::get_base_message);
    let mut message;
    let same_media;
    // Compare the full transfer fields, not just display dimensions or filenames.
    // Caption/context changes must not invalidate a download already in flight.
    macro_rules! media {
        ($field:ident $(, $repeated:ident)*) => {{
            let previous = old?.$field.as_option()?;
            let mut next = previous.clone();
            if let Some(new) = new {
                let update = new.$field.as_option()?;
                next.merge_from_slice(&update.encode_to_vec()).ok()?;
                next.caption = update.caption.clone();
                if update.context_info.is_set() {
                    next.context_info = update.context_info.clone();
                }
                // An edit is a snapshot/delta, not an append to repeated fields.
                $(if !update.$repeated.is_empty() {
                    next.$repeated = update.$repeated.clone();
                })*
            } else {
                // Old releases discarded unsupported edit payloads. Recover the
                // attachment, but never present its original caption as current.
                next.caption = Some(if legacy == "[Unsupported edited message]" {
                    String::new()
                } else {
                    legacy.into()
                });
            }
            let mut before = previous.clone();
            let mut after = next.clone();
            before.caption = None;
            after.caption = None;
            before.context_info = Default::default();
            after.context_info = Default::default();
            same_media = before == after;
            message = old?.clone();
            message.$field = Some(next).into();
        }};
    }
    if old.is_some_and(|m| m.image_message.is_set()) {
        media!(
            image_message,
            interactive_annotations,
            scan_lengths,
            annotations
        );
    } else if old.is_some_and(|m| m.document_message.is_set()) {
        media!(document_message);
    } else if old.is_some_and(|m| m.video_message.is_set()) {
        media!(
            video_message,
            interactive_annotations,
            annotations,
            processed_videos
        );
    } else {
        // A text edit cannot replace a different attachment kind.
        if old.is_some_and(|m| {
            m.audio_message.is_set() || m.sticker_message.is_set() || m.protocol_message.is_set()
        }) || new.is_some_and(|m| {
            m.image_message.is_set() || m.document_message.is_set() || m.video_message.is_set()
        }) {
            return None;
        }
        message = new.cloned().unwrap_or_else(|| wa::Message::text(legacy));
        if message.context_info().is_none()
            && let Some(context) = old.and_then(MessageExt::context_info)
        {
            message.set_context_info(context.clone());
        }
        same_media = false;
    }
    let text = text(&message)?;
    let (_, content, thumbnail) = crate::protocol_content::extract(&message);
    Some(Applied {
        message: Prepared {
            raw: codec::message_to_vec(&message),
            text,
            content,
            thumbnail,
            visible: true,
        },
        same_media,
    })
}
