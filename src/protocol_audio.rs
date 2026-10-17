//! SDK types and media credentials never leave the protocol boundary.
use crate::{
    audio::{self, Error, Source},
    protocol_media::Error as MediaError,
};
use std::sync::Arc;
use whatsapp_rust::{Client, prelude::MessageExt, waproto::codec};
pub struct Audio(pub Arc<Client>);

fn payload(raw: &[u8]) -> Result<whatsapp_rust::prelude::wa::message::AudioMessage, Error> {
    let message = codec::message_decode(raw).map_err(|_| Error::Unavailable)?;
    if message.is_view_once() {
        return Err(Error::Unavailable);
    }
    let clip = message
        .get_base_message()
        .audio_message
        .as_option()
        .ok_or(Error::Unavailable)?;
    if clip.view_once == Some(true) {
        return Err(Error::Unavailable);
    }
    if clip
        .file_length
        .is_some_and(|n| n > audio::MAX_BYTES as u64)
    {
        return Err(Error::TooLarge);
    }
    if [&clip.media_key, &clip.file_sha256, &clip.file_enc_sha256]
        .iter()
        .any(|v| v.as_ref().is_none_or(|v| v.len() != 32))
    {
        return Err(Error::Unavailable);
    }
    Ok(clip.clone())
}

#[async_trait::async_trait]
impl Source for Audio {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, Error> {
        let clip = payload(raw)?;
        crate::protocol_media::download(&self.0, &clip, audio::MAX_BYTES)
            .await
            .map_err(|state| match state {
                MediaError::TooLarge => Error::TooLarge,
                MediaError::Unavailable => Error::Unavailable,
                _ => Error::Download,
            })
    }
}
