//! WhatsApp poster selection: server thumbnail when supplied, otherwise decode
//! an ordinary video's initial portion. No playback or verified-original write.
use crate::{
    archive::Archive,
    document_previews::{self, Request},
    video,
};
use whatsapp_rust::{
    Client,
    download::{DownloadParams, MediaType},
    prelude::MessageExt,
    waproto::{codec, whatsapp::message::VideoMessage},
};

fn reference(video: &VideoMessage) -> Option<DownloadParams> {
    let path = video
        .thumbnail_direct_path
        .as_deref()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))?;
    DownloadParams::encrypted(
        path,
        video.media_key.as_deref()?,
        video.thumbnail_sha256.as_deref()?,
        video.thumbnail_enc_sha256.as_deref()?,
        0,
        MediaType::VideoThumbnail,
    )
    .ok()
}
fn generates(video: &VideoMessage) -> bool {
    video::SUPPORTED
        && video.view_once != Some(true)
        && video.gif_playback != Some(true)
        && video.file_length.is_some_and(|n| n > 0)
        && video.static_url.is_none()
        && video
            .direct_path
            .as_deref()
            .is_some_and(|p| p.starts_with('/') && !p.starts_with("//"))
        && [&video.media_key, &video.file_sha256, &video.file_enc_sha256]
            .iter()
            .all(|v| v.as_ref().is_some_and(|v| v.len() == 32))
}
async fn generated(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let pixels = crate::video_process::poster(bytes).await?;
    crate::photos::blocking(move || {
        let pixels = if pixels.width > 480 || pixels.height > 480 {
            let image = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba)?;
            let image = image::DynamicImage::ImageRgba8(image)
                .thumbnail(480, 480)
                .into_rgba8();
            crate::photos::Pixels {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            }
        } else {
            pixels
        };
        document_previews::encode(pixels)
    })
    .await
    .flatten()
}

pub async fn fetch(client: &Client, archive: &Archive, request: &Request) -> Option<Vec<u8>> {
    let message = codec::message_decode(&request.raw).ok()?;
    if message.is_view_once() {
        return None;
    }
    let video = message.get_base_message().video_message.as_option()?;
    if video.view_once == Some(true) {
        return None;
    }
    // The high-resolution server poster is full media: authenticate its own
    // hashes with the parent key and video-thumbnail HKDF context.
    if let Some(reference) = reference(video)
        && let Ok(bytes) =
            crate::protocol_media::download(client, &reference, document_previews::MAX_BYTES).await
    {
        let valid = crate::photos::blocking({
            let bytes = bytes.clone();
            move || document_previews::video_decode(&bytes).is_some()
        })
        .await
        .unwrap_or(false);
        if valid {
            return Some(bytes);
        }
    }
    // Match Web's ordinary-video generation gate; GIF-style videos keep their
    // supplied poster rather than starting a decoder before explicit playback.
    if !generates(video) {
        return None;
    }
    let source = video::Request {
        id: request.id,
        raw: request.raw.clone(),
    };
    if let Some(bytes) = archive.cached_video(&source).await.ok().flatten() {
        return generated(bytes).await;
    }
    // Web's 1.5-second retry is only for contexts without a chat. These requests
    // all originate from a conversation, so use its normal one-second request.
    let bytes = crate::protocol_media::video_prefix(client, video, 1.)
        .await
        .ok()?;
    generated(bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn video() -> VideoMessage {
        let mut v = VideoMessage::default();
        v.file_length = Some(1024 * 1024);
        v.seconds = Some(10);
        v.direct_path = Some("/synthetic-video".into());
        v.media_key = Some(vec![1; 32]);
        v.file_sha256 = Some(vec![2; 32]);
        v.file_enc_sha256 = Some(vec![3; 32]);
        v
    }
    #[test]
    fn supplied_video_thumbnails_have_their_own_path_hashes_and_crypto_context() {
        use whatsapp_rust::download::Downloadable;
        let mut v = video();
        v.thumbnail_direct_path = Some("/synthetic-poster".into());
        v.thumbnail_sha256 = Some(vec![4; 32]);
        v.thumbnail_enc_sha256 = Some(vec![5; 32]);
        let r = reference(&v).unwrap();
        assert_eq!(r.direct_path(), v.thumbnail_direct_path.as_deref());
        assert_eq!(r.file_sha256(), v.thumbnail_sha256.as_deref());
        assert_eq!(r.file_enc_sha256(), v.thumbnail_enc_sha256.as_deref());
        assert_eq!(r.app_info().app_info(), "WhatsApp Video Thumbnail Keys");
        assert_eq!(r.app_info().mms_type(), "thumbnail-video");
        v.gif_playback = Some(true);
        assert!(
            reference(&v).is_some(),
            "a GIF may still use its supplied poster"
        );
        assert!(
            !generates(&v),
            "Web excludes GIF-style videos from frame generation"
        );
        v.thumbnail_sha256 = Some(vec![0; 31]);
        assert!(reference(&v).is_none());
    }
    #[test]
    fn generation_excludes_view_once_gifs_unavailable_platforms_and_bad_references() {
        let mut v = video();
        assert_eq!(generates(&v), video::SUPPORTED);
        v.view_once = Some(true);
        assert!(!generates(&v));
        v = video();
        v.gif_playback = Some(true);
        assert!(!generates(&v));
        v = video();
        v.media_key = Some(vec![1; 31]);
        assert!(!generates(&v));
        v = video();
        v.direct_path = Some("//other.invalid/video".into());
        assert!(!generates(&v));
        v = video();
        v.static_url = Some("https://other.invalid/video".into());
        assert!(!generates(&v));
    }
}
