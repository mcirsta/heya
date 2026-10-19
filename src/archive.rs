//! Heya's encrypted, account-scoped archive. No protocol or UI types live here.
use crate::{
    domain,
    storage::{self, SessionFiles},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

#[path = "archive_contacts.rs"]
mod contacts;
pub use contacts::Contact;
#[path = "archive_avatars.rs"]
mod avatars;
pub use avatars::AvatarEntry;
#[path = "archive_reads.rs"]
mod reads;
pub use reads::ReadUpdate;
#[path = "archive_read_sync.rs"]
mod read_sync;
pub use read_sync::{ReadMessage, ReadSync};
#[path = "archive_receipts.rs"]
mod receipts;
pub use receipts::{ReceiptJob, ReceiptKind};
#[path = "archive_actions.rs"]
mod actions;
#[path = "archive_attachments.rs"]
mod attachments;
#[path = "archive_audio.rs"]
mod audio;
#[path = "archive_controls.rs"]
mod controls;
#[path = "archive_document_previews.rs"]
mod document_previews;
#[path = "archive_downloads.rs"]
mod downloads;
#[path = "archive_edits.rs"]
mod edits;
#[path = "archive_history.rs"]
mod history;
#[path = "archive_navigation.rs"]
mod navigation;
#[path = "archive_photos.rs"]
mod photos;
#[path = "archive_repair.rs"]
mod repair;
#[path = "archive_timeline.rs"]
mod timeline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Database,
    Identity,
    Invalid,
    Limit,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Database => "The encrypted archive could not be read or saved.",
            Self::Identity => "This archive belongs to a different linked account.",
            Self::Invalid => "The requested chat or message is not available.",
            Self::Limit => "The history staging limit was reached.",
        })
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self::Database
    }
}
pub type Result<T> = std::result::Result<T, Error>;

// These records can contain private content. Deliberately omit Debug.
#[derive(Clone)]
pub struct Record {
    pub chat: String,
    pub sender: String,
    pub id: String,
    pub timestamp: i64,
    pub outgoing: bool,
    pub sender_name: String,
    pub text: String,
    pub raw: Vec<u8>,
    pub content: crate::message_content::Content,
    pub thumbnail: Vec<u8>,
    pub visible: bool,
    pub expires: Option<i64>,
    pub change: Option<Change>,
}
#[derive(Clone)]
pub struct Change {
    pub id: String,
    pub sender: String,
    pub text: Option<String>,
    pub version: i64,
    /// Opaque edited envelope; interpreted only by the protocol adapter.
    pub raw: Vec<u8>,
}
#[derive(Clone, Default)]
pub struct ChatMeta {
    pub jid: String,
    pub name: String,
    pub group: bool,
    pub read_only: Option<bool>,
    pub ephemeral: Option<u32>,
}
#[derive(Default)]
pub struct Batch {
    pub messages: Vec<Record>,
    pub chats: Vec<ChatMeta>,
    pub aliases: Vec<(String, String)>,
    pub profiles: Vec<(String, String)>,
    pub live: bool,
}
pub struct Outgoing {
    pub chat: i32,
    pub jid: String,
    pub id: String,
    pub text: String,
    pub ephemeral: u32,
    pub raw: Vec<u8>,
    pub control: Option<crate::message_actions::Control>,
}
#[derive(Clone)]
pub struct View {
    pub chats: Arc<[domain::ChatSummary]>,
    pub conversation: Option<domain::Conversation>,
    pub can_send: bool,
    pub history_failed: bool,
    pub phone_more: bool,
}

struct Inner {
    connection: Mutex<Connection>,
    timeline: Mutex<timeline::Cache>,
    probe: std::sync::OnceLock<Arc<crate::performance::Probe>>,
    changed: watch::Sender<u64>,
    avatars_changed: watch::Sender<u64>,
    // Also retain the session lock while archive work is queued or in flight.
    _files: Arc<SessionFiles>,
}
#[derive(Clone)]
pub struct Archive(Arc<Inner>);

