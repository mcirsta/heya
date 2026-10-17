//! Guard downloaded audio against edits, expiry, revocation and identity merges.
use super::*;
use crate::audio::{MAX_BYTES, Request};

const ELIGIBLE: &str = "m.visible=1 AND m.deleted=0 AND (m.expires IS NULL OR m.expires>unixepoch()) AND json_type(nullif(m.content,''),'$.audio')='object'";
impl Archive {
    pub async fn audio_request(&self, id: i32) -> Result<Option<Request>> {
        self.call(move |db| {
            Ok(db
                .query_row(
                    &format!("SELECT m.raw FROM messages m WHERE m.id=?1 AND {ELIGIBLE}"),
                    [id],
                    |r| Ok(Request { id, raw: r.get(0)? }),
                )
                .optional()?)
        })
        .await
    }
    pub async fn cached_audio(&self, request: &Request) -> Result<Option<Vec<u8>>> {
        let (id, raw) = (request.id, request.raw.clone());
        self.call(move |db| Ok(db.query_row(&format!("SELECT a.original FROM message_audio a JOIN messages m ON m.id=a.message_id WHERE m.id=?1 AND m.raw=?2 AND length(a.original)<=?3 AND {ELIGIBLE}"),params![id,raw,MAX_BYTES as i64],|r|r.get(0)).optional()?)).await
    }
    pub async fn save_audio(&self, request: &Request, bytes: Vec<u8>) -> Result<bool> {
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let (id, raw) = (request.id, request.raw.clone());
        self.call(move |db| Ok(db.execute(&format!("INSERT INTO message_audio(message_id,original) SELECT m.id,?3 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE} ON CONFLICT(message_id) DO UPDATE SET original=excluded.original"),params![id,raw,bytes])?>0)).await
    }
}
