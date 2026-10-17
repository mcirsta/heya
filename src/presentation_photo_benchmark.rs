//! Disposable, synthetic encrypted media archive for the UI benchmark. Uses the
//! production database/decode/presentation path, rather than three shared textures.
use crate::{AppWindow, archive, domain, photos, storage};
use slint::ComponentHandle;
use std::sync::Arc;
use tokio::{runtime::Runtime, sync::watch, task::JoinHandle};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Media {
    Photos,
    Documents,
}

pub fn start(
    ui: &AppWindow,
    runtime: &Runtime,
    messages: Vec<domain::Message>,
    media: Media,
) -> Vec<JoinHandle<()>> {
    // Preparation finishes before timing starts. There is no desktop vault or
    // account path here; the temporary directory is owned by the worker below.
    let (temp, db) = runtime.block_on(runtime.spawn_blocking(move || {
        struct SyntheticVault;
        impl storage::SecretVault for SyntheticVault {
            fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::connection::Failure> {
                Ok(Some(zeroize::Zeroizing::new(vec![29; 32])))
            }
            fn write(&self, _: &[u8]) -> Result<(), crate::connection::Failure> {
                unreachable!("the synthetic key already exists")
            }
        }
        let temp = tempfile::tempdir().expect("benchmark directory");
        let files = storage::prepare_at(temp.path(), &SyntheticVault).expect("synthetic files");
        let db = archive::Archive::open(files.clone(), &files.key_sql).expect("synthetic archive");
        let previews = (0..3).map(|i| {
            let p = if media == Media::Documents {
                let image = crate::fixtures::document_thumbnail();
                photos::Pixels { width: image.width(), height: image.height(), rgba: image.into_raw() }
            } else { crate::fixtures::photo_pixels(i, photos::INLINE_EDGE) };
            let mut encoded = Vec::new();
            let mut encoder = png::Encoder::new(&mut encoded, p.width, p.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(&p.rgba).unwrap();
            encoded
        }).collect::<Vec<_>>();
        db.blocking(|db| {
            let tx = db.transaction()?;
            tx.execute("INSERT INTO chats(id,jid) VALUES(1,'synthetic-media-benchmark')", [])?;
            for message in messages {
                let content = serde_json::to_string(&*message.content).expect("synthetic content");
                tx.execute("INSERT INTO messages(id,chat_id,sender,wire_id,stamp,outgoing,sender_name,body,raw,visible,source_chat,source_sender,content,thumbnail) VALUES(?1,1,'synthetic',?1,1,0,'Synthetic','',X'00',1,'synthetic','synthetic',?2,?3)", rusqlite::params![message.id, content, if media == Media::Documents && message.content.document.as_ref().is_some_and(|d| d.has_thumbnail) { previews[0].as_slice() } else { &[] }])?;
                if media == Media::Photos { tx.execute("INSERT INTO message_photos(message_id,original,preview,state,revision) VALUES(?1,?2,?2,2,1)", rusqlite::params![message.id, previews[message.id.unsigned_abs() as usize%3]])?; }
            }
            tx.commit()?;
            Ok(())
        }).expect("seed synthetic archive");
        (temp, db)
    })).expect("benchmark preparation");
    if media == Media::Documents {
        let demand = crate::presentation_content::bind_images(ui);
        let (updates, snapshots) = watch::channel(crate::message_thumbnails::Snapshot::default());
        let worker = runtime.spawn(async move {
            let (_archives, archives) = watch::channel(Some(db));
            crate::message_thumbnails::present(archives, demand, updates).await;
            drop(temp);
        });
        let pump = runtime.spawn(crate::live_presentation::present_latest(
            ui.as_weak(),
            snapshots,
            crate::presentation_content::publish_images,
        ));
        return vec![worker, pump];
    }
    let demand = crate::presentation_photos::bind(ui);
    let (updates, snapshots) = watch::channel(Arc::new(photos::Snapshot::default()));
    let worker = runtime.spawn(async move {
        let (_archives, archives) = watch::channel(Some(db));
        photos::present(archives, demand, updates).await;
        drop(temp);
    });
    let pump = runtime.spawn(crate::live_presentation::present_latest(
        ui.as_weak(),
        snapshots,
        crate::presentation_photos::publish,
    ));
    vec![worker, pump]
}
