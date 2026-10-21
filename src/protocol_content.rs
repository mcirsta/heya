//! Protocol adapter for sender-supplied preview metadata. Never fetches a website.
use crate::message_content::{self, Content, Preview};
use whatsapp_rust::prelude::{MessageExt, wa};

pub fn album(message: &wa::Message) -> Option<message_content::Album> {
    if message.is_view_once() {
        return None;
    }
    let mut current = message;
    // Context lives on the outer associated-child wrapper in modern albums,
    // or directly beside the media in older envelopes. Do not infer albums.
    for _ in 0..32 {
        if let Some(a) = current.message_context_info.message_association.as_option()
            && a.association_type == Some(wa::message_association::AssociationType::MEDIA_ALBUM)
            && let Some(parent) = a
                .parent_message_key
                .id
                .as_deref()
                .filter(|id| !id.is_empty())
        {
            return Some(message_content::Album {
                parent: parent.into(),
                index: a.message_index.and_then(|n| u32::try_from(n).ok()),
            });
        }
        let next = current
            .device_sent_message
            .as_option()
            .and_then(|m| m.message.as_option())
            .or_else(|| {
                current
                    .ephemeral_message
                    .as_option()
                    .and_then(|m| m.message.as_option())
            })
            .or_else(|| {
                current
                    .associated_child_message
                    .as_option()
                    .and_then(|m| m.message.as_option())
            });
        current = next?;
    }
    None
}

pub fn extract(message: &wa::Message) -> (Option<String>, Content, Vec<u8>) {
    let (text, mut content, thumbnail) = extract_body(message);
    if !message.is_view_once() && content.notice.is_none() {
        crate::protocol_actions::decorate(message, &mut content);
        let base = message.get_base_message();
        content.has_attachment = base.document_message.is_set()
            || base
                .video_message
                .as_option()
                .is_some_and(|v| v.view_once != Some(true))
            || base.sticker_message.is_set();
    }
    (text, content, thumbnail)
}

fn extract_body(message: &wa::Message) -> (Option<String>, Content, Vec<u8>) {
    if message.is_view_once() {
        return (None, Content::default(), Vec::new());
    }
    if let Some(pin) = message.get_base_message().pin_in_chat_message.as_option() {
        use message_content::Notice;
        use wa::message::pin_in_chat_message::Type;
        let notice = match pin.r#type {
            Some(Type::PIN_FOR_ALL) => Some(Notice::MessagePinned),
            Some(Type::UNPIN_FOR_ALL) => Some(Notice::MessageUnpinned),
            _ => None,
        };
        if let Some(notice) = notice {
            return (
                Some(notice.description().into()),
                Content {
                    notice: Some(notice),
                    ..Default::default()
                },
                Vec::new(),
            );
        }
    }
    if let Some((text, content)) = crate::protocol_business::parse(message) {
        return (Some(text), content, Vec::new());
    }
    let base = message.get_base_message();
    if let Some(log) = base.call_log_messsage.as_option() {
        let call = crate::protocol_calls::message(log);
        return (
            Some(call.label().into()),
            Content {
                notice: Some(message_content::Notice::Call(call)),
                ..Default::default()
            },
            Vec::new(),
        );
    }
    if let Some(video) = base.video_message.as_option() {
        if video.view_once == Some(true) {
            return (None, Content::default(), Vec::new());
        }
        let (width, height) = match (video.width, video.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 && w <= 65536 && h <= 65536 => (w, h),
            _ => (640, 360),
        };
        let thumbnail = video
            .jpeg_thumbnail
            .as_ref()
            .filter(|v| v.len() <= crate::message_thumbnails::MAX_ENCODED)
            .cloned()
            .unwrap_or_default();
        return (
            None,
            Content {
                body: Some(video.caption.clone().unwrap_or_default()),
                video: Some(message_content::Video {
                    width,
                    height,
                    seconds: video.seconds,
                    size: video.file_length,
                    gif: video.gif_playback.unwrap_or(false),
                }),
                ..Default::default()
            },
            thumbnail,
        );
    }
    if let Some(document) = base.document_message.as_option() {
        let thumbnail = document
            .jpeg_thumbnail
            .as_ref()
            .filter(|bytes| {
                !bytes.is_empty() && bytes.len() <= crate::message_thumbnails::MAX_ENCODED
            })
            .cloned()
            .unwrap_or_default();
        let caption = document.caption.clone().unwrap_or_default();
        let metadata = message_content::Document {
            file_name: document
                .file_name
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| document.title.as_deref().filter(|s| !s.trim().is_empty()))
                .unwrap_or("Document")
                .into(),
            mime_type: document.mimetype.clone().unwrap_or_default(),
            size: document.file_length,
            pages: document.page_count.filter(|n| *n > 0),
            has_thumbnail: !thumbnail.is_empty()
                || crate::protocol_document_previews::available(document),
        };
        return (
            Some(metadata.transcript(&caption)),
            Content {
                body: Some(caption),
                document: Some(metadata),
                ..Default::default()
            },
            thumbnail,
        );
    }
    if let Some(audio) = base.audio_message.as_option() {
        if audio.view_once == Some(true) {
            return (None, Content::default(), Vec::new());
        }
        return (
            None,
            Content {
                body: Some(String::new()),
                audio: Some(message_content::Audio {
                    seconds: audio.seconds,
                    voice: audio.ptt.unwrap_or(false),
                    size: audio.file_length,
                    waveform: audio
                        .waveform
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .take(64)
                        .map(|v| (*v).min(100))
                        .collect(),
                }),
                ..Default::default()
            },
            Vec::new(),
        );
    }
    if let Some(photo) = base.image_message.as_option() {
        let (width, height) = match (photo.width, photo.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 && w <= 65536 && h <= 65536 => (w, h),
            _ => (640, 480),
        };
        let thumbnail = photo
            .jpeg_thumbnail
            .as_ref()
            .filter(|bytes| bytes.len() <= crate::message_thumbnails::MAX_ENCODED)
            .cloned()
            .unwrap_or_default();
        return (
            None,
            Content {
                body: Some(photo.caption.clone().unwrap_or_default()),
                photo: Some(message_content::Photo {
                    width,
                    height,
                    size: photo.file_length,
                }),
                ..Default::default()
            },
            thumbnail,
        );
    }
    let Some(extended) = base.extended_text_message.as_option() else {
        return (None, Content::default(), Vec::new());
    };
    let title = message_content::caption(extended.title.as_deref().unwrap_or(""), 1024);
    let description = message_content::caption(extended.description.as_deref().unwrap_or(""), 2048);
    let thumbnail = extended
        .jpeg_thumbnail
        .as_ref()
        .filter(|bytes| bytes.len() <= crate::message_thumbnails::MAX_ENCODED)
        .cloned()
        .unwrap_or_default();
    let matched = extended.matched_text.as_deref().and_then(|value| {
        message_content::web_url(value).or_else(|| {
            // WhatsApp also supplies bare domains. Never reinterpret URI schemes.
            (!value.contains(':'))
                .then(|| message_content::web_url(&format!("https://{value}")))
                .flatten()
        })
    });
    let url = matched.or_else(|| message_content::first_link(base.text_content().unwrap_or("")));
    let Some(url) =
        url.filter(|_| !title.is_empty() || !description.is_empty() || !thumbnail.is_empty())
    else {
        return (None, Content::default(), Vec::new());
    };
    (
        None,
        Content {
            preview: Some(Preview {
                url,
                title,
                description,
                has_thumbnail: !thumbnail.is_empty(),
            }),
            ..Default::default()
        },
        thumbnail,
    )
}
