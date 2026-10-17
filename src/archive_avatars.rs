use super::{Archive, Result, canonical, local_chat};
use crate::avatars::{MAX_DEMAND, Outcome, Thumbnail};
use rusqlite::{OptionalExtension, Transaction, params};
use std::sync::Arc;
use tokio::sync::watch;

// Private account data: intentionally no Debug implementation.
pub struct AvatarEntry {
    pub chat: i32,
    pub jid: String,
    pub generation: i64,
    pub picture_id: Option<String>,
    pub photo: Option<Arc<Thumbnail>>,
    pub has_photo: bool,
    pub retry_at: i64,
}

impl Archive {
    pub fn subscribe_avatars(&self) -> watch::Receiver<u64> {
        self.0.avatars_changed.subscribe()
    }
    fn avatars_changed(&self) {
        self.0
            .avatars_changed
            .send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Zero denotes the account avatar, even if no self-chat exists.
    pub async fn avatar_entries(
        &self,
        ids: Vec<i32>,
        with_pixels: bool,
    ) -> Result<Vec<AvatarEntry>> {
        self.call(move |db| {
            let mut entries = Vec::new();
            for chat in ids.into_iter().take(MAX_DEMAND) {
                let jid: Option<String> = if chat == 0 {
                    db.query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| r.get(0)).optional()?
                } else {
                    db.query_row("SELECT jid FROM chats WHERE id=?1", [local_chat(db, chat)?], |r| r.get(0)).optional()?
                };
                let Some(jid) = jid else { continue; };
                let cached = db.query_row(
                    "SELECT generation,picture_id,CASE WHEN ?2 THEN pixels END,retry_at,pixels IS NOT NULL FROM avatars WHERE jid=?1", params![jid,with_pixels], |r| {
                        let bytes: Option<Vec<u8>> = r.get(2)?;
                        Ok((r.get(0)?, r.get(1)?, bytes.and_then(Thumbnail::from_pixels).map(Arc::new), r.get(3)?,r.get(4)?))
                    },
                ).optional()?;
                let (generation,picture_id,photo,retry_at,has_photo) = cached.unwrap_or((0,None,None,0,false));
                entries.push(AvatarEntry {chat,jid,generation,picture_id,photo,retry_at,has_photo});
            }
            Ok(entries)
        }).await
    }

    /// Compare-and-save rejects downloads started before a picture notification
    /// or alias merge. A late result must never restore a removed/private photo.
    pub async fn save_avatar(
        &self,
        entry: AvatarEntry,
        outcome: Outcome,
        retry_at: i64,
    ) -> Result<()> {
        let changed = self.call(move |db| {
            let tx = db.transaction()?;
            if canonical(&tx, &entry.jid)? != entry.jid { return Ok(false); }
            tx.execute("INSERT OR IGNORE INTO avatars(jid) VALUES(?1)", [&entry.jid])?;
            let changed = match outcome {
                Outcome::Photo {id, photo} => tx.execute(
                    "UPDATE avatars SET picture_id=?3,pixels=?4,retry_at=?5,generation=generation+1 WHERE jid=?1 AND generation=?2",
                    params![entry.jid,entry.generation,id,photo.pixels.as_ref(),retry_at],
                )? > 0,
                Outcome::Absent => tx.execute(
                    "UPDATE avatars SET picture_id=NULL,pixels=NULL,retry_at=?3,generation=generation+1 WHERE jid=?1 AND generation=?2",
                    params![entry.jid,entry.generation,retry_at],
                )? > 0,
                Outcome::Unchanged => {
                    tx.execute("UPDATE avatars SET retry_at=?3 WHERE jid=?1 AND generation=?2",params![entry.jid,entry.generation,retry_at])?;
                    false
                }
            };
            tx.commit()?;
            Ok(changed)
        }).await?;
        if changed {
            self.avatars_changed();
        }
        Ok(())
    }

    pub async fn invalidate_avatar(&self, jid: String, removed: bool, stamp: i64) -> Result<()> {
        let changed = self.call(move |db| {
            let jid = canonical(db, &jid)?;
            Ok(db.execute(
                "INSERT INTO avatars(jid,generation,event_stamp,retry_at) VALUES(?1,1,?2,?3)
                 ON CONFLICT(jid) DO UPDATE SET generation=generation+1,pixels=NULL,picture_id=NULL,event_stamp=?2,retry_at=?3
                 WHERE avatars.event_stamp<=?2",
                params![jid,stamp,if removed {chrono::Utc::now().timestamp()+86400} else {0}],
            )? > 0)
        }).await?;
        if changed {
            self.avatars_changed();
        }
        Ok(())
    }
}

