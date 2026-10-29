//! Cached contact and shared-media projection, entirely on the archive worker.
use super::*;
use crate::contact_info::{Info, Kind, MEDIA_LIMIT, Media, MediaList, PREVIEW_COUNT, Section};

// Classify metadata only. No envelopes, original media or thumbnail blobs are
// read, and malformed/legacy empty JSON cannot turn an info request into an error.
// Each message has one entry, so the combined count never double-counts it.
const ITEMS: &str = "WITH eligible AS (
    SELECT id,stamp,CASE WHEN json_valid(content) THEN content ELSE '{}' END AS data
    FROM messages WHERE chat_id=?1 AND visible=1 AND deleted=0
      AND (expires IS NULL OR expires>unixepoch())
), items AS (
    SELECT id,stamp,data,CASE
      WHEN json_type(data,'$.photo')='object' THEN 0
      WHEN json_type(data,'$.video')='object' THEN 1
      WHEN json_type(data,'$.document')='object' THEN 2
      WHEN json_type(data,'$.preview')='object'
        AND length(trim(coalesce(json_extract(data,'$.preview.url'),'')))>0 THEN 3
    END AS kind FROM eligible
)";

fn available(db: &Connection, chat: i32) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM chats c WHERE c.id=?1 AND c.listed=1
         AND NOT EXISTS(SELECT 1 FROM chat_settings s
           WHERE s.jid=c.jid AND s.kind=2 AND s.value=1))",
        [chat],
        |r| r.get(0),
    )?)
}

impl Archive {
    pub async fn contact_info(&self, requested_chat: i32) -> Result<Option<Info>> {
        self.call(move |db| {
            let chat = local_chat(db, requested_chat)?;
            if !available(db, chat)? {
                return Ok(None);
            }
            let (jid, name, group, ephemeral_seconds, muted, archived) = db.query_row(
                "SELECT c.jid,
                   CASE WHEN n.saved_name IS NOT NULL
                     THEN coalesce(nullif(n.saved_name,''),n.profile_name)
                     ELSE coalesce(nullif(c.name,''),n.profile_name,'') END,
                   c.is_group,c.ephemeral,
                   coalesce(mute.value<0 OR mute.value>?2,0),coalesce(archive.value,0)
                 FROM chats c LEFT JOIN contacts n ON n.jid=c.jid
                 LEFT JOIN chat_settings mute ON mute.jid=c.jid AND mute.kind=0
                 LEFT JOIN chat_settings archive ON archive.jid=c.jid AND archive.kind=1
                 WHERE c.id=?1",
                params![chat, chrono::Utc::now().timestamp_millis()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, bool>(4)?,
                        r.get::<_, bool>(5)?,
                    ))
                },
            )?;
            let owner: Option<(String, String)> = db
                .query_row(
                    "SELECT i.value,coalesce(p.value,'') FROM metadata i
                     LEFT JOIN metadata p ON p.key='profile_name' WHERE i.key='identity'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let own = owner
                .as_ref()
                .map(|(identity, _)| canonical(db, identity))
                .transpose()?
                .is_some_and(|identity| identity == jid);
            let name = if own {
                let profile = owner.map(|(_, profile)| profile).unwrap_or_default();
                if profile.trim().is_empty() {
                    "You".into()
                } else {
                    format!("{profile} (You)")
                }
            } else if name.is_empty() {
                jid.split('@').next().unwrap_or(&jid).into()
            } else {
                name
            };
            let phone = jid
                .strip_suffix("@s.whatsapp.net")
                .and_then(domain::phone_number)
                .map(|number| format!("+{number}"));
            let counts = db.query_row(
                &format!(
                    "{ITEMS} SELECT coalesce(sum(kind IN (0,1)),0),
                     coalesce(sum(kind=3),0),coalesce(sum(kind=2),0),count(*)
                     FROM items WHERE kind IS NOT NULL"
                ),
                [chat],
                |r| {
                    Ok([
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ])
                },
            )?;
            let [media_count, link_count, document_count, shared_count] =
                counts.map(|count| usize::try_from(count.max(0)).unwrap_or(usize::MAX));
            let photos = db
                .prepare_cached(&format!(
                    "{ITEMS} SELECT id FROM items WHERE kind=0
                     ORDER BY stamp DESC,id DESC LIMIT ?2"
                ))?
                .query_map(params![chat, PREVIEW_COUNT as i64], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(Some(Info {
                chat: requested_chat,
                name,
                phone,
                group,
                own,
                ephemeral_seconds,
                muted,
                archived,
                media_count,
                link_count,
                document_count,
                shared_count,
                photos,
            }))
        })
        .await
    }

