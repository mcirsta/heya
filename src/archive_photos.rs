//! Transactional photo cache with guards against late writes after mutation.
use super::*;
use crate::photos::{Cached, Request, State};
use std::collections::BTreeMap;

pub(crate) struct Row {
    pub id: i32,
    pub state: State,
    pub revision: i64,
    pub bytes: Option<Vec<u8>>,
    pub original: Option<Vec<u8>>,
}
const ELIGIBLE: &str = "m.visible=1 AND m.deleted=0 AND (m.expires IS NULL OR m.expires>unixepoch()) AND json_type(nullif(m.content,''),'$.photo')='object'";

impl Archive {
    pub async fn photo_candidates(&self, ids: Vec<i32>) -> Result<Vec<(i32, State)>> {
        self.call(move |db| {
            let mut query = db.prepare_cached(&format!("SELECT coalesce(p.state,0) FROM messages m LEFT JOIN message_photos p ON p.message_id=m.id WHERE m.id=?1 AND {ELIGIBLE}"))?;
            let mut rows = Vec::new();
            for id in ids.into_iter().take(crate::photos::MAX_DEMAND+1) {
                if let Some(state) = query.query_row([id],|r|r.get::<_,i32>(0)).optional()? {
                    rows.push((id,State::from_db(state)));
                }
            }
            Ok(rows)
        }).await
    }
    pub(crate) async fn photo_rows(
        &self,
        ids: Vec<i32>,
        known: BTreeMap<i32, i64>,
        viewer: Option<(i32, Option<i64>)>,
    ) -> Result<Vec<Row>> {
        self.call(move |db| {
            let mut query = db.prepare_cached(&format!("SELECT coalesce(p.state,0),coalesce(p.revision,0),
                CASE WHEN coalesce(p.revision,0)=?2 THEN NULL ELSE CASE WHEN length(p.preview)>0 THEN p.preview ELSE m.thumbnail END END,
                CASE WHEN ?3 AND p.state=2 AND coalesce(p.revision,0)<>?4 THEN p.original ELSE NULL END
                FROM messages m LEFT JOIN message_photos p ON p.message_id=m.id WHERE m.id=?1 AND {ELIGIBLE}"))?;
            let mut rows = Vec::new();
            for id in ids.into_iter().take(crate::photos::MAX_DEMAND + 1) {
                let viewing = viewer.is_some_and(|v| v.0==id);
                if let Some(row) = query.query_row(params![id,known.get(&id).copied().unwrap_or(-1),viewing,viewer.and_then(|v|v.1).unwrap_or(-1)],|r| Ok(Row {
                    id,state:State::from_db(r.get(0)?),revision:r.get(1)?,bytes:r.get(2)?,original:r.get(3)?
                })).optional()? { rows.push(row); }
            }
            Ok(rows)
        }).await
    }
    pub async fn photo_request(&self, id: i32) -> Result<Option<Request>> {
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
    pub async fn reset_photo_downloads(&self) -> Result<()> {
        self.call(|db| {
            db.execute("UPDATE message_photos SET state=0 WHERE state=1", [])?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub async fn photo_state(&self, request: &Request, state: State) -> Result<bool> {
        let (id, raw) = (request.id, request.raw.clone());
        let changed = self.call(move |db| Ok(db.execute(&format!("INSERT INTO message_photos(message_id,state) SELECT m.id,?3 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE}
            ON CONFLICT(message_id) DO UPDATE SET state=excluded.state WHERE message_photos.state<>2 OR excluded.state=1"),params![id,raw,state as i32])?>0)).await?;
        if changed {
            self.changed();
        }
        Ok(changed)
    }
    pub async fn save_photo(&self, request: &Request, photo: Cached) -> Result<bool> {
        if photo.original.len() > crate::photos::MAX_BYTES
            || photo.preview.len() > crate::photos::MAX_BYTES
        {
            return Err(Error::Limit);
        }
        let (id, raw) = (request.id, request.raw.clone());
        let changed = self.call(move |db| Ok(db.execute(&format!("INSERT INTO message_photos(message_id,original,preview,state,revision)
            SELECT m.id,?3,?4,2,1 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE}
            ON CONFLICT(message_id) DO UPDATE SET original=excluded.original,preview=excluded.preview,state=2,revision=message_photos.revision+1"),params![id,raw,photo.original,photo.preview])?>0)).await?;
        if changed {
            self.changed();
        }
        Ok(changed)
    }
}
