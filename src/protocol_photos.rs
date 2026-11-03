//! Received-photo and static-sticker protocol boundary.
use crate::photos::{self, Source, State};
use std::sync::Arc;
use whatsapp_rust::{Client, download::Downloadable, prelude::MessageExt, waproto::codec};
pub struct Photos(pub Arc<Client>);
#[async_trait::async_trait]
impl Source for Photos {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, State> {
        let message = codec::message_decode(raw).map_err(|_| State::Unavailable)?;
        if message.is_view_once() {
            return Err(State::Unavailable);
        }
        let base = message.get_base_message();
        let (media, limit): (&dyn Downloadable, usize) =
            if let Some(photo) = base.image_message.as_option() {
                (photo, photos::MAX_BYTES)
            } else if let Some(sticker) = base
                .sticker_message
                .as_option()
                .filter(|s| s.is_animated != Some(true) && s.is_lottie != Some(true))
            {
                (sticker, 1024 * 1024)
            } else {
                return Err(State::Unavailable);
            };
        if media.file_length().is_some_and(|n| n > limit as u64) {
            return Err(State::TooLarge);
        }
        if [
            media.media_key(),
            media.file_sha256(),
            media.file_enc_sha256(),
        ]
        .iter()
        .any(|v| v.is_none_or(|v| v.len() != 32))
        {
            return Err(State::Unavailable);
        }
        // Received images use authenticated CDN routes, not arbitrary
        // static URLs supplied by senders. No unauthenticated image fetching.
        if media.static_url().is_some() {
            return Err(State::Unavailable);
        }
        crate::protocol_media::download(&self.0, media, limit)
            .await
            .map_err(|error| match error {
                crate::protocol_media::Error::Failed => State::Failed,
                crate::protocol_media::Error::TooLarge => State::TooLarge,
                crate::protocol_media::Error::Unavailable => State::Unavailable,
            })
    }
}