    pub async fn contact_media(
        &self,
        requested_chat: i32,
        section: Option<Section>,
    ) -> Result<Option<MediaList>> {
        self.call(move |db| {
            let chat = local_chat(db, requested_chat)?;
            if !available(db, chat)? {
                return Ok(None);
            }
            let filter = match section {
                None => "kind IS NOT NULL",
                Some(Section::Media) => "kind IN (0,1)",
                Some(Section::Documents) => "kind=2",
                Some(Section::Links) => "kind=3",
            };
            // The filter is a fixed application value. SQL selects at most 101
            // matching records, rather than loading the conversation into memory.
            let sql = format!(
                "{ITEMS} SELECT id,stamp,data,kind FROM items WHERE {filter}
                 ORDER BY stamp DESC,id DESC LIMIT ?2"
            );
            let mut query = db.prepare_cached(&sql)?;
            let mut rows = query.query(params![chat, (MEDIA_LIMIT + 1) as i64])?;
            let mut result = MediaList {
                chat: requested_chat,
                section,
                ..Default::default()
            };
            while let Some(row) = rows.next()? {
                if result.rows.len() == MEDIA_LIMIT {
                    result.more = true;
                    break;
                }
                let Ok(content) = serde_json::from_str::<crate::message_content::Content>(
                    &row.get::<_, String>(2)?,
                ) else {
                    continue;
                };
                let kind = match row.get::<_, i32>(3)? {
                    0 => Kind::Photo,
                    1 => Kind::Video,
                    2 => Kind::Document,
                    _ => Kind::Link,
                };
                let (label, details, link) = match kind {
                    Kind::Photo => (
                        content
                            .body
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "Photo".into()),
                        String::new(),
                        None,
                    ),
                    Kind::Video => (
                        content
                            .body
                            .filter(|s| !s.trim().is_empty())
                            .unwrap_or_else(|| "Video".into()),
                        content
                            .video
                            .and_then(|v| v.seconds)
                            .map(|seconds| format!("{}:{:02}", seconds / 60, seconds % 60))
                            .unwrap_or_default(),
                        None,
                    ),
                    Kind::Document => {
                        let document = content.document.unwrap();
                        let details = document.details();
                        let label = if document.file_name.trim().is_empty() {
                            "Document".into()
                        } else {
                            document.file_name
                        };
                        (label, details, None)
                    }
                    Kind::Link => {
                        let preview = content.preview.unwrap();
                        let label = if preview.title.trim().is_empty() {
                            preview.url.clone()
                        } else {
                            preview.title
                        };
                        (
                            label,
                            crate::message_content::caption(&preview.description, 160),
                            Some(preview.url),
                        )
                    }
                };
                let date = time(row.get(1)?);
                let detail = if details.is_empty() {
                    date
                } else {
                    format!("{date} · {details}")
                };
                result.rows.push(Media {
                    id: row.get(0)?,
                    kind,
                    label: crate::message_content::caption(&label, 240),
                    detail,
                    link,
                });
            }
            Ok(Some(result))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive::tests::{LID, PN, fixture, record},
        chat_management::{Setting, Update},
        message_content::{Content, Document, Photo, Preview, Video},
    };

    fn photo(i: i32) -> Record {
        let mut row = record(&format!("info-photo-{i}"));
        row.timestamp += i64::from(i);
        row.content = Content {
            body: Some(String::new()),
            photo: Some(Photo {
                width: 900,
                height: 1200,
                size: Some(12345),
            }),
            ..Default::default()
        };
        row
    }
    fn link() -> Record {
        let mut row = record("info-link");
        row.timestamp -= 1;
        row.content = Content {
            preview: Some(Preview {
                url: format!("https://example.invalid/{}", "path".repeat(100)),
                title: "A long title 👩🏽‍💻 日本語\n".repeat(100),
                description: "A supplied link preview".into(),
                has_thumbnail: false,
            }),
            ..Default::default()
        };
        row
    }
    fn update(setting: Setting, value: i64, stamp: i64) -> Update {
        Update {
            jid: PN.into(),
            setting,
            value,
            stamp: Some(stamp),
        }
    }
    fn unchanged(db: &Archive) -> (i64, i64, i64, i64) {
        db.blocking(|c| {
            Ok(c.query_row(
                "SELECT sum(unread),sum(length(raw)),sum(length(content)),max(id) FROM messages",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?)
        })
        .unwrap()
    }

    #[tokio::test]
    async fn cached_info_counts_only_available_media_without_reading_or_mutating_messages() {
        let (_temp, db) = fixture();
        let mut video = record("info-video");
        video.timestamp += 1;
        video.content.video = Some(Video {
            width: 1920,
            height: 1080,
            seconds: Some(75),
            size: Some(23456),
            gif: false,
        });
        let mut document = record("info-document");
        document.timestamp += 2;
        document.content.document = Some(Document {
            file_name: "Report 日本語.pdf".into(),
            mime_type: "application/pdf".into(),
            size: Some(42000),
            pages: Some(2),
            has_thumbnail: false,
        });
        let mut hidden = photo(20);
        hidden.visible = false;
        let mut expired = photo(21);
        expired.expires = Some(1);
        db.ingest(Batch {
            chats: vec![ChatMeta {
                jid: PN.into(),
                name: "History title".into(),
                ephemeral: Some(86400),
                ..Default::default()
            }],
            messages: vec![
                photo(0),
                video,
                document,
                link(),
                photo(4),
                photo(5),
                hidden,
                expired,
                record("legacy-empty"),
            ],
            live: true,
            ..Default::default()
        })
        .await
        .unwrap();
        db.call(|c| {
            c.execute(
                "UPDATE messages SET deleted=1 WHERE wire_id='info-photo-5'",
                [],
            )?;
            c.execute(
                "UPDATE messages SET content='' WHERE wire_id='legacy-empty'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.save_contacts(
            vec![Contact {
                jid: PN.into(),
                name: "Saved name".into(),
                timestamp: Some(1),
            }],
            vec![],
            false,
        )
        .await
        .unwrap();
        db.save_chat_settings(vec![
            update(Setting::Mute, -1, 10),
            update(Setting::Archive, 1, 10),
        ])
        .await
        .unwrap();
        let saved = unchanged(&db);
        let info = db.contact_info(1).await.unwrap().unwrap();
        assert_eq!(info.name, "Saved name");
        assert_eq!(info.phone.as_deref(), Some("+15550000001"));
        assert!(!info.group && !info.own && info.muted && info.archived);
        assert_eq!(info.ephemeral_seconds, 86400);
        assert_eq!(
            (
                info.media_count,
                info.document_count,
                info.link_count,
                info.shared_count
            ),
            (3, 1, 1, 5)
        );
        assert_eq!(info.photos, [5, 1]);
        let media = db
            .contact_media(1, Some(Section::Media))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            media.rows.iter().map(|m| m.id).collect::<Vec<_>>(),
            [5, 2, 1]
        );
        assert_eq!(media.rows[1].kind, Kind::Video);
        assert!(media.rows[1].detail.contains("1:15"));
        let documents = db
            .contact_media(1, Some(Section::Documents))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(documents.rows.len(), 1);
        assert_eq!(documents.rows[0].label, "Report 日本語.pdf");
        assert!(documents.rows[0].detail.contains("2 pages • PDF • 42 kB"));
        let links = db
            .contact_media(1, Some(Section::Links))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(links.rows.len(), 1);
        assert_eq!(links.rows[0].kind, Kind::Link);
        assert_eq!(links.rows[0].link, link().content.preview.map(|p| p.url));
        assert!(links.rows[0].label.chars().count() <= domain::SUMMARY_CHARS + 1);
        assert_eq!(
            saved,
            unchanged(&db),
            "opening details must not mark items viewed"
        );
        db.save_chat_settings(vec![update(
            Setting::Mute,
            chrono::Utc::now().timestamp_millis() - 1000,
            11,
        )])
        .await
        .unwrap();
        assert!(!db.contact_info(1).await.unwrap().unwrap().muted);
        db.bind_account(PN.into(), "Owner".into()).await.unwrap();
        let info = db.contact_info(1).await.unwrap().unwrap();
        assert!(info.own);
        assert_eq!(info.name, "Owner (You)");
    }

    #[tokio::test]
    async fn previews_and_lists_are_bounded_and_filter_before_limiting() {
        let (_temp, db) = fixture();
        let original = "Long caption 👩🏽‍💻 ăâîșț 日本語\n".repeat(1000);
        let mut messages = (0..105).map(photo).chain([link()]).collect::<Vec<_>>();
        messages[104].content.body = Some(original.clone());
        db.ingest(Batch {
            messages,
            ..Default::default()
        })
        .await
        .unwrap();
        let info = db.contact_info(1).await.unwrap().unwrap();
        assert_eq!(info.media_count, 105);
        assert_eq!(info.shared_count, 106);
        assert_eq!(info.photos, [105, 104, 103, 102]);
        let all = db.contact_media(1, None).await.unwrap().unwrap();
        assert_eq!(all.rows.len(), MEDIA_LIMIT);
        assert!(all.more);
        assert_eq!(all.rows[0].id, 105);
        assert!(all.rows[0].label.chars().count() <= 240);
        assert_eq!(
            db.blocking(|c| Ok(c.query_row(
                "SELECT json_extract(content,'$.body') FROM messages WHERE id=105",
                [],
                |r| r.get::<_, String>(0)
            )?))
            .unwrap(),
            original
        );
        let links = db
            .contact_media(1, Some(Section::Links))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            links.rows.len(),
            1,
            "an older link remains discoverable after 105 photos"
        );
        assert!(!links.more);
        let documents = db
            .contact_media(1, Some(Section::Documents))
            .await
            .unwrap()
            .unwrap();
        assert!(documents.rows.is_empty() && !documents.more);
        db.call(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id IN (104,105)", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            db.contact_info(1).await.unwrap().unwrap().photos,
            [103, 102, 101, 100]
        );
    }

    #[tokio::test]
    async fn aliases_names_and_access_checks_apply_to_both_projections() {
        let (_temp, db) = fixture();
        let mut message = photo(0);
        message.chat = LID.into();
        message.sender = LID.into();
        db.ingest(Batch {
            messages: vec![message],
            ..Default::default()
        })
        .await
        .unwrap();
        let lid = 1;
        assert!(
            db.contact_info(lid).await.unwrap().unwrap().phone.is_none(),
            "a LID is not a phone number"
        );
        let pn = db.create_chat(PN.into()).await.unwrap();
        db.ingest(Batch {
            aliases: vec![(LID.into(), PN.into())],
            ..Default::default()
        })
        .await
        .unwrap();
        let info = db.contact_info(lid).await.unwrap().unwrap();
        assert_eq!(info.chat, lid);
        assert_eq!(info.phone.as_deref(), Some("+15550000001"));
        assert_eq!(info.photos, [1]);
        let redirected = db.contact_media(lid, None).await.unwrap().unwrap();
        assert_eq!(redirected.chat, lid);
        assert_eq!(redirected.rows[0].id, 1);
        db.save_chat_settings(vec![update(Setting::Lock, 1, 10)])
            .await
            .unwrap();
        for chat in [lid, pn, 999] {
            assert!(db.contact_info(chat).await.unwrap().is_none());
            assert!(db.contact_media(chat, None).await.unwrap().is_none());
        }
        db.save_chat_settings(vec![update(Setting::Lock, 0, 11)])
            .await
            .unwrap();
        db.call(move |c| {
            c.execute("UPDATE chats SET listed=0 WHERE id=?1", [pn])?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(db.contact_info(lid).await.unwrap().is_none());
        assert!(db.contact_media(lid, None).await.unwrap().is_none());
        db.ingest(Batch {
            chats: vec![ChatMeta {
                jid: "synthetic-group@g.us".into(),
                name: "Group 日本語".into(),
                group: true,
                ..Default::default()
            }],
            ..Default::default()
        })
        .await
        .unwrap();
        let group = db.create_chat("synthetic-group@g.us".into()).await.unwrap();
        let info = db.contact_info(group).await.unwrap().unwrap();
        assert!(info.group && info.phone.is_none() && !info.own);
        assert_eq!(info.name, "Group 日本語");
    }
}
