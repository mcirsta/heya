//! Cached chat details and shared items. No toolkit or protocol types.
use crate::archive::Archive;
use std::sync::Arc;
use tokio::sync::watch;

pub const PREVIEW_COUNT: usize = 4;
pub const MEDIA_LIMIT: usize = 100;

// Private account data: deliberately no Debug implementation.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// Requested local ID; archive queries resolve redirects internally.
    pub chat: i32,
    pub name: String,
    pub phone: Option<String>,
    pub group: bool,
    pub own: bool,
    pub ephemeral_seconds: u32,
    pub muted: bool,
    pub archived: bool,
    /// Counts describe available local history, not complete account history.
    pub media_count: usize,
    pub link_count: usize,
    pub document_count: usize,
    pub shared_count: usize,
    /// Newest photo message IDs; the existing photo service owns their pixels.
    pub photos: Vec<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Kind {
    Photo = 0,
    Video = 1,
    Document = 2,
    Link = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Media,
    Documents,
    Links,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Media {
    pub id: i32,
    pub kind: Kind,
    pub label: String,
    pub detail: String,
    /// The full sender-supplied preview URL, independent of its shortened label.
    pub link: Option<String>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct MediaList {
    /// Requested local ID, matching the demand even after an alias merge.
    pub chat: i32,
    pub section: Option<Section>,
    pub rows: Vec<Media>,
    pub more: bool,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Demand {
    pub chat: i32,
    pub request: i32,
    pub section: Option<Section>,
    pub media: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub demand: Demand,
    pub info: Option<Info>,
    pub list: Option<MediaList>,
    pub error: String,
}

async fn load(db: &Archive, demand: Demand) -> Snapshot {
    let mut snapshot = Snapshot {
        demand,
        ..Default::default()
    };
    let info = match db.contact_info(snapshot.demand.chat).await {
        Ok(Some(info)) => info,
        Ok(None) => {
            snapshot.error = "This chat is no longer available.".into();
            return snapshot;
        }
        Err(_) => {
            snapshot.error = "Contact info could not be loaded. Please try again.".into();
            return snapshot;
        }
    };
    if snapshot.demand.media {
        match db
            .contact_media(snapshot.demand.chat, snapshot.demand.section)
            .await
        {
            Ok(Some(list)) => snapshot.list = Some(list),
            Ok(None) => {
                snapshot.error = "This chat is no longer available.".into();
                return snapshot;
            }
            Err(_) => {
                snapshot.error = "Shared items could not be loaded. Please try again.".into();
                return snapshot;
            }
        }
    }
    snapshot.info = Some(info);
    snapshot
}

async fn archive_changed(changes: &mut Option<watch::Receiver<u64>>) {
    if let Some(changes) = changes {
        let _ = changes.changed().await;
    } else {
        std::future::pending::<()>().await;
    }
}

/// A latest-demand task on the existing runtime, with no polling or service
/// thread. Closing/navigation cancels waiting and fences any late DB result.
/// Archive methods own SQL work; the panel never rebuilds the conversation.
pub async fn run(
    mut archives: watch::Receiver<Option<Archive>>,
    mut requests: watch::Receiver<Demand>,
    updates: watch::Sender<Arc<Snapshot>>,
) {
    loop {
        let demand = requests.borrow_and_update().clone();
        let archive = archives.borrow_and_update().clone();
        let mut changes = archive
            .as_ref()
            .filter(|_| demand.chat > 0)
            .map(Archive::subscribe);
        let snapshot = if demand.chat <= 0 {
            Snapshot {
                demand,
                ..Default::default()
            }
        } else if let Some(archive) = &archive {
            tokio::select! {
                biased;
                result=requests.changed()=>{if result.is_err(){return;}continue;}
                result=archives.changed()=>{if result.is_err(){return;}continue;}
                _=updates.closed()=>return,
                result=load(archive,demand)=>result,
            }
        } else {
            Snapshot {
                demand,
                error: "Open a saved conversation to view its contact info.".into(),
                ..Default::default()
            }
        };
        // A DB job already dispatched to spawn_blocking is finite but cannot
        // be cancelled once started. Coalesce archive bursts until it finishes
        // instead of queuing another read for every notification.
        if changes
            .as_ref()
            .is_some_and(|changes| changes.has_changed().unwrap_or(true))
        {
            continue;
        }
        if updates.send(Arc::new(snapshot)).is_err() {
            return;
        }
        tokio::select! {
            biased;
            result=requests.changed()=>if result.is_err(){return;},
            result=archives.changed()=>if result.is_err(){return;},
            _=updates.closed()=>return,
            _=archive_changed(&mut changes)=>{},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        archive::{
            Batch,
            tests::{fixture, record},
        },
        message_content::Photo,
    };
    use std::time::Duration;

    async fn next(
        updates: &mut watch::Receiver<Arc<Snapshot>>,
        predicate: impl Fn(&Snapshot) -> bool,
    ) -> Arc<Snapshot> {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let current = updates.borrow_and_update().clone();
                if predicate(&current) {
                    return current;
                }
                updates.changed().await.unwrap();
            }
        })
        .await
        .expect("contact-info worker did not publish a result")
    }

    #[tokio::test]
    async fn latest_demand_closes_during_a_slow_database_read_and_refreshes_only_while_open() {
        let (_temp, db) = fixture();
        let mut message = record("contact-panel-photo");
        message.content.photo = Some(Photo {
            width: 900,
            height: 1200,
            size: Some(12345),
        });
        db.ingest(Batch {
            messages: vec![message],
            ..Default::default()
        })
        .await
        .unwrap();
        let (_archives, archives) = watch::channel(Some(db.clone()));
        let (requests, requested) = watch::channel(Demand::default());
        let (updates, mut snapshots) = watch::channel(Arc::new(Snapshot::default()));
        let worker = tokio::spawn(run(archives, requested, updates));
        let (entered, blocked) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let database = db.clone();
        let blocker = tokio::spawn(async move {
            database
                .call(move |_| {
                    let _ = entered.send(());
                    released.recv().unwrap();
                    Ok(())
                })
                .await
                .unwrap();
        });
        blocked.await.unwrap();
        requests
            .send(Demand {
                chat: 1,
                request: 1,
                ..Default::default()
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        requests
            .send(Demand {
                request: 2,
                ..Default::default()
            })
            .unwrap();
        let closed = next(&mut snapshots, |s| s.demand.request == 2).await;
        assert!(closed.info.is_none() && closed.list.is_none() && closed.error.is_empty());
        release.send(()).unwrap();
        blocker.await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !snapshots.has_changed().unwrap(),
            "the cancelled result must not reopen the panel"
        );

        requests
            .send(Demand {
                chat: 1,
                request: 3,
                media: true,
                section: Some(Section::Media),
            })
            .unwrap();
        let open = next(&mut snapshots, |s| {
            s.demand.request == 3 && s.info.is_some()
        })
        .await;
        assert_eq!(open.info.as_ref().unwrap().media_count, 1);
        assert_eq!(open.list.as_ref().unwrap().rows.len(), 1);
        db.call(|c| {
            c.execute("UPDATE messages SET deleted=1 WHERE id=1", [])?;
            Ok(())
        })
        .await
        .unwrap();
        db.changed();
        let refreshed = next(&mut snapshots, |s| {
            s.demand.request == 3 && s.info.as_ref().is_some_and(|i| i.media_count == 0)
        })
        .await;
        assert!(refreshed.list.as_ref().unwrap().rows.is_empty());
        requests
            .send(Demand {
                request: 4,
                ..Default::default()
            })
            .unwrap();
        next(&mut snapshots, |s| s.demand.request == 4).await;
        db.changed();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !snapshots.has_changed().unwrap(),
            "closed panels must not refresh on archive bursts"
        );
        db.call(|c| {
            c.execute_batch("DROP TABLE contacts")?;
            Ok(())
        })
        .await
        .unwrap();
        requests
            .send(Demand {
                chat: 1,
                request: 5,
                ..Default::default()
            })
            .unwrap();
        let failed = next(&mut snapshots, |s| s.demand.request == 5).await;
        assert!(failed.info.is_none() && failed.list.is_none() && !failed.error.is_empty());
        drop(requests);
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn a_parked_worker_exits_when_its_output_receiver_is_dropped() {
        let (_archives, archives) = watch::channel(None);
        let (_requests, requests) = watch::channel(Demand::default());
        let (updates, mut snapshots) = watch::channel(Arc::new(Snapshot::default()));
        let worker = tokio::spawn(run(archives, requests, updates));
        snapshots.changed().await.unwrap();
        drop(snapshots);
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
    }
}