pub(super) fn merge(tx: &Transaction<'_>, lid: &str, pn: &str) -> Result<()> {
    // Keep the newer notification, including removals, across identity merges.
    // Bump the PN generation to fence work started before this transaction.
    tx.execute(
        "INSERT INTO avatars(jid,picture_id,pixels,event_stamp,retry_at)
        SELECT ?2,picture_id,pixels,event_stamp,retry_at FROM avatars WHERE jid=?1
        ON CONFLICT(jid) DO UPDATE SET picture_id=excluded.picture_id,pixels=excluded.pixels,
            event_stamp=excluded.event_stamp,retry_at=excluded.retry_at
        WHERE excluded.event_stamp>avatars.event_stamp",
        params![lid, pn],
    )?;
    tx.execute(
        "UPDATE avatars SET generation=generation+1,
        retry_at=CASE WHEN pixels IS NULL AND event_stamp>0 THEN retry_at ELSE 0 END WHERE jid=?1",
        [pn],
    )?;
    tx.execute("DELETE FROM avatars WHERE jid=?1", [lid])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{
        Batch,
        tests::{KEY, LID, PN, fixture},
    };
    fn photo(value: u8) -> Outcome {
        Outcome::Photo {
            id: format!("synthetic-{value}"),
            photo: Thumbnail::from_pixels(vec![value; 65536]).unwrap(),
        }
    }
    async fn entry(db: &Archive, chat: i32) -> AvatarEntry {
        db.avatar_entries(vec![chat], true).await.unwrap().remove(0)
    }
    #[tokio::test]
    async fn alias_merge_keeps_newer_removals_and_rejects_an_old_pn_download() {
        let (_temp, db) = fixture();
        let pn = db.create_chat(PN.into()).await.unwrap();
        db.create_chat(LID.into()).await.unwrap();
        db.save_avatar(entry(&db, pn).await, photo(7), 0)
            .await
            .unwrap();
        let late = entry(&db, pn).await;
        db.invalidate_avatar(LID.into(), true, 101).await.unwrap();
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        db.save_avatar(late, photo(99), 0).await.unwrap();
        assert!(!entry(&db, pn).await.has_photo);
        assert!(entry(&db, pn).await.retry_at > chrono::Utc::now().timestamp());
    }
    #[tokio::test]
    async fn cached_photos_migrate_reopen_and_survive_aliases_without_resurrecting_removals() {
        let temp = tempfile::tempdir().unwrap();
        let files =
            crate::storage::prepare_at(temp.path(), &crate::storage::tests::MemoryVault::default())
                .unwrap();
        let old = rusqlite::Connection::open(files.path.with_file_name("archive.db")).unwrap();
        old.execute_batch(KEY).unwrap();
        old.execute_batch(include_str!("archive_schema.sql"))
            .unwrap();
        old.execute_batch(include_str!("archive_contacts.sql"))
            .unwrap();
        old.execute(
            "INSERT INTO chats(jid,draft) VALUES(?1,'Synthetic saved draft')",
            [LID],
        )
        .unwrap();
        let chat = old.last_insert_rowid() as i32;
        old.pragma_update(None, "user_version", 2).unwrap();
        drop(old);
        let db = Archive::open(files.clone(), KEY).unwrap();
        assert_eq!(
            db.view(chat, 100, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .draft,
            "Synthetic saved draft"
        );
        db.save_avatar(entry(&db, chat).await, photo(7), 10)
            .await
            .unwrap();
        let late_lid = entry(&db, chat).await;
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        db.save_avatar(late_lid, photo(99), 10).await.unwrap();
        db.bind_account(PN.into(), "Synthetic owner".into())
            .await
            .unwrap();
        assert_eq!(entry(&db, 0).await.photo.unwrap().pixels[0], 7);
        let late = entry(&db, chat).await;
        db.invalidate_avatar(PN.into(), true, 100).await.unwrap();
        db.save_avatar(late, photo(99), 10).await.unwrap();
        db.invalidate_avatar(PN.into(), false, 99).await.unwrap();
        assert!(!entry(&db, chat).await.has_photo);
        assert!(entry(&db, chat).await.retry_at > chrono::Utc::now().timestamp());
        db.invalidate_avatar(PN.into(), false, 101).await.unwrap();
        assert_eq!(entry(&db, chat).await.retry_at, 0);
        db.save_avatar(entry(&db, chat).await, photo(17), 10)
            .await
            .unwrap();
        drop(db);
        let db = Archive::open(files, KEY).unwrap();
        assert_eq!(entry(&db, 0).await.photo.unwrap().pixels[0], 17);
        assert_eq!(entry(&db, chat).await.photo.unwrap().pixels.len(), 65536);
        db.blocking(|c| {c.execute_batch("CREATE TRIGGER fail_avatar BEFORE UPDATE ON avatars BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;")?;Ok(())}).unwrap();
        assert!(
            db.save_avatar(entry(&db, chat).await, photo(99), 20)
                .await
                .is_err()
        );
        assert_eq!(entry(&db, chat).await.photo.unwrap().pixels[0], 17);
    }
}
