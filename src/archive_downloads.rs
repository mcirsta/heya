//! Captured attachment identity and bounded reuse of encrypted originals.
use super::*;
use crate::downloads::{Kind, Request};
const ELIGIBLE: &str = "m.visible=1 AND m.deleted=0 AND length(m.raw)>0 AND (m.expires IS NULL OR m.expires>unixepoch()) AND (json_type(nullif(m.content,''),'$.photo')='object' OR json_type(nullif(m.content,''),'$.audio')='object' OR json_type(nullif(m.content,''),'$.document')='object' OR json_extract(nullif(m.content,''),'$.has_attachment')=1)";
impl Archive {
    pub async fn download_request(&self, chat: i32, id: i32) -> Result<Option<Request>> {
        self.call(move |db| {
            let chat = local_chat(db, chat)?;
            Ok(db
                .query_row(
                    &format!(
                        "SELECT m.raw FROM messages m WHERE m.id=?1 AND m.chat_id=?2 AND {ELIGIBLE}"
                    ),
                    params![id, chat],
                    |r| {
                        Ok(Request {
                            chat,
                            id,
                            raw: r.get(0)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
    }
    pub async fn download_cached(&self, request: &Request, kind: Kind) -> Result<Option<Vec<u8>>> {
        let table = match kind {
            Kind::Photo => "message_photos",
            Kind::Audio => "message_audio",
            _ => return Ok(None),
        };
        let (id, raw) = (request.id, request.raw.clone());
        self.call(move|db|Ok(db.query_row(&format!("SELECT c.original FROM {table} c JOIN messages m ON m.id=c.message_id WHERE m.id=?1 AND m.raw=?2 AND length(c.original)>0 AND length(c.original)<=?3 AND {ELIGIBLE}"),params![id,raw,crate::photos::MAX_BYTES as i64],|r|r.get(0)).optional()?)).await
    }
}
