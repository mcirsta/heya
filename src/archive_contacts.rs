use super::*;

// Private account data; intentionally no Debug implementation.
pub struct Contact {
    pub jid: String,
    pub name: String,
    pub timestamp: Option<i64>,
}
impl Contact {
    pub fn supersedes(&self, previous: &Self) -> bool {
        match (self.timestamp, previous.timestamp) {
            (Some(next), Some(old)) => next >= old,
            (None, Some(_)) => false,
            _ => true,
        }
    }
}

impl Archive {
    /// Mark before starting the SDK. An unclean exit requires a snapshot because
    /// the SDK may have committed its app-state cursor before our contact write.
    pub async fn begin_contacts(&self) -> Result<bool> {
        self.call(|db| {
            let tx = db.transaction()?;
            let clean: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key='contacts_clean' AND value='1')",
                [],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('contacts_clean','0')",
                [],
            )?;
            tx.commit()?;
            Ok(!clean)
        })
        .await
    }
    pub async fn finish_contacts(&self) -> Result<()> {
        self.call(|db| {
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('contacts_clean','1')",
                [],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn save_contacts(
        &self,
        contacts: Vec<Contact>,
        aliases: Vec<(String, String)>,
        replace: bool,
    ) -> Result<()> {
        self.call(move |db| {
            let tx = db.transaction()?;
            if replace {
                // Replace atomically only after a complete snapshot. A failed
                // fetch never removes the user's previously displayed names.
                tx.execute("UPDATE contacts SET saved_name='',saved_stamp=NULL WHERE saved_name IS NOT NULL", [])?;
            }
            for (lid, pn) in aliases { merge_alias(&tx, &lid, &pn)?; }
            for contact in contacts {
                let jid = canonical(&tx, &contact.jid)?;
                tx.execute("INSERT INTO contacts(jid,saved_name,saved_stamp) VALUES(?1,?2,?3)
                    ON CONFLICT(jid) DO UPDATE SET saved_name=excluded.saved_name,saved_stamp=excluded.saved_stamp
                    WHERE contacts.saved_stamp IS NULL OR excluded.saved_stamp>=contacts.saved_stamp",
                    params![jid, contact.name, contact.timestamp])?;
            }
            tx.commit()?;
            Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
}

pub(super) fn profile(tx: &Transaction<'_>, jid: &str, name: &str) -> Result<()> {
    if !name.trim().is_empty() {
        let jid = canonical(tx, jid)?;
        tx.execute(
            "INSERT INTO contacts(jid,profile_name) VALUES(?1,?2)
            ON CONFLICT(jid) DO UPDATE SET profile_name=excluded.profile_name",
            params![jid, name],
        )?;
    }
    Ok(())
}

pub(super) fn merge(tx: &Transaction<'_>, lid: &str, pn: &str) -> Result<()> {
    tx.execute("INSERT INTO contacts(jid,saved_name,saved_stamp,profile_name)
        SELECT ?2,saved_name,saved_stamp,profile_name FROM contacts WHERE jid=?1
        ON CONFLICT(jid) DO UPDATE SET
            saved_name=CASE WHEN contacts.saved_name IS NULL OR (excluded.saved_name IS NOT NULL AND (contacts.saved_stamp IS NULL OR excluded.saved_stamp>contacts.saved_stamp)) THEN excluded.saved_name ELSE contacts.saved_name END,
            saved_stamp=CASE WHEN contacts.saved_name IS NULL OR (excluded.saved_name IS NOT NULL AND (contacts.saved_stamp IS NULL OR excluded.saved_stamp>contacts.saved_stamp)) THEN excluded.saved_stamp ELSE contacts.saved_stamp END,
            profile_name=CASE WHEN contacts.profile_name='' THEN excluded.profile_name ELSE contacts.profile_name END", params![lid, pn])?;
    tx.execute("DELETE FROM contacts WHERE jid=?1", [lid])?;
    Ok(())
}

impl Archive {
    /// Resolve just the open peer and its known PN/LID aliases, off the UI thread.
    pub async fn presence_target(&self, selected: i32) -> Result<Option<crate::presence::Target>> {
        self.call(move |db| {
            let chat = local_chat(db, selected)?;
            let jid: Option<String> = db.query_row(
                "SELECT jid FROM chats WHERE id=?1 AND is_group=0 AND read_only=0 AND jid<>coalesce((SELECT value FROM metadata WHERE key='identity'),'')",
                [chat], |r| r.get(0),
            ).optional()?;
            let Some(jid) = jid.filter(|jid| jid.ends_with("@s.whatsapp.net") || jid.ends_with("@lid")) else { return Ok(None); };
            let mut addresses=vec![jid.clone()];
            let mut query=db.prepare("SELECT alias FROM aliases WHERE canonical=?1 LIMIT 8")?;
            for address in query.query_map([&jid],|r|r.get::<_,String>(0))? { addresses.push(address?); }
            Ok(Some(crate::presence::Target {chat:selected,jid,addresses}))
        }).await
    }
}