impl Archive {
    pub fn open(files: Arc<SessionFiles>, key: &str) -> Result<Self> {
        let path = files.path.with_file_name("archive.db");
        storage::private_file(&path).map_err(|_| Error::Database)?;
        let mut db = Connection::open(path)?;
        db.execute_batch(key)?;
        let version: String = db.query_row("PRAGMA cipher_version", [], |r| r.get(0))?;
        if version.is_empty() {
            return Err(Error::Database);
        }
        db.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        })?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 15 {
            return Err(Error::Database);
        }
        if version == 0 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_schema.sql"))?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_contacts.sql"))?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_avatars.sql"))?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if version < 4 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_reads.sql"))?;
            tx.pragma_update(None, "user_version", 4)?;
            tx.commit()?;
        }
        if version < 5 {
            let tx = db.transaction()?;
            tx.execute_batch("ALTER TABLE messages ADD COLUMN content TEXT NOT NULL DEFAULT ''; ALTER TABLE messages ADD COLUMN thumbnail BLOB NOT NULL DEFAULT X'';")?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        if version < 6 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_photos.sql"))?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        if version < 7 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_audio.sql"))?;
            tx.pragma_update(None, "user_version", 7)?;
            tx.commit()?;
        }
        if version < 8 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_actions.sql"))?;
            tx.pragma_update(None, "user_version", 8)?;
            tx.commit()?;
        }
        if version < 9 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_read_sync.sql"))?;
            tx.pragma_update(None, "user_version", 9)?;
            tx.commit()?;
        }
        if version < 10 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_document_previews.sql"))?;
            tx.pragma_update(None, "user_version", 10)?;
            tx.commit()?;
        }
        if version < 11 {
            let tx = db.transaction()?;
            // Keep the old pixels visible while previously failed/tiny previews
            // get one immediate chance to use the PDF rendering path.
            tx.execute_batch("ALTER TABLE document_previews ADD COLUMN sharp INTEGER NOT NULL DEFAULT 0; UPDATE document_previews SET retry_at=0;")?;
            tx.pragma_update(None, "user_version", 11)?;
            tx.commit()?;
        }
        if version < 12 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_controls.sql"))?;
            tx.pragma_update(None, "user_version", 12)?;
            tx.commit()?;
        }
        if version < 13 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_edits.sql"))?;
            expire(&tx)?;
            edits::apply(&tx, None)?;
            tx.pragma_update(None, "user_version", 13)?;
            tx.commit()?;
        }
        if version < 14 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_history.sql"))?;
            tx.pragma_update(None, "user_version", 14)?;
            tx.commit()?;
        }
        if version < 15 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("archive_receipts.sql"))?;
            tx.pragma_update(None, "user_version", 15)?;
            tx.commit()?;
        }
        // Never resend something that may have reached the network before a crash.
        db.execute("UPDATE message_photos SET state=0 WHERE state=1", [])?;
        db.execute(
            "UPDATE messages SET delivery='uncertain' WHERE delivery='sending'",
            [],
        )?;
        timeline::install(&db)?;
        let (changed, _) = watch::channel(0);
        Ok(Self(Arc::new(Inner {
            connection: Mutex::new(db),
            timeline: Default::default(),
            probe: Default::default(),
            changed,
            avatars_changed: watch::channel(0).0,
            _files: files,
        })))
    }

    pub fn enable_measurements(&self, probe: Arc<crate::performance::Probe>) {
        let _ = self.0.probe.set(probe);
    }
    pub fn measurements(&self) -> Option<Arc<crate::performance::Probe>> {
        self.0.probe.get().cloned()
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.changed.subscribe()
    }
    pub async fn needs_link(&self) -> Result<bool> {
        self.call(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='needs_link' AND value='1')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
    }
    pub async fn set_needs_link(&self, value: bool) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            tx.execute("INSERT INTO metadata(key,value) VALUES('needs_link',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [if value {"1"} else {"0"}])?;
            if value {
                tx.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('contacts_clean','0'),('reads_clean','0')", [])?;
            }
            tx.commit()?;
            Ok(())
        }).await
    }
    pub fn blocking<T>(&self, operation: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut db = self.0.connection.lock().map_err(|_| Error::Database)?;
        operation(&mut db)
    }
    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let archive = self.clone();
        let probe = self.measurements();
        let queued = probe.as_ref().map(|_| std::time::Instant::now());
        tokio::task::spawn_blocking(move || {
            archive.blocking(|db| {
                let start = queued.map(|_| std::time::Instant::now());
                if let (Some(probe), Some(queued)) = (&probe, queued) {
                    probe.record(crate::performance::Metric::Queue, queued.elapsed());
                }
                let result = operation(db);
                if let (Some(probe), Some(start)) = (&probe, start) {
                    probe.record(crate::performance::Metric::ArchiveWork, start.elapsed());
                }
                result
            })
        })
        .await
        .map_err(|_| Error::Database)?
    }
    pub fn changed(&self) {
        self.0.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub async fn bind_account(&self, identity: String, profile_name: String) -> Result<()> {
        let changed = self
            .call(move |db| {
                let tx = db.transaction()?;
                let current: Option<String> = tx
                    .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
                        r.get(0)
                    })
                    .optional()?;
                if current.as_ref().is_some_and(|current| current != &identity) {
                    return Err(Error::Identity);
                }
                tx.execute(
                    "INSERT OR IGNORE INTO metadata(key,value) VALUES('identity',?1)",
                    [&identity],
                )?;
                // The owner's profile name is distinct from their phonebook entry
                // and historical push names. Keep it for self-chat while offline.
                let changed = tx.execute(
                    "INSERT INTO metadata(key,value) VALUES('profile_name',?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value<>excluded.value",
                    [profile_name],
                )? > 0;
                tx.commit()?;
                Ok(changed)
            })
            .await?;
        if changed {
            self.changed();
        }
        Ok(())
    }

    pub fn ingest_blocking(&self, batch: Batch) -> Result<()> {
        self.blocking(|db| {
            let tx = db.transaction()?;
            for (lid, pn) in batch.aliases {
                merge_alias(&tx, &lid, &pn)?;
            }
            for (jid, name) in batch.profiles {
                contacts::profile(&tx, &jid, &name)?;
            }
            for meta in batch.chats {
                upsert_chat(&tx, &meta)?;
            }
            for record in batch.messages {
                insert_record(&tx, record, batch.live)?;
            }
            expire(&tx)?;
            tx.commit()?;
            Ok(())
        })?;
        self.changed();
        Ok(())
    }
    pub async fn ingest(&self, batch: Batch) -> Result<()> {
        let archive = self.clone();
        tokio::task::spawn_blocking(move || archive.ingest_blocking(batch))
            .await
            .map_err(|_| Error::Database)?
    }

    pub async fn draft(&self, chat: i32, text: String) -> Result<()> {
        self.call(move |db| {
            let chat = local_chat(db, chat)?;
            db.execute("UPDATE chats SET draft=?2 WHERE id=?1", params![chat, text])?;
            Ok(())
        })
        .await
    }
    pub async fn create_chat(&self, jid: String) -> Result<i32> {
        let id = self
            .call(move |db| {
                let tx = db.transaction()?;
                let id = upsert_chat(
                    &tx,
                    &ChatMeta {
                        jid,
                        ..Default::default()
                    },
                )?;
                tx.commit()?;
                Ok(id)
            })
            .await?;
        self.changed();
        Ok(id)
    }
    /// The command actor serializes composer edits and sends. Its send command
    /// carries the complete latest draft, even if an earlier edit was dropped
    /// under backpressure. Clear that draft atomically with durable insertion.
    pub async fn queue(&self, chat: i32, id: String, text: String) -> Result<()> {
        if text.trim().is_empty() || text.len() > 64 * 1024 {
            return Err(Error::Invalid);
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            let chat = local_chat(&tx,chat)?;
            if Self::queue_reply(&tx,chat,id.clone(),text.clone())? {tx.commit()?;return Ok(());}
            let (read_only, ephemeral): (bool, u32) = tx.query_row(
                "SELECT read_only,ephemeral FROM chats WHERE id=?1", [chat], |r| Ok((r.get(0)?,r.get(1)?)))?;
            if read_only { return Err(Error::Invalid); }
            let now = chrono::Utc::now().timestamp();
            tx.execute("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,delivery,expires,source_chat,source_sender) VALUES(?1,'#self',?2,?3,1,'You',?4,X'',1,'queued',?5,(SELECT jid FROM chats WHERE id=?1),'#self')", params![chat,id,now,text, (ephemeral > 0).then_some(now + ephemeral as i64)])?;
            tx.execute("UPDATE chats SET draft='' WHERE id=?1", [chat])?;
            tx.commit()?; Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
    pub async fn claim_next(&self) -> Result<Option<Outgoing>> {
        let result = self.call(|db| {
            let tx = db.transaction()?;
            expire(&tx)?;
            tx.execute("UPDATE messages SET delivery='queued' WHERE delivery='uncertain' AND wire_id IN (SELECT event_wire FROM message_controls WHERE kind='hide' AND retry_at<=unixepoch())",[])?;
            let mut next = tx.query_row("SELECT m.chat_id,c.jid,m.wire_id,m.body,c.ephemeral,m.raw FROM messages m JOIN chats c ON c.id=m.chat_id WHERE m.delivery='queued' AND m.deleted=0 ORDER BY m.id LIMIT 1", [], |r| Ok(Outgoing {chat:r.get(0)?,jid:r.get(1)?,id:r.get(2)?,text:r.get(3)?,ephemeral:r.get(4)?,raw:r.get(5)?,control:None})).optional()?;
            if let Some(job)=&mut next {job.control=controls::payload(&tx,&job.id)?;}
            if let Some(next) = &next { tx.execute("UPDATE messages SET delivery='sending' WHERE chat_id=?1 AND sender='#self' AND wire_id=?2", params![next.chat,next.id])?; }
            tx.commit()?; Ok(next)
        }).await?;
        if result.is_some() {
            self.changed();
        }
        Ok(result)
    }
    pub async fn delivery(&self, jid: String, id: String, status: &'static str) -> Result<()> {
        self.delivery_at(jid, id, status, None).await
    }
    pub async fn delivery_at(
        &self,
        jid: String,
        id: String,
        status: &'static str,
        sent_at: Option<i64>,
    ) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            let db = &tx;
            let jid = canonical(db, &jid)?;
            let chat: Option<i32> = db.query_row("SELECT id FROM chats WHERE jid=?1", [&jid], |r|r.get(0)).optional()?;
            // Offline queue time can precede the actual send by hours. Use the
            // server ACK's timestamp for the read range and displayed sent time.
            if status=="accepted" && let Some(stamp)=sent_at.filter(|s|*s>0) {
                db.execute("UPDATE messages SET expires=CASE WHEN expires IS NULL THEN NULL ELSE ?3+(expires-stamp) END,stamp=?3 WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 AND stamp<>?3 AND NOT EXISTS(SELECT 1 FROM message_controls WHERE event_wire=?2)", params![chat,id,stamp])?;
            }
            // Evidence may arrive before send() returns. Never downgrade it.
            let allowed = match status {
                "accepted" => "('queued','sending','uncertain','failed')",
                "delivered" => "('queued','sending','uncertain','failed','accepted')",
                "read" => "('queued','sending','uncertain','failed','accepted','delivered')",
                "failed" => "('queued','sending','uncertain')",
                _ => "('sending')",
            };
            db.execute(&format!("UPDATE messages SET delivery=?3 WHERE chat_id=(SELECT id FROM chats WHERE jid=?1) AND wire_id=?2 AND sender='#self' AND delivery IN {allowed}"), params![jid,id,status])?;
            if let Some(chat)=chat {controls::accepted(db,chat,&id,status)?;}
            db.execute("UPDATE reactions SET delivery=(SELECT delivery FROM messages WHERE chat_id=reactions.chat_id AND wire_id=reactions.event_wire AND sender='#self') WHERE actor='#self' AND chat_id=(SELECT id FROM chats WHERE jid=?1) AND event_wire=?2",params![jid,id])?;
            if matches!(status,"accepted"|"delivered"|"read") && let Some(chat)=chat {
                read_sync::sent(db,chat,&id)?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
    pub async fn recover_sends(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "UPDATE messages SET delivery='uncertain' WHERE delivery='sending'",
                [],
            )?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }

    pub async fn capture_history(&self, id: String, compressed: Vec<u8>) -> Result<()> {
        if compressed.len() > 16 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        self.call(move |db| {
            let tx = db.transaction()?;
            if tx
                .query_row(
                    "SELECT 1 FROM history_chunks WHERE id=?1",
                    [&id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
            {
                let pending: i64 = tx.query_row(
                    "SELECT coalesce(sum(length(data)),0) FROM history_chunks",
                    [],
                    |r| r.get(0),
                )?;
                if pending + compressed.len() as i64 > 256 * 1024 * 1024 {
                    return Err(Error::Limit);
                }
                tx.execute(
                    "INSERT INTO history_chunks(id,data) VALUES(?1,?2)",
                    params![id, compressed],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
        self.changed();
        Ok(())
    }
    pub fn next_history(&self) -> Result<Option<(String, Vec<u8>, usize)>> {
        self.blocking(|db| Ok(db.query_row("SELECT id,data,progress FROM history_chunks WHERE state='pending' ORDER BY rowid LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get::<_,i64>(2)? as usize))).optional()?))
    }
    pub fn history_progress(&self, id: &str, progress: usize, state: &str) -> Result<()> {
        self.blocking(|db| {
            db.execute("UPDATE history_chunks SET progress=?2,state=?3,data=CASE WHEN ?3='done' THEN NULL ELSE data END WHERE id=?1", params![id,progress as i64,state])?; Ok(())
        })?;
        self.changed();
        Ok(())
    }

    #[cfg(test)]
    pub async fn view(
        &self,
        selected: i32,
        limit: usize,
        query: String,
        unread: bool,
        mark_read: bool,
    ) -> Result<View> {
        self.view_window(selected, limit, (query, unread, mark_read), None)
            .await
    }

    pub async fn view_window(
        &self,
        selected: i32,
        limit: usize,
        filters: (String, bool, bool),
        end: Option<(i64, i32)>,
    ) -> Result<View> {
        let (query, unread, mark_read) = filters;
        let probe = self.measurements();
        let archive = self.clone();
        self.call(move |db| {
            let started=probe.as_ref().map(|_|std::time::Instant::now());
            expire(db)?;
            let selected=local_chat(db,selected)?;
            if mark_read {
                let tx = db.transaction()?;
                reads::local_read(&tx, selected)?;
                tx.commit()?;
            }
            let owner: Option<(String,String)> = db.query_row(
                "SELECT i.value,coalesce(p.value,'') FROM metadata i LEFT JOIN metadata p ON p.key='profile_name' WHERE i.key='identity'",
                [], |r| Ok((r.get(0)?,r.get(1)?)),
            ).optional()?;
            let mut stmt = db.prepare(include_str!("archive_sidebar.sql"))?;
            let rows = stmt.query_map([(domain::SUMMARY_CHARS + 1) as i64], |r| {
                let name: String = r.get(2)?;
                let jid: String = r.get(1)?;
                let title = if let Some((identity, profile)) = &owner && identity == &jid {
                    if profile.trim().is_empty() { "You".into() } else { format!("{profile} (You)") }
                } else if name.is_empty() { jid.split('@').next().unwrap_or(&jid).to_owned() } else { name };
                let group: bool = r.get(7)?;
                let outgoing: bool = r.get(8)?;
                let sender_name: String = r.get(9)?;
                let saved: bool = r.get(10)?;
                let sender: String = r.get(11)?;
                let sender_name = if sender_name.trim().is_empty() {
                    sender.split('@').next().unwrap_or(&sender).to_owned()
                } else if saved { sender_name } else { format!("~{sender_name}") };
                let notice = r.get::<_,Option<String>>(14)?.and_then(|s| serde_json::from_str::<crate::message_content::Notice>(&s).ok());
                let notice_text = notice.map(|n| n.label(if outgoing {"You"} else {&sender_name}));
                let preview = domain::chat_preview(
                    &r.get::<_, String>(4)?, r.get::<_,i32>(0)? == selected,
                    notice_text.as_deref().unwrap_or(&r.get::<_, String>(12)?),
                    r.get::<_, Option<String>>(13)?.as_deref(),
                    (group && !outgoing && notice.is_none()).then_some(sender_name.as_str()),
                );
                Ok(domain::ChatSummary {
                    id: r.get(0)?,
                    initials: initials(&title),
                    title,
                    tint: 0x91d6bc,
                    preview,
                    time: time(r.get(5)?),
                    unread: r.get::<_, u32>(3)? as usize,
                    marked_unread: r.get(6)?,
                })
            })?.collect::<std::result::Result<Vec<_>,_>>()?;
            let summary = rows.iter().find(|c| c.id == selected).cloned();
            let query = query.trim().to_lowercase();
            let chats = rows.into_iter().filter(|c| (!unread || c.unread > 0 || c.marked_unread) && c.title.to_lowercase().contains(&query)).collect();
            let mut can_send = false;
            let conversation = if let Some(summary) = summary {
                let (group,draft,read_only): (bool,String,bool) = db.query_row("SELECT is_group,draft,read_only FROM chats WHERE id=?1", [selected], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                can_send = !read_only;
                // The cache contains only the loaded timeline. Sidebar, draft,
                // reply, permissions and history state remain fresh on every view.
                let rows = archive.0.timeline.lock().map_err(|_|Error::Database)?
                    .load(db,selected,limit,group,end,probe.as_deref())?;
                Some(domain::Conversation {chat:summary,is_group:group,messages:rows.messages,draft,reply:actions::draft(db,selected)?,older_count:rows.total.saturating_sub(limit),transcript:rows.transcript})
            } else {
                archive.0.timeline.lock().map_err(|_|Error::Database)?.clear(db)?;
                None
            };
            let history_failed = db.query_row("SELECT EXISTS(SELECT 1 FROM history_chunks WHERE state='failed')", [], |r| r.get(0))?;
            let result=View {chats,conversation,can_send,history_failed,phone_more:end.is_none() && history::more(db,selected)?};
            if let (Some(probe),Some(started))=(probe,started) {probe.record(crate::performance::Metric::ViewBuild,started.elapsed());}
            Ok(result)
        }).await
    }
}

fn time(stamp: i64) -> String {
    if stamp == 0 {
        return String::new();
    }
    chrono::DateTime::from_timestamp(stamp, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%d %b %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
fn initials(title: &str) -> String {
    let mut letters = title
        .split(|c: char| c.is_whitespace() || c == '-')
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()));
    let Some(first) = letters.next() else {
        return "#".into();
    };
    let mut result: String = first.to_uppercase().collect();
    if let Some(last) = letters.next_back() {
        result.extend(last.to_uppercase());
    }
    result
}
fn delivery_label(status: &str, group: bool) -> &str {
    match status {
        "queued" => "Queued",
        "sending" => "Sending",
        "accepted" => "Sent",
        "delivered" if group => "Delivered (some)",
        "read" if group => "Read (some)",
        "delivered" => "Delivered",
        "read" => "Read",
        "failed" => "Failed",
        "uncertain" => "Unconfirmed · check phone",
        _ => "",
    }
}
fn canonical(db: &Connection, jid: &str) -> Result<String> {
    Ok(db
        .query_row("SELECT canonical FROM aliases WHERE alias=?1", [jid], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or_else(|| jid.into()))
}
fn upsert_chat(tx: &Transaction<'_>, meta: &ChatMeta) -> Result<i32> {
    let jid = canonical(tx, &meta.jid)?;
    tx.execute("INSERT INTO chats(jid,name,is_group,read_only,ephemeral) VALUES(?1,?2,?3,coalesce(?4,0),coalesce(?5,0)) ON CONFLICT(jid) DO UPDATE SET listed=1,name=CASE WHEN excluded.name<>'' THEN excluded.name ELSE chats.name END, read_only=coalesce(?4,chats.read_only),ephemeral=coalesce(?5,chats.ephemeral)", params![jid,meta.name,meta.group,meta.read_only,meta.ephemeral])?;
    Ok(tx.query_row("SELECT id FROM chats WHERE jid=?1", [jid], |r| r.get(0))?)
}
fn insert_record(tx: &Transaction<'_>, r: Record, live: bool) -> Result<()> {
    if r.id.is_empty() || r.chat.is_empty() || r.sender.is_empty() {
        return Err(Error::Invalid);
    }
    let chat = upsert_chat(
        tx,
        &ChatMeta {
            jid: r.chat.clone(),
            group: r.chat.ends_with("@g.us"),
            ..Default::default()
        },
    )?;
    let sender = if r.outgoing {
        "#self".into()
    } else {
        canonical(tx, &r.sender)?
    };
    if let Some(change) = r.change {
        let target_sender = canonical(tx, &change.sender)?;
        tx.execute("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted,raw) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET body=CASE WHEN mutations.deleted=1 THEN NULL ELSE excluded.body END,raw=CASE WHEN mutations.deleted=1 OR excluded.deleted=1 THEN X'' ELSE excluded.raw END,version=max(mutations.version,excluded.version),deleted=max(mutations.deleted,excluded.deleted) WHERE excluded.deleted=1 OR excluded.version>mutations.version", params![chat,target_sender,change.id,change.text,change.version,change.text.is_none(),change.raw])?;
    }
    let inserted = tx.execute("INSERT OR IGNORE INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,expires,delivery,source_chat,source_sender,content,thumbnail) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)", params![chat,sender,r.id,r.timestamp,r.outgoing,r.sender_name,r.text,r.raw,r.visible,r.expires,if r.outgoing {"accepted"} else {""},r.chat,r.sender,encode_content(&r.content)?,r.thumbnail])?;
    let inserted_row = tx.last_insert_rowid();
    if let Some(value) = &r.content.reaction {
        actions::reaction(
            tx,
            chat,
            &sender,
            &r.id,
            r.timestamp,
            if r.outgoing { "accepted" } else { "" },
            value,
        )?;
    }
    if r.outgoing {
        tx.execute("UPDATE messages SET expires=CASE WHEN expires IS NULL THEN NULL ELSE ?3+(expires-stamp) END,stamp=?3 WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 AND delivery IN ('queued','sending','uncertain','failed') AND NOT EXISTS(SELECT 1 FROM message_controls WHERE event_wire=?2)", params![chat,r.id,r.timestamp])?;
        let confirmed=tx.execute("UPDATE messages SET delivery='accepted' WHERE chat_id=?1 AND sender='#self' AND wire_id=?2 AND delivery IN ('queued','sending','uncertain','failed')", params![chat,r.id])?>0;
        if confirmed {
            controls::accepted(tx, chat, &r.id, "accepted")?;
            read_sync::sent(tx, chat, &r.id)?;
        }
    }
    if inserted > 0 {
        reads::inserted(tx, chat, inserted_row, live)?;
        if live {
            receipts::inserted(tx, inserted_row)?;
        }
    }
    apply_mutations(tx, chat)?;
    controls::hide_if_needed(tx, chat, &sender, &r.id)?;
    Ok(())
}
fn encode_content(content: &crate::message_content::Content) -> Result<String> {
    if *content == Default::default() {
        return Ok(String::new());
    }
    serde_json::to_string(content).map_err(|_| Error::Invalid)
}

fn apply_mutations(db: &Connection, chat: i32) -> Result<()> {
    edits::apply(db, Some(chat))
}
fn expire(db: &Connection) -> Result<()> {
    db.execute("UPDATE messages SET content='',thumbnail=X'',body='[Disappearing message expired]',raw=X'',deleted=1,delivery='' WHERE expires IS NOT NULL AND expires<=?1 AND deleted=0", [chrono::Utc::now().timestamp()])?;
    db.execute("UPDATE mutations SET body=NULL,raw=X'',deleted=1 WHERE EXISTS(SELECT 1 FROM messages m WHERE m.chat_id=mutations.chat_id AND m.sender=mutations.sender AND m.wire_id=mutations.wire_id AND m.deleted=1)", [])?;
    db.execute("UPDATE message_controls SET text='' WHERE CASE WHEN text<>'' THEN event_wire IN (SELECT wire_id FROM messages WHERE deleted=1 AND expires IS NOT NULL AND expires<=unixepoch()) ELSE 0 END",[])?;
    Ok(())
}

fn merge_alias(tx: &Transaction<'_>, lid: &str, pn: &str) -> Result<()> {
    if lid == pn {
        return Ok(());
    }
    let existing: Option<String> = tx
        .query_row("SELECT canonical FROM aliases WHERE alias=?1", [lid], |r| {
            r.get(0)
        })
        .optional()?;
    if let Some(existing) = existing {
        return if existing == pn {
            Ok(())
        } else {
            Err(Error::Identity)
        };
    }
    tx.execute(
        "INSERT OR IGNORE INTO aliases(alias,canonical) VALUES(?1,?2)",
        params![lid, pn],
    )?;
    contacts::merge(tx, lid, pn)?;
    avatars::merge(tx, lid, pn)?;
    actions::merge(tx, "actor", &lid, &pn)?;
    controls::merge(tx, "sender", &lid, &pn)?;
    move_records(tx, "sender", &lid, &pn)?;
    let old: Option<i32> = tx
        .query_row("SELECT id FROM chats WHERE jid=?1", [lid], |r| r.get(0))
        .optional()?;
    if let Some(old) = old {
        let target: Option<i32> = tx
            .query_row("SELECT id FROM chats WHERE jid=?1", [pn], |r| r.get(0))
            .optional()?;
        if let Some(target) = target {
            move_records(tx, "chat_id", &old, &target)?;
            // Joining distinct drafts must not attach their combined text to
            // an arbitrary quote. Retain the text and let the user reselect it.
            let conflicting_quotes: bool = tx.query_row(
                "SELECT EXISTS(
                SELECT 1 FROM reply_drafts a JOIN reply_drafts b
                ON a.chat_id=?1 AND b.chat_id=?2 WHERE a.message_id<>b.message_id)",
                params![old, target],
                |r| r.get(0),
            )?;
            tx.execute(
                "DELETE FROM reply_drafts WHERE chat_id IN (?1,?2) AND (?3 OR (
                (SELECT draft FROM chats WHERE id=?1)<>'' AND
                (SELECT draft FROM chats WHERE id=?2)<>'' AND
                (SELECT draft FROM chats WHERE id=?1)<>(SELECT draft FROM chats WHERE id=?2)))",
                params![old, target, conflicting_quotes],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO reply_drafts(chat_id,message_id)
                SELECT ?2,message_id FROM reply_drafts WHERE chat_id=?1 AND (
                (SELECT draft FROM chats WHERE id=?2)='' OR
                (SELECT draft FROM chats WHERE id=?1)=(SELECT draft FROM chats WHERE id=?2))",
                params![old, target],
            )?;
            tx.execute("UPDATE chats SET listed=max(listed,(SELECT listed FROM chats WHERE id=?1)),read_only=max(read_only,(SELECT read_only FROM chats WHERE id=?1)),ephemeral=CASE WHEN ephemeral=0 THEN (SELECT ephemeral FROM chats WHERE id=?1) ELSE ephemeral END,name=CASE WHEN name='' THEN (SELECT name FROM chats WHERE id=?1) ELSE name END,draft=CASE WHEN draft='' THEN (SELECT draft FROM chats WHERE id=?1) WHEN draft<>(SELECT draft FROM chats WHERE id=?1) AND (SELECT draft FROM chats WHERE id=?1)<>'' THEN draft||char(10)||(SELECT draft FROM chats WHERE id=?1) ELSE draft END WHERE id=?2", params![old,target])?;
            tx.execute(
                "UPDATE chat_redirects SET target=?2 WHERE target=?1",
                params![old, target],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO chat_redirects(id,target) VALUES(?1,?2)",
                params![old, target],
            )?;
            reads::merge(tx, old, target)?;
            read_sync::merge(tx, old, target)?;
            actions::merge(tx, "chat_id", &old, &target)?;
            controls::merge(tx, "chat_id", &old, &target)?;
            controls::settle_hides(tx, target)?;
            history::merge(tx, old, target)?;
            tx.execute("DELETE FROM chats WHERE id=?1", [old])?;
        } else {
            tx.execute("UPDATE chats SET jid=?2 WHERE id=?1", params![old, pn])?;
            reads::settle(tx, old)?;
            reads::recount(tx, old)?;
        }
    }
    // Sender aliases also affect group edits that arrived before their base.
    edits::apply(tx, None)?;
    Ok(())
}

fn local_chat(db: &Connection, id: i32) -> Result<i32> {
    Ok(db
        .query_row("SELECT target FROM chat_redirects WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or(id))
}

fn move_records(
    tx: &Transaction<'_>,
    column: &str,
    from: &dyn rusqlite::ToSql,
    to: &dyn rusqlite::ToSql,
) -> Result<()> {
    // Only the two fixed callers supply column names. Move non-colliding rows
    // in place to preserve local IDs, then reconcile evidence before deduping.
    let affected = if column == "sender" {
        tx.prepare("SELECT DISTINCT chat_id FROM messages WHERE sender=?1")?
            .query_map([from], |r| r.get::<_, i32>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let chat = if column == "chat_id" { "?2" } else { "chat_id" };
    let sender = if column == "sender" { "?2" } else { "sender" };
    tx.execute(
        &format!("UPDATE OR IGNORE messages SET {column}=?2 WHERE {column}=?1"),
        [from, to],
    )?;
    tx.execute(&format!("INSERT INTO messages(chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,deleted,expires,delivery,source_chat,source_sender,unread,seen,content,thumbnail)
        SELECT {chat},{sender},wire_id,stamp,outgoing,sender_name,body,raw,visible,deleted,expires,delivery,source_chat,source_sender,unread,seen,content,thumbnail FROM messages WHERE {column}=?1
        ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET
        body=CASE WHEN excluded.deleted=1 THEN excluded.body ELSE messages.body END,edit_version=-1,
        content=CASE WHEN excluded.deleted=1 OR messages.deleted=1 THEN '' WHEN messages.content='' THEN excluded.content ELSE messages.content END,
        thumbnail=CASE WHEN excluded.deleted=1 OR messages.deleted=1 THEN X'' WHEN messages.content='' THEN excluded.thumbnail ELSE messages.thumbnail END,
        raw=CASE WHEN excluded.deleted=1 OR messages.deleted=1 THEN X'' ELSE messages.raw END,
        deleted=max(messages.deleted,excluded.deleted), visible=max(messages.visible,excluded.visible), unread=CASE WHEN max(messages.seen,excluded.seen)=1 THEN 0 ELSE max(messages.unread,excluded.unread) END, seen=max(messages.seen,excluded.seen),
        expires=CASE WHEN messages.expires IS NULL THEN excluded.expires WHEN excluded.expires IS NULL THEN messages.expires ELSE min(messages.expires,excluded.expires) END,
        delivery=CASE WHEN instr('|queued|sending|failed|uncertain|accepted|delivered|read|','|'||excluded.delivery||'|')>instr('|queued|sending|failed|uncertain|accepted|delivered|read|','|'||messages.delivery||'|') THEN excluded.delivery ELSE messages.delivery END"), [from,to])?;
    // Keep a verified cached photo when exact duplicate envelopes coalesce.
    // A tombstone, mutation or different envelope cannot inherit its pixels.
    let target_chat = if column == "chat_id" {
        "?2"
    } else {
        "loser.chat_id"
    };
    let target_sender = if column == "sender" {
        "?2"
    } else {
        "loser.sender"
    };
    tx.execute(&format!("INSERT INTO message_photos(message_id,original,preview,state,revision)
        SELECT winner.id,p.original,p.preview,p.state,p.revision FROM messages loser
        JOIN messages winner ON winner.chat_id={target_chat} AND winner.sender={target_sender} AND winner.wire_id=loser.wire_id
        JOIN message_photos p ON p.message_id=loser.id
        WHERE loser.{column}=?1 AND winner.id<>loser.id AND winner.raw=loser.raw AND winner.content=loser.content
        AND winner.deleted=0 AND winner.visible=1 AND (winner.expires IS NULL OR winner.expires>unixepoch()) AND p.state=2
        ON CONFLICT(message_id) DO UPDATE SET original=excluded.original,preview=excluded.preview,state=2,revision=message_photos.revision+1 WHERE message_photos.state<>2"), [from,to])?;
    tx.execute(&format!("INSERT INTO message_audio(message_id,original)
        SELECT winner.id,a.original FROM messages loser
        JOIN messages winner ON winner.chat_id={target_chat} AND winner.sender={target_sender} AND winner.wire_id=loser.wire_id
        JOIN message_audio a ON a.message_id=loser.id
        WHERE loser.{column}=?1 AND winner.id<>loser.id AND winner.raw=loser.raw AND winner.content=loser.content
        AND winner.deleted=0 AND winner.visible=1 AND (winner.expires IS NULL OR winner.expires>unixepoch())
        ON CONFLICT(message_id) DO NOTHING"), [from,to])?;
    tx.execute(&format!("INSERT INTO outgoing_receipts(message_id,kind,state)
        SELECT winner.id,r.kind,r.state FROM messages loser
        JOIN messages winner ON winner.chat_id={target_chat} AND winner.sender={target_sender} AND winner.wire_id=loser.wire_id
        JOIN outgoing_receipts r ON r.message_id=loser.id WHERE loser.{column}=?1 AND winner.id<>loser.id
        ON CONFLICT(message_id,kind) DO UPDATE SET state=min(outgoing_receipts.state,excluded.state)"),[from,to])?;
    tx.execute(&format!("UPDATE reply_drafts SET message_id=(SELECT winner.id FROM messages loser JOIN messages winner ON winner.chat_id={target_chat} AND winner.sender={target_sender} AND winner.wire_id=loser.wire_id WHERE loser.id=reply_drafts.message_id AND loser.{column}=?1) WHERE message_id IN (SELECT id FROM messages WHERE {column}=?1)"),[from,to])?;
    tx.execute(&format!("DELETE FROM messages WHERE {column}=?1"), [from])?;
    tx.execute(
        &format!("UPDATE OR IGNORE mutations SET {column}=?2 WHERE {column}=?1"),
        [from, to],
    )?;
    tx.execute(&format!("INSERT INTO mutations(chat_id,sender,wire_id,body,version,deleted,raw)
        SELECT {chat},{sender},wire_id,body,version,deleted,raw FROM mutations WHERE {column}=?1
        ON CONFLICT(chat_id,sender,wire_id) DO UPDATE SET
        body=CASE WHEN mutations.deleted=1 OR excluded.deleted=1 THEN NULL WHEN excluded.version>mutations.version THEN excluded.body ELSE mutations.body END,
        raw=CASE WHEN mutations.deleted=1 OR excluded.deleted=1 THEN X'' WHEN excluded.version>mutations.version THEN excluded.raw ELSE mutations.raw END,
        version=max(mutations.version,excluded.version),deleted=max(mutations.deleted,excluded.deleted)"), [from,to])?;
    tx.execute(&format!("DELETE FROM mutations WHERE {column}=?1"), [from])?;
    for chat in affected {
        controls::settle_hides(tx, chat)?;
        reads::settle(tx, chat)?;
        reads::recount(tx, chat)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive_tests.rs"]
pub(crate) mod tests;
