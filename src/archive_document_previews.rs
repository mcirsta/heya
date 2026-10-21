//! Encrypted document-preview cache with mutation/expiry and late-write guards.
use super::*;
use crate::document_previews::{MAX_BYTES, Request};
const ELIGIBLE: &str = "m.visible=1 AND m.deleted=0 AND (m.expires IS NULL OR m.expires>unixepoch()) AND json_type(nullif(m.content,''),'$.document')='object'";
impl Archive {
    pub async fn document_preview_request(&self, ids: Vec<i32>) -> Result<Option<Request>> {
        self.call(move |db| {
            let mut query = db.prepare_cached(&format!("SELECT m.raw FROM messages m LEFT JOIN document_previews p ON p.message_id=m.id WHERE m.id=?1 AND {ELIGIBLE} AND coalesce(p.sharp,0)=0 AND coalesce(p.retry_at,0)<=unixepoch()"))?;
            for id in ids.into_iter().take(crate::message_thumbnails::MAX_DEMAND) {
                if let Some(raw) = query.query_row([id], |r|r.get(0)).optional()? { return Ok(Some(Request {id,raw})); }
            }
            Ok(None)
        }).await
    }
    pub async fn document_preview_current(&self, request: &Request) -> Result<bool> {
        let (id, raw) = (request.id, request.raw.clone());
        self.call(move |db| Ok(db.query_row(&format!("SELECT EXISTS(SELECT 1 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE})"), params![id, raw], |r|r.get(0))?)).await
    }
    pub async fn save_document_preview(
        &self,
        request: &Request,
        preview: Option<Vec<u8>>,
    ) -> Result<bool> {
        if preview
            .as_ref()
            .is_some_and(|p| p.is_empty() || p.len() > MAX_BYTES)
        {
            return Err(Error::Limit);
        }
        let (id, raw) = (request.id, request.raw.clone());
        let changed = self.call(move |db| {
            if let Some(preview) = preview {
                let sharp = crate::document_previews::sharp_enough(&preview);
                // Replacing gets a fresh monotonic id: the presenter must swap
                // its texture even when an older low-resolution cache exists.
                Ok(db.execute(&format!("INSERT OR REPLACE INTO document_previews(message_id,preview,retry_at,sharp)
                    SELECT m.id,?3,unixepoch()+3600,?4 FROM messages m LEFT JOIN document_previews p ON p.message_id=m.id
                    WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE} AND coalesce(p.sharp,0)=0"), params![id,raw,preview,sharp])?>0)
            } else {
                // A failed upgrade retains usable old pixels and their revision.
                Ok(db.execute(&format!("INSERT INTO document_previews(message_id,retry_at)
                    SELECT m.id,unixepoch()+3600 FROM messages m WHERE m.id=?1 AND m.raw=?2 AND {ELIGIBLE}
                    ON CONFLICT(message_id) DO UPDATE SET retry_at=excluded.retry_at WHERE document_previews.sharp=0"), params![id,raw])?>0)
            }
        }).await?;
        if changed {
            self.changed();
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{KEY, fixture, record};
    #[tokio::test]
    async fn schema_nine_and_ten_upgrades_preserve_data_and_retry_old_previews() {
        let (_temp, db) = fixture();
        let mut r = record("synthetic-migration");
        r.content.document = Some(crate::message_content::Document {
            file_name: "Synthetic.pdf".into(),
            has_thumbnail: true,
            mime_type: "application/pdf".into(),
            size: None,
            pages: None,
        });
        db.ingest(Batch {
            messages: vec![r],
            ..Default::default()
        })
        .await
        .unwrap();
        let files = db.0._files.clone();
        db.blocking(|c| {
            c.execute_batch("DROP TRIGGER discard_changed_document_preview; DROP TABLE document_previews; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; DROP TABLE outgoing_receipts; DROP TABLE chat_settings; DROP INDEX messages_album; ALTER TABLE messages DROP COLUMN album_index; ALTER TABLE messages DROP COLUMN album; PRAGMA user_version=9; UPDATE chats SET draft='Keep synthetic draft';")?;Ok(())
        }).unwrap();
        drop(db);
        let db = Archive::open(files.clone(), KEY).unwrap();
        let id = db
            .blocking(|c| {
                assert_eq!(
                    c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
                    18
                );
                assert_eq!(
                    c.query_row("SELECT draft FROM chats", [], |r| r.get::<_, String>(0))?,
                    "Keep synthetic draft"
                );
                Ok(c.query_row("SELECT id FROM messages", [], |r| r.get::<_, i32>(0))?)
            })
            .unwrap();
        let request = db
            .document_preview_request(vec![id])
            .await
            .unwrap()
            .unwrap();
        db.save_document_preview(&request, Some(vec![1, 2, 3]))
            .await
            .unwrap();
        drop(db);
        let db = Archive::open(files.clone(), KEY).unwrap();
        assert!(
            db.document_preview_request(vec![id])
                .await
                .unwrap()
                .is_none()
        );
        db.blocking(|c| {
            assert_eq!(
                c.query_row("SELECT preview FROM document_previews", [], |r| r
                    .get::<_, Vec<u8>>(0))?,
                vec![1, 2, 3]
            );
            Ok(())
        })
        .unwrap();
        // Version 10 considered any saved image complete, including tiny HD
        // thumbnails. Keep those pixels visible but immediately try an upgrade.
        db.blocking(|c| {
            c.execute_batch("ALTER TABLE document_previews DROP COLUMN sharp; UPDATE document_previews SET retry_at=unixepoch()+3600; DROP TABLE history_pages; DROP TABLE history_requests; ALTER TABLE messages DROP COLUMN edit_version; ALTER TABLE mutations DROP COLUMN raw; DROP TABLE outgoing_receipts; DROP TABLE chat_settings; DROP INDEX messages_album; ALTER TABLE messages DROP COLUMN album_index; ALTER TABLE messages DROP COLUMN album; PRAGMA user_version=10;")?;
            Ok(())
        }).unwrap();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        assert!(
            db.document_preview_request(vec![id])
                .await
                .unwrap()
                .is_some()
        );
        db.blocking(|c| {
            assert_eq!(
                c.query_row("SELECT preview FROM document_previews", [], |r| r
                    .get::<_, Vec<u8>>(0))?,
                vec![1, 2, 3]
            );
            assert_eq!(
                c.query_row("SELECT draft FROM chats", [], |r| r.get::<_, String>(0))?,
                "Keep synthetic draft"
            );
            Ok(())
        })
        .unwrap();
    }
}
