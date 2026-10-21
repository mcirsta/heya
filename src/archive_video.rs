//! One explicitly selected video, checked again after download and on mutation.
use super::*;
use crate::video::{MAX_BYTES, Request};

const ELIGIBLE: &str = "m.visible=1 AND m.deleted=0 AND length(m.raw)>0 AND (m.expires IS NULL OR m.expires>unixepoch()) AND json_type(nullif(m.content,''),'$.video')='object' AND NOT EXISTS(SELECT 1 FROM chats c JOIN chat_settings s ON s.jid=c.jid AND s.kind=2 AND s.value=1 WHERE c.id=m.chat_id)";
impl Archive {
    pub async fn video_request(&self, id: i32) -> Result<Option<Request>> {
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
    pub async fn cached_video(&self, request: &Request) -> Result<Option<Vec<u8>>> {
        let request = request.clone();
        self.call(move |db| Ok(db.query_row(
            &format!("SELECT v.original FROM message_videos v JOIN messages m ON m.id=v.message_id WHERE m.id=?1 AND m.raw=?2 AND length(v.original)>0 AND length(v.original)<=?3 AND {ELIGIBLE}"),
            params![request.id,request.raw,MAX_BYTES as i64], |r|r.get(0),
        ).optional()?)).await
    }
    pub async fn save_video(&self, request: &Request, bytes: Vec<u8>) -> Result<bool> {
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let request = request.clone();
        self.call(move |db| Ok(db.execute(
            &format!("INSERT INTO message_videos(message_id,original) SELECT m.id,?3 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE} ON CONFLICT(message_id) DO UPDATE SET original=excluded.original"),
            params![request.id,request.raw,bytes],
        )?>0)).await
    }
}
