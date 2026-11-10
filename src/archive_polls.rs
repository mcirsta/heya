//! Durable poll add-ons; decrypt once on ingestion, project bounded loaded polls.
use super::*;
use crate::{
    message_content::Content,
    polls::{Draft, Results, View, Voter},
};
use std::collections::HashMap;

fn identities(db: &Connection, source: &str, own: bool) -> Result<Vec<String>> {
    let base = if own || source == "#self" {
        db.query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .unwrap_or_default()
    } else {
        canonical(db, source)?
    };
    let mut result = Vec::new();
    if !source.is_empty() && source != "#self" && canonical(db, source)? == base {
        result.push(source.into());
    }
    if (base.ends_with("@s.whatsapp.net") || base.ends_with("@lid")) && !result.contains(&base) {
        result.push(base.clone());
    }
    let mut q = db.prepare_cached("SELECT alias FROM aliases WHERE canonical=?1 LIMIT 8")?;
    for value in q.query_map([base], |r| r.get::<_, String>(0))? {
        let value = value?;
        if !result.contains(&value) {
            result.push(value);
        }
    }
    Ok(result)
}
fn actor(db: &Connection, value: &str) -> Result<String> {
    if value == "#self" {
        return Ok(value.into());
    }
    let value = canonical(db, value)?;
    let own = db
        .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
            r.get::<_, String>(0)
        })
        .optional()?;
    Ok(if own.as_deref() == Some(&value) {
        "#self".into()
    } else {
        value
    })
}
pub(super) fn capture(db: &Connection, chat: i32, sender: &str, r: &Record) -> Result<()> {
    let Some(b) = &r.content.ballot else {
        if r.content.poll.is_some() {
            // A sent-message echo/history can recover the creator if shutdown
            // interrupted the send result. Keep an already captured namespace.
            if r.outgoing
                && r.sender != "#self"
                && identities(db, &r.sender, true)?
                    .first()
                    .is_some_and(|s| s == &r.sender)
            {
                db.execute("UPDATE messages SET source_sender=?3 WHERE chat_id=?1 AND wire_id=?2 AND sender='#self' AND deleted=0 AND visible=1 AND source_sender IN ('','#self')",params![chat,r.id,r.sender])?;
            }
            let old:Option<(i32,Vec<u8>)>=db.query_row("SELECT id,raw FROM messages WHERE chat_id=?1 AND sender=?2 AND wire_id=?3 AND deleted=0 AND (expires IS NULL OR expires>unixepoch())",params![chat,sender,r.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((id, old)) = old
                && let Some(raw) = crate::protocol_polls::merge_secret(&old, &r.raw)
            {
                let poll = crate::protocol_polls::from_raw(&raw).ok_or(Error::Invalid)?;
                db.execute("UPDATE messages SET raw=?2,content=json_set(content,'$.poll',json(?3)) WHERE id=?1",params![id,raw,serde_json::to_string(&poll).map_err(|_|Error::Invalid)?])?;
                db.execute("UPDATE poll_events SET state=0 WHERE target=?2 AND state=2 AND event_id IN (SELECT id FROM messages WHERE chat_id=?1)",params![chat,r.id])?;
            }
            apply(db, chat, &r.id)?;
        }
        if let Some(change) = &r.change
            && change.text.is_none()
        {
            cleanup(db)?;
        }
        return Ok(());
    };
    let id = db.query_row(
        "SELECT id FROM messages WHERE chat_id=?1 AND sender=?2 AND wire_id=?3",
        params![chat, sender, r.id],
        |row| row.get::<_, i32>(0),
    )?;
    if b.hashes.is_some() {
        // History's plaintext add-on can fill an earlier opaque encrypted event.
        db.execute(
            "UPDATE messages SET content=?2 WHERE id=?1 AND visible=0 AND deleted=0",
            params![id, encode_content(&r.content)?],
        )?;
    }
    let stamp = if b.stamp > 0 {
        b.stamp
    } else {
        r.timestamp.saturating_mul(1000)
    };
    db.execute("INSERT INTO poll_events(event_id,target,author,version) VALUES(?1,?2,?3,?4) ON CONFLICT(event_id) DO UPDATE SET state=CASE WHEN ?5 THEN 0 ELSE poll_events.state END",params![id,b.target,b.author,stamp,b.hashes.is_some()])?;
    apply(db, chat, &b.target)
}

pub(super) fn apply(db: &Connection, chat: i32, target: &str) -> Result<()> {
    let mut p=db.prepare_cached("SELECT id,sender,source_sender,outgoing,raw,expires FROM messages WHERE chat_id=?1 AND wire_id=?2 AND visible=1 AND deleted=0 AND (expires IS NULL OR expires>unixepoch()) AND json_type(nullif(content,''),'$.poll')='object' LIMIT 8")?;
    let parents = p
        .query_map(params![chat, target], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, Option<i64>>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if parents.is_empty() {
        cleanup(db)?;
        return Ok(());
    }
    let mut q=db.prepare_cached("SELECT e.event_id,e.author,m.source_sender,m.outgoing,m.raw,m.content FROM poll_events e JOIN messages m ON m.id=e.event_id WHERE m.chat_id=?1 AND e.target=?2 AND (e.state=0 OR (e.poll_id IS NULL AND e.state=1)) AND m.deleted=0")?;
    let events = q
        .query_map(params![chat, target], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, author, voter, own, raw, content) in events {
        let content: Content = serde_json::from_str(&content).unwrap_or_default();
        let ballot = content.ballot.unwrap_or_default();
        if !ballot.remote.is_empty()
            && ballot.remote.ends_with("@g.us")
            && canonical(db, &ballot.remote)?
                != db.query_row("SELECT jid FROM chats WHERE id=?1", [chat], |r| {
                    r.get::<_, String>(0)
                })?
        {
            db.execute("UPDATE poll_events SET state=2 WHERE event_id=?1", [id])?;
            continue;
        }
        let voters = identities(db, &voter, own)?;
        let mut valid = None;
        for (poll, sender, creator, outgoing, parent, expires) in &parents {
            if !author.is_empty() && actor(db, &author)? != actor(db, sender)? {
                continue;
            }
            let Some(definition) = crate::protocol_polls::from_raw(parent) else {
                continue;
            };
            let selected = if let Some(hashes) = &ballot.hashes {
                crate::protocol_polls::indices(&definition, hashes)
            } else {
                crate::protocol_polls::decrypt(
                    parent,
                    &raw,
                    target,
                    &identities(db, creator, *outgoing)?,
                    &voters,
                )
            };
            if let Some(selected) = selected {
                valid = Some((*poll, selected, *expires));
                break;
            }
        }
        if let Some((poll, selected, expires)) = valid {
            db.execute(
                "UPDATE poll_events SET poll_id=?2,selection=?3,state=1 WHERE event_id=?1",
                params![
                    id,
                    poll,
                    serde_json::to_string(&selected).map_err(|_| Error::Invalid)?
                ],
            )?;
            if let Some(expires) = expires {
                db.execute("UPDATE messages SET expires=?2 WHERE id=?1 AND (expires IS NULL OR expires>?2)",params![id,expires])?;
            }
        } else {
            db.execute(
                "UPDATE poll_events SET state=2,poll_id=?2 WHERE event_id=?1",
                params![
                    id,
                    if parents.len() == 1 {
                        Some(parents[0].0)
                    } else {
                        None
                    }
                ],
            )?;
        }
    }
    Ok(())
}
pub(super) fn cleanup(db: &Connection) -> Result<()> {
    let removed = "SELECT e.event_id FROM poll_events e JOIN messages v ON v.id=e.event_id JOIN messages p ON p.chat_id=v.chat_id AND p.wire_id=e.target WHERE (p.deleted=1 OR (p.expires IS NOT NULL AND p.expires<=unixepoch()) OR EXISTS(SELECT 1 FROM message_hides h WHERE h.chat_id=p.chat_id AND h.sender=p.sender AND h.wire_id=p.wire_id)) AND (e.poll_id=p.id OR (e.poll_id IS NULL AND (e.author='' OR e.author=p.sender OR e.author=p.source_sender)))";
    db.execute(&format!("UPDATE messages SET raw=X'',content='',deleted=1,delivery=CASE WHEN delivery='queued' THEN 'failed' ELSE delivery END WHERE id IN ({removed}) AND (length(raw)>0 OR content<>'')"),[])?;
    db.execute(
        &format!("DELETE FROM poll_events WHERE event_id IN ({removed})"),
        [],
    )?;
    Ok(())
}
pub(super) fn aliases_changed(db: &Connection, lid: &str, pn: &str) -> Result<()> {
    let mut q=db.prepare("SELECT DISTINCT v.chat_id,e.target FROM poll_events e JOIN messages v ON v.id=e.event_id LEFT JOIN messages p ON p.id=e.poll_id WHERE (e.state=2 OR e.poll_id IS NULL) AND ((e.state=1 AND e.poll_id IS NULL) OR e.author IN (?1,?2) OR v.chat_id IN (SELECT id FROM chats WHERE jid=?2) OR v.source_sender IN (?1,?2) OR v.sender=?2 OR p.source_sender IN (?1,?2) OR p.sender=?2 OR ((v.outgoing=1 OR p.outgoing=1) AND ?2=(SELECT value FROM metadata WHERE key='identity')))")?;
    let affected = q
        .query_map(params![lid, pn], |r| {
            Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (chat, target) in affected {
        db.execute("UPDATE poll_events SET state=0 WHERE target=?2 AND event_id IN (SELECT id FROM messages WHERE chat_id=?1)",params![chat,target])?;
        apply(db, chat, &target)?;
    }
    Ok(())
}

pub(super) fn merge_events(
    db: &Connection,
    column: &str,
    from: &dyn rusqlite::ToSql,
    to: &dyn rusqlite::ToSql,
) -> Result<()> {
    assert!(matches!(column, "chat_id" | "sender"));
    let chat = if column == "chat_id" {
        "?2"
    } else {
        "loser.chat_id"
    };
    let sender = if column == "sender" {
        "?2"
    } else {
        "loser.sender"
    };
    db.execute(&format!("INSERT INTO poll_events(event_id,target,author,version,poll_id,selection,state)
        SELECT winner.id,e.target,e.author,e.version,NULL,e.selection,e.state FROM messages loser
        JOIN messages winner ON winner.chat_id={chat} AND winner.sender={sender} AND winner.wire_id=loser.wire_id
        JOIN poll_events e ON e.event_id=loser.id WHERE loser.{column}=?1 AND winner.id<>loser.id
        ON CONFLICT(event_id) DO UPDATE SET selection=coalesce(poll_events.selection,excluded.selection),state=CASE WHEN poll_events.state=1 OR excluded.state=1 THEN 1 ELSE 0 END"),[from,to])?;
    Ok(())
}

pub(super) struct Projection {
    views: HashMap<i32, View>,
}
impl Projection {
    pub(super) fn load(
        db: &Connection,
        chat: i32,
        limit: usize,
        boundary: (i64, i32),
    ) -> Result<Self> {
        let mut views = HashMap::new();
        let mut q=db.prepare("SELECT id,json_array_length(content,'$.poll.options'),json_extract(content,'$.poll.ready'),coalesce(json_extract(content,'$.poll.restriction'),''),json_extract(content,'$.poll.end'),outgoing,source_sender,delivery,(SELECT is_group FROM chats WHERE id=?1) FROM messages WHERE id IN (SELECT id FROM messages WHERE chat_id=?1 AND visible=1 AND (stamp,id)<=(?2,?3) ORDER BY stamp DESC,id DESC LIMIT ?4) AND deleted=0 AND json_type(nullif(content,''),'$.poll')='object'")?;
        for row in q.query_map(params![chat, boundary.0, boundary.1, limit as i64], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, i64>(1)? as usize,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, bool>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, bool>(8)?,
            ))
        })? {
            let (id, count, ready, restriction, end, own, sender, delivery, group) = row?;
            // Ordinary direct sends use our PN. Group creations must retain
            // the exact PN/LID namespace reported by the SDK or history.
            let known_creator = !own
                || (sender != "#self"
                    && !sender.is_empty()
                    && (!group
                        || identities(db, &sender, true)?
                            .first()
                            .is_some_and(|s| s == &sender)));
            let ended = end.is_some_and(|e| e <= chrono::Utc::now().timestamp());
            let status = if !restriction.is_empty() {
                restriction.clone()
            } else if ended {
                "Poll ended".into()
            } else if !ready {
                "Poll data is incomplete. Fetch older messages from your phone.".into()
            } else if !known_creator {
                if matches!(delivery.as_str(), "queued" | "sending") {
                    "Sending poll…".into()
                } else {
                    "Poll identity is unavailable. Fetch older messages from your phone.".into()
                }
            } else {
                String::new()
            };
            views.insert(
                id,
                View {
                    counts: vec![0; count.min(12)],
                    can_vote: ready && restriction.is_empty() && !ended && known_creator,
                    status,
                    ..Default::default()
                },
            );
        }
        if views.is_empty() {
            return Ok(Self { views });
        }
        let mut latest: HashMap<(i32, String), Vec<usize>> = HashMap::new();
        // Read only the newest valid/invalid and confirmed/pending ballot per
        // stored voter. Historical changes need no repeated identity lookups.
        let mut q=db.prepare("WITH latest AS (
            SELECT e.poll_id,m.sender,e.selection,m.outgoing,m.delivery,e.state,e.version,m.wire_id,m.id,
              row_number() OVER (PARTITION BY e.poll_id,m.sender,e.state,
                (m.outgoing=1 AND m.delivery NOT IN ('accepted','delivered','read'))
                ORDER BY e.version DESC,m.wire_id DESC,m.id DESC) AS position
            FROM poll_events e JOIN messages m ON m.id=e.event_id
            WHERE e.poll_id IN (SELECT id FROM messages WHERE chat_id=?1 AND visible=1
              AND (stamp,id)<=(?2,?3) ORDER BY stamp DESC,id DESC LIMIT ?4) AND m.deleted=0)
            SELECT poll_id,sender,selection,outgoing,delivery,state FROM latest
            WHERE position=1 ORDER BY version,wire_id,id")?;
        for row in q.query_map(params![chat, boundary.0, boundary.1, limit as i64], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i32>(5)?,
            ))
        })? {
            let (id, voter, selection, own, delivery, state) = row?;
            let Some(view) = views.get_mut(&id) else {
                continue;
            };
            if state != 1 {
                if view.status.is_empty() {
                    view.status = "Some votes could not be verified.".into();
                }
                continue;
            }
            let selected = selection
                .as_deref()
                .and_then(|s| serde_json::from_str::<Vec<usize>>(s).ok())
                .unwrap_or_default();
            let voter = actor(db, &voter)?;
            if voter == "#self" {
                view.selected = selected.clone();
                if own && matches!(delivery.as_str(), "queued" | "sending") {
                    view.busy = true;
                    view.status = "Sending vote…".into();
                } else if own && matches!(delivery.as_str(), "failed" | "uncertain") {
                    view.status = if delivery == "failed" {
                        "Vote failed · choose an option to retry."
                    } else {
                        "Vote unconfirmed · choose an option to retry."
                    }
                    .into();
                } else if !view.busy && view.status.starts_with("Vote ") {
                    view.status.clear();
                }
            }
            if !own || matches!(delivery.as_str(), "accepted" | "delivered" | "read") {
                latest.insert((id, voter), selected);
            }
        }
        for ((id, _), selected) in latest {
            if let Some(view) = views.get_mut(&id) {
                if !selected.is_empty() {
                    view.voters += 1;
                }
                for index in selected {
                    if let Some(count) = view.counts.get_mut(index) {
                        *count += 1;
                    }
                }
            }
        }
        Ok(Self { views })
    }
    pub(super) fn apply(&self, id: i32, content: &mut Content) {
        if let Some(p) = &mut content.poll {
            p.view = self.views.get(&id).cloned().unwrap_or_default();
        }
    }
}

impl Archive {
    pub async fn queue_poll(&self, chat: i32, id: String, draft: Draft, reply: i32) -> Result<()> {
        let prepared = self
            .call(move |db| {
                let chat = local_chat(db, chat)?;
                chat_settings::unlocked(db, chat)?;
                let source = if reply > 0 {
                    Some(actions::source(db, chat, reply)?)
                } else {
                    None
                };
                let owner = db
                    .query_row("SELECT value FROM metadata WHERE key='identity'", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?
                    .unwrap_or_default();
                crate::protocol_polls::prepare(&draft, source.as_ref(), &owner)
            })
            .await?;
        self.queue_media(chat, id, prepared, (reply > 0).then_some(reply), None)
            .await
    }
    pub async fn poll_creator(&self, chat: i32, wire: String, creator: String) -> Result<()> {
        self.call(move|db|{
            let chat=local_chat(db,chat)?;
            db.execute("UPDATE messages SET source_sender=?3 WHERE chat_id=?1 AND wire_id=?2 AND outgoing=1 AND deleted=0 AND json_type(nullif(content,''),'$.poll')='object' AND source_sender<>?3",params![chat,wire,creator])?;
            db.execute("UPDATE poll_events SET state=0 WHERE target=?2 AND state=2 AND event_id IN (SELECT id FROM messages WHERE chat_id=?1)",params![chat,wire])?;
            apply(db,chat,&wire)
        }).await?;
        self.changed();
        Ok(())
    }
    pub async fn queue_vote(
        &self,
        chat: i32,
        message: i32,
        id: String,
        option: usize,
    ) -> Result<()> {
        self.call(move|db|{
            let tx=db.transaction()?;let source=actions::source(&tx,chat,message)?;
            let p=crate::protocol_polls::from_raw(&source.raw).ok_or(Error::Invalid)?;
            let stamp=tx.query_row("SELECT stamp FROM messages WHERE id=?1",[message],|r|r.get::<_,i64>(0))?;
            let projection=Projection::load(&tx,source.chat,1,(stamp,message))?;
            let view=projection.views.get(&message).ok_or(Error::Invalid)?;
            if !view.can_vote || view.busy || option>=p.options.len(){return Err(Error::Invalid);}
            let mut selected=view.selected.clone();
            if let Some(index)=selected.iter().position(|i|*i==option){selected.remove(index);}else if p.selectable==1{selected=vec![option];}else{selected.push(option);}
            if selected.len()>p.selectable{return Err(Error::Invalid);}
            let creators=identities(&tx,&source.sender,source.outgoing)?;
            let creator=creators.first().ok_or(Error::Invalid)?;
            let voters=identities(&tx,"#self",true)?;
            let voter=voters.iter().find(|s|s.ends_with(if creator.ends_with("@lid"){"@lid"}else{"@s.whatsapp.net"})).ok_or(Error::Invalid)?;
            let previous:Option<i64>=tx.query_row("SELECT max(e.version) FROM poll_events e JOIN messages m ON m.id=e.event_id WHERE e.poll_id=?1 AND m.sender='#self'",[message],|r|r.get(0))?;
            let stamp=chrono::Utc::now().timestamp_millis().max(previous.unwrap_or(0).saturating_add(1));
            let prepared=crate::protocol_polls::vote(&source,&selected,creator,voter,stamp)?;
            let r=Record{chat:source.jid.clone(),sender:voter.clone(),id:id.clone(),timestamp:stamp/1000,outgoing:true,sender_name:String::new(),text:String::new(),raw:prepared.raw,content:prepared.content,album:None,thumbnail:Vec::new(),visible:false,expires:None,change:None};
            insert_record(&tx,r,false)?;
            tx.execute("UPDATE messages SET delivery='queued' WHERE chat_id=?1 AND sender='#self' AND wire_id=?2",params![source.chat,id])?;
            tx.commit()?;Ok(())
        }).await?;
        self.changed();
        Ok(())
    }
    pub fn repair_polls(&self) -> Result<()> {
        if self.blocking(|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM metadata WHERE key='poll_decoder_revision' AND value='1')",[],|r|r.get::<_,bool>(0))?))? {return Ok(());}
        let mut after = 0;
        loop {
            let last=self.blocking(|db|{
                let tx=db.transaction()?;
                let mut q=tx.prepare("SELECT id,chat_id,sender,wire_id,stamp,outgoing,source_chat,source_sender,raw,content FROM messages WHERE id>?1 AND deleted=0 AND json_type(nullif(content,''),'$.ballot')='object' ORDER BY id LIMIT 100")?;
                let rows=q.query_map([after],|r|Ok((r.get::<_,i32>(0)?,r.get::<_,i32>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,bool>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,Vec<u8>>(8)?,r.get::<_,String>(9)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;drop(q);
                let last=rows.last().map(|r|r.0);
                for (_,chat,sender,wire,stamp,own,jid,original,raw,content) in rows {
                    capture(&tx,chat,&sender,&Record{chat:jid,sender:original,id:wire,timestamp:stamp,outgoing:own,sender_name:String::new(),text:String::new(),raw,content:serde_json::from_str(&content).unwrap_or_default(),album:None,thumbnail:Vec::new(),visible:false,expires:None,change:None})?;
                }
                if last.is_none(){cleanup(&tx)?;tx.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('poll_decoder_revision','1')",[])?;}
                tx.commit()?;Ok(last)
            })?;
            if let Some(id) = last {
                after = id;
            } else {
                break;
            }
        }
        self.changed();
        Ok(())
    }
    pub async fn poll_results(
        &self,
        chat: i32,
        message: i32,
        option: usize,
        limit: usize,
    ) -> Result<Results> {
        self.call(move|db|{
            let source=actions::source(db,chat,message)?;
            let p=crate::protocol_polls::from_raw(&source.raw).ok_or(Error::Invalid)?;
            if option>=p.options.len() || !p.restriction.is_empty(){return Err(Error::Invalid);}
            let stamp=db.query_row("SELECT stamp FROM messages WHERE id=?1",[message],|r|r.get::<_,i64>(0))?;
            let projection=Projection::load(db,source.chat,1,(stamp,message))?;
            let counts=projection.views.get(&message).map(|v|v.counts.clone()).unwrap_or_default();
            let mut q=db.prepare("WITH latest AS (
                SELECT m.sender,m.sender_name,e.selection,row_number() OVER(PARTITION BY m.sender ORDER BY e.version DESC,m.wire_id DESC,m.id DESC) AS position
                FROM poll_events e JOIN messages m ON m.id=e.event_id WHERE e.poll_id=?1 AND e.state=1 AND m.deleted=0
                  AND (m.outgoing=0 OR m.delivery IN ('accepted','delivered','read')))
                SELECT a.sender,a.sender_name,c.saved_name,c.profile_name FROM latest a LEFT JOIN contacts c ON c.jid=a.sender
                WHERE position=1 AND EXISTS(SELECT 1 FROM json_each(a.selection) WHERE value=?2)
                ORDER BY coalesce(nullif(c.saved_name,''),nullif(c.profile_name,''),a.sender_name,a.sender)")?;
            let mut voters=Vec::new();let mut more=false;
            for row in q.query_map(params![message,option as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))? {
                let (jid,profile,saved,known)=row?;
                if voters.len()==limit.max(1){more=true;break;}
                let own=actor(db,&jid)?=="#self";
                let name=if own {"You".into()}else{crate::domain::participant_name(saved.as_deref().filter(|s|!s.is_empty()).or(known.as_deref().filter(|s|!s.is_empty())).unwrap_or(&profile),saved.as_ref().is_some_and(|s|!s.is_empty()),&jid)};
                voters.push(Voter{jid:if own{"#self".into()}else{jid},name,detail:String::new()});
            }
            Ok(Results{message,question:p.question,options:p.options,counts,option,voters,more})
        }).await
    }
}
