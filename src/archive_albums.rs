//! Album indexing and viewer projection. No protocol decoding on a viewer click.
use super::*;
use crate::{message_content::Album, photos::Gallery};

impl Archive {
    /// Backfill only association columns: existing cached originals/previews and
    /// caption edits keep their identity. Each batch is restartable and atomic.
    pub fn repair_albums(&self, decode: impl Fn(&[u8]) -> Option<Album>) -> Result<()> {
        if self.blocking(|db| Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='album_decoder_revision' AND value='1')", [], |r|r.get::<_,bool>(0))?))? { return Ok(()); }
        let mut after = 0;
        loop {
            let last = self.blocking(|db| {
                let tx = db.transaction()?;
                let rows = tx.prepare("SELECT id,raw FROM messages WHERE id>?1 AND length(raw)>0 AND deleted=0 AND (expires IS NULL OR expires>unixepoch()) ORDER BY id LIMIT 100")?
                    .query_map([after], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
                for (id, raw) in &rows {
                    if let Some(album) = decode(raw) {
                        tx.execute("UPDATE messages SET album=?2,album_index=?3 WHERE id=?1 AND album IS NULL",params![id,album.parent,album.index])?;
                    }
                }
                if rows.is_empty() { tx.execute("INSERT OR REPLACE INTO metadata VALUES('album_decoder_revision','1')",[])?; }
                tx.commit()?;
                Ok(rows.last().map(|r|r.0))
            })?;
            match last {
                Some(id) => after = id,
                None => return Ok(()),
            }
        }
    }

    pub async fn photo_gallery(&self, chat: i32, id: i32) -> Result<Option<Gallery>> {
        self.call(move |db| {
            let chat = local_chat(db, chat)?;
            if chat_settings::unlocked(db, chat).is_err() { return Ok(None); }
            let selected = db.query_row(&format!("SELECT m.sender,m.album,m.body,m.stamp,m.outgoing,
                coalesce(nullif(trim(n.saved_name),''),nullif(trim(n.profile_name),''),m.sender_name),
                nullif(trim(n.saved_name),'') IS NOT NULL,c.is_group,m.content,m.wire_id,m.delivery,length(m.raw)
                FROM messages m JOIN chats c ON c.id=m.chat_id LEFT JOIN contacts n ON n.jid=m.sender
                WHERE m.id=?1 AND m.chat_id=?2 AND {}",photos::ELIGIBLE), params![id,chat], |r| {
                Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,bool>(4)?,r.get::<_,String>(5)?,r.get::<_,bool>(6)?,r.get::<_,bool>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,String>(10)?,r.get::<_,i64>(11)?))
            }).optional()?;
            let Some((author,album,body,stamp,outgoing,name,saved,group,content,wire,delivery,raw_len))=selected else { return Ok(None); };
            let members = if let Some(album) = album {
                db.prepare_cached(&format!("SELECT m.id FROM messages m WHERE m.chat_id=?1 AND m.sender=?2 AND m.album=?3 AND {} ORDER BY m.album_index IS NULL,m.album_index,m.stamp,m.id", photos::ELIGIBLE))?
                    .query_map(params![chat,author,album],|r|r.get::<_,i32>(0))?.collect::<std::result::Result<Vec<_>,_>>()?
            } else { photo_groups::members(db,chat,id,stamp)? };
            let Some(position)=members.iter().position(|&member|member==id) else { return Ok(None); };
            let mut content = serde_json::from_str::<crate::message_content::Content>(&content).unwrap_or_default();
            content.metadata = crate::message_actions::Metadata {wire:wire.clone(),stamp,delivery:delivery.clone(),..Default::default()};
            marks::project(db, chat, &author, &wire, &mut content.metadata)?;
            actions::Projection::load(db, chat, 1, (stamp,id))?.apply(db,&wire,&mut content)?;
            content.metadata.pending=controls::pending_one(db,chat,&author,&wire)?;
            content.unavailable = raw_len==0 && (!outgoing || !matches!(delivery.as_str(),"accepted"|"delivered"|"read"));
            let sender = if outgoing {"You".into()} else if group {domain::participant_name(&name,saved,&author)} else {name};
            let when=crate::message_time::local(stamp);
            let message=domain::Message {
                id,sender:sender.clone(),sender_id:author,text:body.clone(),
                time:when.map(|t|t.format("%H:%M").to_string()).unwrap_or_default(),date:when.map(|t|t.date_naive()),
                outgoing,local:false,content:Arc::new(content),
            };
            Ok(Some(Gallery {
                id,members,position,
                sender,
                date:when.map(|t|t.format("%d %b %Y, %H:%M").to_string()).unwrap_or_default(), caption:message.content.body.clone().unwrap_or(body),
                message:Some(message),
            }))
        }).await
    }

    /// Only small cached/embedded previews for the bounded filmstrip. Viewing an
    /// album never queues downloads of its other full-size photos.
    pub async fn album_thumbnails(
        &self,
        ids: Vec<i32>,
        known: std::collections::BTreeMap<i32, i64>,
    ) -> Result<Vec<(i32, i64, Vec<u8>)>> {
        self.call(move |db| {
            let mut query=db.prepare_cached(&format!("SELECT coalesce(p.revision,0),coalesce(nullif(p.preview,X''),m.thumbnail) FROM messages m LEFT JOIN message_photos p ON p.message_id=m.id WHERE m.id=?1 AND {} AND coalesce(p.revision,0)<>?2",photos::ELIGIBLE))?;
            let mut rows=Vec::new();
            for id in ids.into_iter().take(crate::photos::THUMB_COUNT) {
                if let Some((revision,bytes))=query.query_row(params![id,known.get(&id).copied().unwrap_or(-1)],|r|Ok((r.get(0)?,r.get(1)?))).optional()? {rows.push((id,revision,bytes));}
            }
            Ok(rows)
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::tests::{PN, fixture, record};
    #[tokio::test]
    async fn timeline_album_counts_cover_other_pages_and_refresh_after_changes() {
        let (_temp, db) = fixture();
        let records = (0..38)
            .map(|i| {
                let mut m = record(&format!("grid-{i}"));
                m.timestamp += i64::from(i);
                m.album = Some(Album {
                    parent: "grid-parent".into(),
                    index: Some(i),
                });
                m.content = crate::message_content::Content {
                    body: Some(String::new()),
                    photo: Some(crate::message_content::Photo {
                        width: 900,
                        height: 1200,
                        size: Some(1),
                    }),
                    ..Default::default()
                };
                m
            })
            .collect();
        db.ingest(Batch {
            messages: records,
            ..Default::default()
        })
        .await
        .unwrap();
        let chat = db.create_chat(PN.into()).await.unwrap();
        let page = db
            .view(chat, 2, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(page.messages.len(), 2);
        assert!(page.messages.iter().all(|m| m.content.album_count == 38));
        assert_eq!(
            crate::photo_albums::Layout::new(&page.messages, true).grids[&0].total,
            38
        );
        let gallery = db
            .photo_gallery(chat, page.messages[0].id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(gallery.members.len(), 38);
        db.call(|c| {
            c.execute(
                "UPDATE messages SET deleted=1,content='' WHERE wire_id='grid-0'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let page = db
            .view(chat, 2, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert!(page.messages.iter().all(|m| m.content.album_count == 37));
        // Album-only association repair must invalidate an already cached view.
        db.call(|c| {
            c.execute(
                "UPDATE messages SET album='another-parent' WHERE wire_id='grid-37'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let page = db
            .view(chat, 2, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert_eq!(
            page.messages[1].content.album.as_ref().unwrap().key,
            crate::photo_albums::Key::Explicit("another-parent".into())
        );
        assert!(
            crate::photo_albums::Layout::new(&page.messages, true)
                .grids
                .is_empty()
        );
    }
}
