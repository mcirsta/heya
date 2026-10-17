//! Received-photo protocol boundary.
use crate::photos::{self, Source, State};
use std::sync::Arc;
use whatsapp_rust::{Client, prelude::MessageExt, waproto::codec};
pub struct Photos(pub Arc<Client>);
#[async_trait::async_trait]
impl Source for Photos {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, State> {
        let message = codec::message_decode(raw).map_err(|_| State::Unavailable)?;
        if message.is_view_once() {
            return Err(State::Unavailable);
        }
        let photo = message
            .get_base_message()
            .image_message
            .as_option()
            .ok_or(State::Unavailable)?;
        if photo
            .file_length
            .is_some_and(|n| n > photos::MAX_BYTES as u64)
        {
            return Err(State::TooLarge);
        }
        if [&photo.media_key, &photo.file_sha256, &photo.file_enc_sha256]
            .iter()
            .any(|v| v.as_ref().is_none_or(|v| v.len() != 32))
        {
            return Err(State::Unavailable);
        }
        // Ordinary received photos use authenticated CDN routes, not arbitrary
        // static URLs supplied by senders. No unauthenticated image fetching.
        if photo.static_url.is_some() {
            return Err(State::Unavailable);
        }
        crate::protocol_media::download(&self.0, photo, photos::MAX_BYTES)
            .await
            .map_err(|error| match error {
                crate::protocol_media::Error::Failed => State::Failed,
                crate::protocol_media::Error::TooLarge => State::TooLarge,
                crate::protocol_media::Error::Unavailable => State::Unavailable,
            })
    }
}
