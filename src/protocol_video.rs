//! Authenticated video bytes; SDK envelopes and media keys stay in the adapter.
use crate::{
    protocol_media::Error as MediaError,
    video::{self, Error, Source},
};
use std::sync::Arc;
use whatsapp_rust::{Client, prelude::MessageExt, waproto::codec};

pub struct Video(pub Arc<Client>);
#[async_trait::async_trait]
impl Source for Video {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, Error> {
        let message = codec::message_decode(raw).map_err(|_| Error::Unavailable)?;
        if message.is_view_once() {
            return Err(Error::Unavailable);
        }
        let clip = message
            .get_base_message()
            .video_message
            .as_option()
            .ok_or(Error::Unavailable)?;
        if clip.view_once == Some(true) {
            return Err(Error::Unavailable);
        }
        if clip
            .file_length
            .is_some_and(|n| n > video::MAX_BYTES as u64)
        {
            return Err(Error::TooLarge);
        }
        if [&clip.media_key, &clip.file_sha256, &clip.file_enc_sha256]
            .iter()
            .any(|v| v.as_ref().is_none_or(|v| v.len() != 32))
        {
            return Err(Error::Unavailable);
        }
        crate::protocol_media::download(&self.0, clip, video::MAX_BYTES)
            .await
            .map_err(|e| match e {
                MediaError::TooLarge => Error::TooLarge,
                MediaError::Unavailable => Error::Unavailable,
                _ => Error::Download,
            })
    }
}
