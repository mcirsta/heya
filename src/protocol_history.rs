//! Protocol boundary for requesting phone history through the pinned public API.
use crate::phone_history::{self, Anchor, Failure, Source};
use std::sync::Arc;
use whatsapp_rust::{Client, wacore_binary::Jid};
pub struct History(pub Arc<Client>);
#[async_trait::async_trait]
impl Source for History {
    async fn fetch(&self, anchor: &Anchor) -> Result<String, Failure> {
        let jid: Jid = anchor.jid.parse().map_err(|_| Failure::Failed)?;
        self.0
            .fetch_message_history(
                &jid,
                &anchor.wire,
                anchor.outgoing,
                anchor.stamp,
                phone_history::BATCH,
            )
            .await
            .map_err(|_| Failure::Failed)
    }
}

/// The capture hook and completion event share these immutable compressed bytes.
/// Their digest correlates metadata without decoding twice or exposing payloads.
pub fn chunk_id(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}
