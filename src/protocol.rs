//! whatsapp-rust boundary. Owns the SDK, connection callbacks and encrypted pool.
use crate::{
    archive::Archive,
    connection::{Failure, Phase, QrImage, Request, Snapshot},
    protocol_messages::{self, Ingest, MessageEvents},
    storage::{self, SessionFiles},
};
use diesel::{RunQueryDsl, connection::SimpleConnection};
use qrcodegen::{QrCode, QrCodeEcc};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::watch,
    time::{Instant, timeout},
};
use whatsapp_rust::{
    prelude::{Bot, Event, EventHandler, EventInterest, EventKind},
    store::{SqliteDatabase, SqliteDatabaseConfig, SqliteStore, Synchronous},
};
use zeroize::Zeroizing;

#[path = "protocol_startup.rs"]
mod startup;

#[derive(diesel::QueryableByName)]
struct CipherVersion {
    #[diesel(sql_type = diesel::sql_types::Text)]
    cipher_version: String,
}

async fn open_database(files: Arc<SessionFiles>) -> Result<SqliteDatabase, Failure> {
    let path = files
        .path
        .to_str()
        .ok_or(Failure::DataDirectory)?
        .to_owned();
    let config = SqliteDatabaseConfig {
        synchronous: Synchronous::Full,
        ..SqliteDatabaseConfig::default()
    }
    .with_connection_init(move |connection| {
        // Keeping files in this hook also holds the lock until the entire pool
        // releases it, even if SDK tasks outlive the bot supervisor.
        connection.batch_execute(&files.key_sql)?;
        let cipher =
            diesel::sql_query("PRAGMA cipher_version").get_result::<CipherVersion>(connection)?;
        if cipher.cipher_version.is_empty() {
            return Err(std::io::Error::other("SQLCipher unavailable").into());
        }
        diesel::sql_query("SELECT count(*) FROM sqlite_master").execute(connection)?;
        Ok(())
    });
    let database = SqliteDatabase::open(&path, config)
        .await
        .map_err(|_| Failure::Database)?;
    Ok(database)
}

async fn open_store(files: Arc<SessionFiles>) -> Result<SqliteStore, Failure> {
    Ok(open_database(files).await?.store(1))
}

/// Called only after the previous client's store-release fence has completed.
/// The protocol scope is reset transactionally; Heya's archive/key are untouched.
async fn reset_session(files: Arc<SessionFiles>) -> Result<(), Failure> {
    let database = open_database(files).await?;
    if database
        .device_exists(1)
        .await
        .map_err(|_| Failure::Database)?
    {
        database
            .reset_device(1)
            .await
            .map_err(|_| Failure::Database)?;
    }
    Ok(())
}

#[derive(Clone)]
enum Observation {
    Phase(Phase),
    Qr {
        payload: Zeroizing<String>,
        expires: Instant,
    },
}

struct ConnectionEvents(watch::Sender<Observation>);

impl EventHandler for ConnectionEvents {
    fn interest(&self) -> EventInterest {
        EventInterest::of(&[
            EventKind::Connected,
            EventKind::Disconnected,
            EventKind::PairSuccess,
            EventKind::PairError,
            EventKind::LoggedOut,
            EventKind::PairingQrCode,
            EventKind::PairingQrCodesExhausted,
            EventKind::ClientOutdated,
            EventKind::StreamReplaced,
            EventKind::QrScannedWithoutMultidevice,
        ])
    }

    fn handle_event(&self, event: Arc<Event>) {
        // Inline callback: no I/O, logging, QR encoding or async callback queue.
        let next = match event.as_ref() {
            Event::Connected(_) => Observation::Phase(Phase::Connected),
            Event::Disconnected(_) => Observation::Phase(Phase::Reconnecting),
            Event::PairSuccess(_) => Observation::Phase(Phase::Pairing),
            Event::PairingQrCode(qr) => Observation::Qr {
                payload: Zeroizing::new(qr.code.clone()),
                expires: Instant::now() + qr.timeout,
            },
            Event::PairError(_) | Event::QrScannedWithoutMultidevice(_) => {
                Observation::Phase(Phase::Failed(Failure::Pairing))
            }
            Event::LoggedOut(_) => Observation::Phase(Phase::Failed(Failure::LoggedOut)),
            Event::PairingQrCodesExhausted(_) => {
                Observation::Phase(Phase::Failed(Failure::QrExpired))
            }
            Event::ClientOutdated(_) => Observation::Phase(Phase::Failed(Failure::Outdated)),
            Event::StreamReplaced(_) => Observation::Phase(Phase::Failed(Failure::Conflict)),
            _ => return,
        };
        self.0.send_if_modified(|current| {
            // A trailing Disconnected must never erase the actionable cause.
            if matches!(current, Observation::Phase(Phase::Failed(_))) {
                return false;
            }
            *current = next;
            true
        });
    }
}

fn qr_image(payload: &str) -> Result<QrImage, Failure> {
    let code = QrCode::encode_text(payload, QrCodeEcc::Medium).map_err(|_| Failure::Pairing)?;
    let width = (code.size() + 8) as u32 * 4;
    let mut pixels = vec![255; width as usize * width as usize * 4];
    for y in 0..width {
        for x in 0..width {
            if code.get_module((x / 4) as i32 - 4, (y / 4) as i32 - 4) {
                let offset = (y as usize * width as usize + x as usize) * 4;
                pixels[offset..offset + 3].fill(0);
            }
        }
    }
    Ok(QrImage {
        width,
        pixels: pixels.into(),
    })
}

fn qr_snapshot(image: QrImage, expires: Instant, now: Instant) -> Snapshot {
    if now >= expires {
        return Snapshot::new(Phase::Connecting);
    }
    Snapshot {
        phase: Phase::Qr,
        qr: Some(image),
        seconds_left: (expires - now)
            .as_nanos()
            .div_ceil(1_000_000_000)
            .min(u32::MAX as u128) as u32,
    }
}

fn requested(requests: &watch::Receiver<Request>) -> bool {
    requests.has_changed().is_ok() && requests.borrow().connects()
}

pub struct LiveServices {
    pub chat_sources: watch::Sender<crate::chat_management::Sources>,
    pub preferences: watch::Receiver<crate::preferences::Preferences>,
    pub avatar_demand: watch::Receiver<Vec<crate::avatars::Target>>,
    pub photo_demand: watch::Receiver<crate::photos::Demand>,
    pub thumbnail_demand: watch::Receiver<Vec<i32>>,
    pub audio_sources: watch::Sender<crate::audio::Sources>,
    pub video_sources: watch::Sender<crate::video::Sources>,
    pub call_sources: watch::Sender<crate::calling::Sources>,
    pub history_sources: watch::Sender<crate::phone_history::Sources>,
    pub download_sources: watch::Sender<crate::downloads::Sources>,
    pub upload_sources: watch::Sender<crate::attachments::Sources>,
    pub presence: crate::presence::Channels,
}

impl LiveServices {
    fn clear_sources(&self) {
        self.audio_sources.send_replace(None);
        self.video_sources.send_replace(None);
        self.call_sources.send_replace(None);
        self.download_sources.send_replace(None);
        self.upload_sources.send_replace(None);
        self.history_sources.send_replace(None);
        self.chat_sources.send_replace(None);
    }
}

async fn connect(
    requests: &mut watch::Receiver<Request>,
    updates: &watch::Sender<Snapshot>,
    files: Arc<SessionFiles>,
    archive: Archive,
    services: &LiveServices,
) -> Result<(), Failure> {
    let audio_sources = &services.audio_sources;
    let chat_changes = Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct ClearMediaSources<'a>(&'a LiveServices);
    impl Drop for ClearMediaSources<'_> {
        fn drop(&mut self) {
            self.0.clear_sources();
        }
    }
    let _clear_sources = ClearMediaSources(services);
    updates.send_replace(Snapshot::new(Phase::Opening));
    let store = open_store(files).await;
    if !requested(requests) {
        return Ok(());
    }
    let store = store?;
    let (events_tx, mut events) = watch::channel(Observation::Phase(Phase::Connecting));
    let (messages_tx, mut messages) = tokio::sync::mpsc::channel(512);
    let (overflow_tx, mut overflow) = watch::channel(false);
    let (ingest_failure, mut ingestion_failed) = watch::channel(None);
    let ingest = Ingest {
        archive: archive.clone(),
        failure: ingest_failure,
    };
    let contact_events = crate::protocol_sync::AppState::default();
    let presence_events = crate::protocol_presence::Events::default();
    let rebuild_contacts = archive.begin_contacts().await.map_err(archive_failure)?;
    let rebuild_reads = archive.begin_reads().await.map_err(archive_failure)?;
    let rebuild_chats = archive
        .begin_chat_settings()
        .await
        .map_err(archive_failure)?;
    let bot = Bot::builder()
        .with_backend(store)
        .with_device_props(whatsapp_rust::wacore::store::DevicePropsOverride::new().with_os("Heya"))
        .with_event_handler(ConnectionEvents(events_tx))
        .with_event_handler(contact_events.clone())
        .with_event_handler(presence_events.clone())
        .with_event_handler(MessageEvents {
            sender: messages_tx,
            overflow: overflow_tx,
        })
        .with_inbound_durability_hook(ingest.clone())
        .with_history_sync_capture_hook(ingest)
        .build()
        .await
        .map_err(|_| Failure::Protocol)?;
    if bot
        .client()
        .persistence_manager()
        .get_device_snapshot()
        .pn
        .is_some()
        && let Err(error) = protocol_messages::verify_identity(&bot.client(), &archive).await
    {
        let client = bot.client();
        let release = client.store_release();
        drop(bot);
        timeout(Duration::from_secs(15), async {
            let report = client.shutdown().await;
            drop(client);
            finish_shutdown(report, release, services).await
        })
        .await
        .map_err(|_| Failure::Shutdown)??;
        return Err(archive_failure(error));
    }
    if !requested(requests) {
        updates.send_replace(Snapshot::new(Phase::Stopping));
        let client = bot.client();
        let release = client.store_release();
        drop(bot);
        timeout(Duration::from_secs(15), async {
            let report = client.shutdown().await;
            drop(client);
            finish_shutdown(report, release, services).await
        })
        .await
        .map_err(|_| Failure::Shutdown)??;
        return Ok(());
    }
    let mut bot = bot.spawn();
    let release = bot.client().store_release();
    let calling = Arc::new(crate::protocol_calling::Calling::new(
        bot.client(),
        archive.clone(),
    ));
    let (online_tx, online) = watch::channel(false);
    let (stop_tx, stop) = watch::channel(false);
    let (contacts_stop, contacts_stopped) = watch::channel(false);
    let mut contacts = tokio::spawn(crate::protocol_sync::run(
        archive.clone(),
        bot.client(),
        contact_events.clone(),
        online.clone(),
        contacts_stopped,
        rebuild_contacts || rebuild_reads || rebuild_chats,
    ));
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut outbox = tokio::spawn(crate::protocol_outbox::run(
        archive.clone(),
        bot.client(),
        online.clone(),
        stop.clone(),
    ));
    let mut reads = tokio::spawn(crate::protocol_reads::run(
        archive.clone(),
        bot.client(),
        online.clone(),
        stop.clone(),
        services.preferences.clone(),
    ));
    let mut presence = tokio::spawn(crate::protocol_presence::run(
        archive.clone(),
        bot.client(),
        presence_events,
        services.presence.clone(),
        online.clone(),
        stop.clone(),
    ));
    let mut avatars = tokio::spawn(crate::avatars::fetch_worker(
        archive.clone(),
        crate::protocol_avatars::Pictures::new(bot.client()),
        services.avatar_demand.clone(),
        online.clone(),
        stop.clone(),
    ));
    let mut photos = tokio::spawn(crate::photos::fetch_worker(
        archive.clone(),
        crate::protocol_photos::Photos(bot.client()),
        services.photo_demand.clone(),
        online.clone(),
        stop.clone(),
    ));
    let mut document_previews = tokio::spawn(crate::document_previews::fetch_worker(
        archive.clone(),
        crate::protocol_document_previews::Previews(bot.client()),
        services.thumbnail_demand.clone(),
        online,
        stop.clone(),
    ));
    let mut history = tokio::spawn(protocol_messages::history_worker(
        archive.clone(),
        cancelled.clone(),
        stop,
    ));
    // Dropping a JoinHandle detaches it. Abort explicitly on cancellation or
    // cleanup timeout, including the cooperative blocking history importer.
    let _workers = Workers {
        abort: [
            outbox.abort_handle(),
            reads.abort_handle(),
            presence.abort_handle(),
            history.abort_handle(),
            contacts.abort_handle(),
            avatars.abort_handle(),
            photos.abort_handle(),
            document_previews.abort_handle(),
        ],
        cancelled: cancelled.clone(),
    };
    let mut outbox_done = false;
    let mut reads_done = false;
    let mut presence_done = false;
    let mut history_done = false;
    let mut contacts_done = false;
    let mut avatars_done = false;
    let mut photos_done = false;
    let mut document_previews_done = false;
    updates.send_replace(Snapshot::new(Phase::Connecting));
    let mut qr: Option<(QrImage, Instant)> = None;
    let mut terminal = None;
    let mut driver_finished = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    while requested(requests) {
        tokio::select! {
            biased;
            changed = requests.changed() => {
                if changed.is_err() || !requests.borrow_and_update().connects() { break; }
            }
            changed = events.changed() => {
                if changed.is_err() { terminal = Some(Failure::Protocol); break; }
                let observation = events.borrow_and_update().clone();
                match observation {
                    Observation::Phase(Phase::Failed(failure)) => { terminal = Some(failure); break; }
                    Observation::Phase(phase) => {
                        if phase!=Phase::Connected {calling.disconnected();}
                        if phase == Phase::Connected && let Err(error) = protocol_messages::verify_identity(&bot.client(),&archive).await { terminal=Some(archive_failure(error));break; }
                        online_tx.send_replace(phase == Phase::Connected);
                        audio_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_audio::Audio(bot.client())) as Arc<dyn crate::audio::Source>));
                        services.video_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_video::Video(bot.client())) as Arc<dyn crate::video::Source>));
                        services.call_sources.send_replace((phase==Phase::Connected).then(||calling.clone() as Arc<dyn crate::calling::Network>));
                        services.history_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_history::History(bot.client())) as Arc<dyn crate::phone_history::Source>));
                        services.chat_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_chat_management::Chats(bot.client(),chat_changes.clone())) as Arc<dyn crate::chat_management::Source>));
                        services.upload_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_uploads::Uploads(bot.client())) as Arc<dyn crate::attachments::Uploader>));
                        services.download_sources.send_replace((phase==Phase::Connected).then(||Arc::new(crate::protocol_downloads::Downloads(bot.client())) as Arc<dyn crate::downloads::Source>));
                        qr = None;
                        updates.send_replace(Snapshot::new(phase));
                    }
                    Observation::Qr { payload, expires } => {
                        match qr_image(&payload) {
                            Ok(image) => {
                                updates.send_replace(qr_snapshot(image.clone(), expires, Instant::now()));
                                qr = Some((image, expires));
                            }
                            Err(failure) => { terminal = Some(failure); break; }
                        }
                    }
                }
            }
            changed = ingestion_failed.changed() => {
                if changed.is_ok() && let Some(error)=*ingestion_failed.borrow() { terminal=Some(archive_failure(error));break; }
            }
            changed = overflow.changed() => {
                if changed.is_ok() && *overflow.borrow() { terminal=Some(Failure::Archive);break; }
            }
            Some(event) = messages.recv() => {
                calling.observe(&event).await;
                if let Err(error) = protocol_messages::event(&archive,&bot.client(),event).await {terminal=Some(archive_failure(error));break;}
            }
            _ = &mut presence => {presence_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut reads => {reads_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut outbox => {outbox_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut history => {history_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut contacts => {contacts_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut avatars => {avatars_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut document_previews => {document_previews_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut photos => {photos_done=true;terminal=Some(Failure::Archive);break;}
            _ = &mut bot => {
                // The event branch runs first so a terminal reason wins over
                // its corresponding supervisor exit whenever both are ready.
                driver_finished = true;
                terminal = Some(Failure::Protocol);
                break;
            }
            _ = tick.tick(), if qr.is_some() => {
                if let Some((image, expires)) = &qr {
                    let snapshot = qr_snapshot(image.clone(), *expires, Instant::now());
                    if snapshot.qr.is_none() { qr = None; }
                    updates.send_replace(snapshot);
                }
            }
        }
    }
    let logging_out = *requests.borrow() == Request::Logout;
    services.clear_sources();
    calling.disconnected();
    drop(calling);
    updates.send_replace(Snapshot::new(Phase::Stopping));
    online_tx.send_replace(false);
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    stop_tx.send_replace(true);
    // A completed BotHandle future cannot be polled a second time by shutdown().
    // Shut down its Client directly, then drop the handle and wait for ownership.
    let cleanup = async {
        // Outbox owns a Client; release it before waiting for the SDK's backend
        // fence. History commits already in progress finish atomically.
        if !presence_done {
            let _ = (&mut presence).await;
        }
        if !reads_done {
            let _ = (&mut reads).await;
        }
        if !outbox_done {
            let _ = (&mut outbox).await;
        }
        if !history_done {
            let _ = (&mut history).await;
        }
        if !avatars_done {
            let _ = (&mut avatars).await;
        }
        if !photos_done {
            let _ = (&mut photos).await;
        }
        if !document_previews_done {
            let _ = (&mut document_previews).await;
        }
        let recovery = archive.recover_sends().await.map_err(archive_failure);
        let report = if logging_out {
            let client = bot.client();
            let report = match timeout(Duration::from_secs(10), client.logout()).await {
                Ok(report) => {
                    terminal = Some(
                        if matches!(
                            report.deregistration,
                            whatsapp_rust::DeregistrationOutcome::Confirmed
                        ) {
                            Failure::LoggedOut
                        } else {
                            Failure::LogoutUnconfirmed
                        },
                    );
                    report.shutdown
                }
                Err(_) => {
                    terminal = Some(Failure::LogoutUnconfirmed);
                    // Cancelling the remote request must not skip local flush
                    // and teardown. Retain credentials if unlinking is uncertain.
                    client.shutdown().await
                }
            };
            drop(client);
            drop(bot);
            report
        } else if driver_finished {
            let report = bot.client().shutdown().await;
            drop(bot);
            report
        } else {
            bot.shutdown().await.shutdown
        };
        // Contacts mirror SDK sync cursors. Drain only after SDK dispatch ends,
        // then release their Client before waiting for the store ownership fence.
        contacts_stop.send_replace(true);
        let contact_result = if contacts_done {
            Ok(false)
        } else {
            (&mut contacts)
                .await
                .map_err(|_| Failure::Archive)
                .and_then(|result| result.map_err(archive_failure))
        };
        finish_shutdown(report, release, services).await?;
        if contact_result?
            && !matches!(
                terminal,
                Some(Failure::LoggedOut | Failure::ArchiveIdentity)
            )
        {
            // Detached SDK owners can finish dispatching while shutdown waits
            // for the store fence. Drain once more after all owners released it.
            contact_events
                .flush(&archive, false)
                .await
                .map_err(archive_failure)?;
            archive.finish_contacts().await.map_err(archive_failure)?;
            if !chat_changes.load(std::sync::atomic::Ordering::Acquire) {
                archive
                    .finish_chat_settings()
                    .await
                    .map_err(archive_failure)?;
            }
            // A receipt queued during teardown was not materialized. Keep the
            // dirty marker so the next connection replays authoritative state.
            if messages.is_empty() && terminal != Some(Failure::Archive) {
                archive.finish_reads().await.map_err(archive_failure)?;
            }
        }
        recovery
    };
    timeout(Duration::from_secs(15), cleanup)
        .await
        .map_err(|_| {
            eprintln!("Heya shutdown: session cleanup timed out.");
            Failure::Shutdown
        })??;
    terminal.map_or(Ok(()), Err)
}

struct Workers {
    abort: [tokio::task::AbortHandle; 8],
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        for handle in &self.abort {
            handle.abort();
        }
    }
}

fn archive_failure(error: crate::archive::Error) -> Failure {
    if error == crate::archive::Error::Identity {
        Failure::ArchiveIdentity
    } else {
        Failure::Archive
    }
}

async fn finish_shutdown(
    report: whatsapp_rust::ShutdownReport,
    release: whatsapp_rust::StoreRelease,
    services: &LiveServices,
) -> Result<(), Failure> {
    use whatsapp_rust::client::DrainOutcome;
    // Retained source snapshots own SDK Clients. Clear every media source before
    // waiting for store release, including in-flight video downloads.
    services.clear_sources();
    let saved = report.device.is_ok()
        && report.inbound == DrainOutcome::Completed
        && report.outbound == DrainOutcome::Completed
        && report.signal_settle == DrainOutcome::Completed
        && report.message_secrets.failed_batches == 0;
    release.wait().await;
    if saved {
        Ok(())
    } else {
        // Fixed flags only: SDK errors may contain account identifiers.
        eprintln!(
            "Heya shutdown: incomplete persistence (device={}, inbound={}, outbound={}, signal={}, secrets={}).",
            report.device.is_ok(),
            report.inbound == DrainOutcome::Completed,
            report.outbound == DrainOutcome::Completed,
            report.signal_settle == DrainOutcome::Completed,
            report.message_secrets.failed_batches == 0,
        );
        Err(Failure::Shutdown)
    }
}

pub async fn run(
    mut requests: watch::Receiver<Request>,
    updates: watch::Sender<Snapshot>,
    archives: watch::Sender<Option<Archive>>,
    services: LiveServices,
) {
    let mut prepared: Option<(Arc<SessionFiles>, Archive)> = None;
    while requests.changed().await.is_ok() {
        let request = *requests.borrow_and_update();
        if matches!(
            request,
            Request::Idle | Request::Disconnect | Request::Logout
        ) {
            continue;
        }
        let relink = request == Request::Relink;
        if relink
            && !matches!(
                updates.borrow().phase,
                Phase::Failed(Failure::LoggedOut | Failure::ArchiveIdentity)
            )
        {
            continue;
        }
        if matches!(request, Request::Startup | Request::OpenOffline) {
            match tokio::task::spawn_blocking(storage::has_session).await {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    updates.send_replace(Snapshot::new(Phase::Idle));
                    continue;
                }
                _ => {
                    updates.send_replace(Snapshot::new(Phase::Failed(Failure::DataDirectory)));
                    continue;
                }
            }
        }
        if requests.has_changed().is_err() || *requests.borrow() == Request::Disconnect {
            updates.send_replace(Snapshot::new(Phase::Stopped));
            continue;
        }
        updates.send_replace(Snapshot::new(Phase::Opening));
        if prepared.is_none() {
            let result = startup::prepare(&mut requests, &updates, || async {
                crate::local_worker::run(|| {
                    let files = storage::prepare()?;
                    let archive = Archive::open(files.clone(), &files.archive_key_sql)
                        .map_err(archive_failure)?;
                    protocol_messages::repair_text(&archive).map_err(archive_failure)?;
                    Ok((files, archive))
                })
                .await
                .unwrap_or(Err(Failure::Archive))
            })
            .await;
            match result {
                Ok(Some((files, archive))) => {
                    archives.send_replace(Some(archive.clone()));
                    prepared = Some((files, archive));
                }
                Ok(None) => continue,
                Err(error) => {
                    updates.send_replace(Snapshot::new(Phase::Failed(error)));
                    continue;
                }
            }
        }
        if !requested(&requests) {
            updates.send_replace(Snapshot::new(Phase::Stopped));
            continue;
        }
        let (files, archive) = prepared.as_ref().expect("prepared above");
        let prepare_connection = async {
            if relink {
                reset_session(files.clone()).await?;
                archive
                    .set_needs_link(false)
                    .await
                    .map_err(archive_failure)?;
            } else if archive.needs_link().await.map_err(archive_failure)? {
                return Err(Failure::LoggedOut);
            }
            Ok(())
        }
        .await;
        let result = match prepare_connection {
            Ok(()) => {
                connect(
                    &mut requests,
                    &updates,
                    files.clone(),
                    archive.clone(),
                    &services,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => {
                updates.send_replace(Snapshot::new(Phase::Stopped));
            }
            Err(mut failure) => {
                // Persist revocation so a restart does not repeatedly attempt
                // to use credentials that WhatsApp has already rejected.
                if matches!(failure, Failure::LoggedOut | Failure::ArchiveIdentity)
                    && archive.set_needs_link(true).await.is_err()
                {
                    failure = Failure::Archive;
                }
                updates.send_replace(Snapshot::new(Phase::Failed(failure)));
                if failure == Failure::Shutdown {
                    eprintln!(
                        "Heya could not complete session cleanup. Restart Heya before reconnecting."
                    );
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn protocol_session_survives_decryption_and_reencryption_without_relinking() {
        use crate::storage::{Mode, change_mode_at, prepare_at, tests::MemoryVault};
        use whatsapp_rust::store::traits::DeviceStore;
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::default();
        let files = prepare_at(temp.path(), &vault).unwrap();
        let store = open_store(files.clone()).await.unwrap();
        store.create().await.unwrap();
        let mut device = store.load().await.unwrap().unwrap();
        device.pn = Some(crate::archive::tests::PN.parse().unwrap());
        device.push_name = "Synthetic saved session".into();
        store.save(&device).await.unwrap();
        let released = Arc::downgrade(&files);
        drop(store);
        drop(files);
        timeout(Duration::from_secs(3), async {
            while released.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        for mode in [Mode::Unencrypted, Mode::Encrypted] {
            change_mode_at(temp.path(), mode, &vault).unwrap();
            let files = prepare_at(temp.path(), &vault).unwrap();
            let released = Arc::downgrade(&files);
            let store = open_store(files).await.unwrap();
            let restored = store.load().await.unwrap().unwrap();
            assert_eq!(restored.pn, device.pn);
            assert_eq!(restored.push_name, device.push_name);
            drop(store);
            timeout(Duration::from_secs(3), async {
                while released.upgrade().is_some() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn relink_resets_only_protocol_credentials_and_preserves_archive_identity() {
        use crate::{
            archive::tests::{KEY, PN, record},
            storage::{prepare_at, tests::MemoryVault},
        };
        use whatsapp_rust::store::traits::DeviceStore;
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::default();
        let files = prepare_at(temp.path(), &vault).unwrap();
        let store = open_store(files.clone()).await.unwrap();
        store.create().await.unwrap();
        let mut device = store.load().await.unwrap().unwrap();
        device.pn = Some(PN.parse().unwrap());
        device.push_name = "Synthetic linked account".into();
        store.save(&device).await.unwrap();
        drop(store);
        let archive = Archive::open(files.clone(), KEY).unwrap();
        archive
            .bind_account(PN.into(), String::new())
            .await
            .unwrap();
        archive
            .ingest(crate::archive::Batch {
                messages: vec![record("preserved")],
                ..Default::default()
            })
            .await
            .unwrap();
        archive.set_needs_link(true).await.unwrap();
        drop(archive);
        let archive = Archive::open(files.clone(), KEY).unwrap();
        assert!(
            archive.needs_link().await.unwrap(),
            "revocation survives restart"
        );
        reset_session(files.clone()).await.unwrap();
        let store = open_store(files).await.unwrap();
        let fresh = store.load().await.unwrap().unwrap();
        assert!(
            fresh.pn.is_none(),
            "next connection must pair instead of restoring rejected credentials"
        );
        assert!(fresh.push_name.is_empty());
        assert_eq!(
            archive
                .view(0, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats
                .len(),
            1
        );
        assert_eq!(
            archive
                .bind_account("15550000002@s.whatsapp.net".into(), String::new())
                .await,
            Err(crate::archive::Error::Identity)
        );
        archive
            .bind_account(PN.into(), String::new())
            .await
            .unwrap();
        archive.set_needs_link(false).await.unwrap();
        assert!(!archive.needs_link().await.unwrap());
    }

    #[tokio::test]
    async fn durability_hook_and_receipts_run_against_synthetic_storage_without_transport() {
        use crate::{
            archive::{
                self,
                tests::{LID, PN, fixture},
            },
            storage::{prepare_at, tests::MemoryVault},
        };
        use whatsapp_rust::{
            InboundDurabilityHook,
            prelude::{InboundMessage, MessageBuilderExt, wa},
            wacore::types::{
                events::{Receipt, ServerAck},
                message::{MessageInfo, MessageSource},
                presence::ReceiptType,
            },
        };
        let (_archive_temp, archive) = fixture();
        let session_temp = tempfile::tempdir().unwrap();
        let store = open_store(prepare_at(session_temp.path(), &MemoryVault::default()).unwrap())
            .await
            .unwrap();
        let bot = Bot::builder().with_backend(store).build().await.unwrap();
        let client = bot.client();
        client
            .persistence_manager()
            .modify_device(|d| {
                d.pn = Some("15550000999@s.whatsapp.net".parse().unwrap());
                d.push_name = "Synthetic owner".into();
            })
            .await;
        let (failure, failed) = watch::channel(None);
        let hook = Ingest {
            archive: archive.clone(),
            failure,
        };
        let source = MessageSource {
            chat: LID.parse().unwrap(),
            sender: LID.parse().unwrap(),
            sender_alt: Some(PN.parse().unwrap()),
            ..Default::default()
        };
        let info = MessageInfo {
            source: source.clone(),
            id: "synthetic-inbound".into(),
            push_name: "Synthetic contact".into(),
            timestamp: chrono::Utc::now(),
            ..Default::default()
        };
        let message = InboundMessage::builder()
            .info(Arc::new(info))
            .message(Arc::new(wa::Message::text("Synthetic live text")))
            .build();
        hook.on_messages(client.clone(), std::slice::from_ref(&message))
            .await
            .unwrap();
        hook.on_messages(client.clone(), std::slice::from_ref(&message))
            .await
            .unwrap();
        let chat = archive.create_chat(PN.into()).await.unwrap();
        let view = archive
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap();
        assert_eq!(view.conversation.unwrap().messages.len(), 1);
        // The protocol marks phone/linked-device reads as ReadSelf, even when
        // source.is_from_me is true. They clear incoming badges, not delivery.
        let own_read = Receipt::builder()
            .source(MessageSource {
                is_from_me: true,
                ..source.clone()
            })
            .message_ids(vec!["synthetic-inbound".into()])
            .timestamp(chrono::Utc::now())
            .r#type(ReceiptType::ReadSelf)
            .offline(true)
            .build();
        protocol_messages::event(&archive, &client, Arc::new(Event::Receipt(own_read)))
            .await
            .unwrap();
        assert_eq!(
            archive
                .view(chat, 100, String::new(), false, false)
                .await
                .unwrap()
                .chats[0]
                .unread,
            0
        );
        archive
            .queue(chat, "synthetic-outbound".into(), "Synthetic reply".into())
            .await
            .unwrap();
        let ack = ServerAck::builder()
            .id("synthetic-outbound".into())
            .class("message".into())
            .from(LID.parse().unwrap())
            .build();
        protocol_messages::event(&archive, &client, Arc::new(Event::ServerAck(ack)))
            .await
            .unwrap();
        let receipt = Receipt::builder()
            .source(source)
            .message_ids(vec!["synthetic-outbound".into()])
            .timestamp(chrono::Utc::now())
            .r#type(ReceiptType::Read)
            .offline(false)
            .build();
        protocol_messages::event(&archive, &client, Arc::new(Event::Receipt(receipt)))
            .await
            .unwrap();
        let view = archive
            .view(chat, 100, String::new(), false, false)
            .await
            .unwrap()
            .conversation
            .unwrap();
        assert!(view.messages.last().unwrap().time.ends_with("Read"));
        // Own-device echoes commonly have no push name. They must still resolve
        // the self-chat title, without naming other outgoing chats after us.
        let own: whatsapp_rust::wacore_binary::Jid = "15550000999@s.whatsapp.net".parse().unwrap();
        let info = MessageInfo {
            source: MessageSource {
                chat: own.clone(),
                sender: own.clone(),
                is_from_me: true,
                ..Default::default()
            },
            id: "synthetic-self-echo".into(),
            timestamp: chrono::Utc::now(),
            ..Default::default()
        };
        let own_message = InboundMessage::builder()
            .info(Arc::new(info))
            .message(Arc::new(wa::Message::text("Synthetic self text")))
            .build();
        hook.on_messages(client.clone(), &[own_message])
            .await
            .unwrap();
        let own_chat = archive
            .create_chat("15550000999@s.whatsapp.net".into())
            .await
            .unwrap();
        assert_eq!(
            archive
                .view(own_chat, 10, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .chat
                .title,
            "Synthetic owner (You)"
        );
        assert_eq!(
            archive
                .view(chat, 10, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .chat
                .title,
            "Synthetic contact"
        );
        // A profile change must reach the UI without waiting for another text.
        client
            .persistence_manager()
            .modify_device(|d| d.push_name = "Renamed owner".into())
            .await;
        let renamed = whatsapp_rust::wacore::types::events::SelfPushNameUpdated::builder()
            .from_server(true)
            .old_name("Synthetic owner".into())
            .new_name("Renamed owner".into())
            .build();
        let (events, mut received) = tokio::sync::mpsc::channel(2);
        let (overflow, _) = watch::channel(false);
        let handler = MessageEvents {
            sender: events,
            overflow,
        };
        assert!(handler.interest().wants(EventKind::SelfPushNameUpdated));
        handler.handle_event(Arc::new(Event::SelfPushNameUpdated(renamed)));
        protocol_messages::event(&archive, &client, received.recv().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            archive
                .view(own_chat, 10, String::new(), false, false)
                .await
                .unwrap()
                .conversation
                .unwrap()
                .chat
                .title,
            "Renamed owner (You)"
        );
        let photo = crate::avatars::Thumbnail::from_pixels(vec![42; 65536]).unwrap();
        let entry = archive
            .avatar_entries(vec![own_chat.into()], false)
            .await
            .unwrap()
            .remove(0);
        archive
            .save_avatar(
                entry,
                crate::avatars::Outcome::Photo {
                    id: "synthetic-picture".into(),
                    photo,
                },
                0,
            )
            .await
            .unwrap();
        assert!(handler.interest().wants(EventKind::PictureUpdate));
        let removed = whatsapp_rust::wacore::types::events::PictureUpdate::builder()
            .jid(own.clone())
            .timestamp(chrono::Utc::now())
            .removed(true)
            .build();
        handler.handle_event(Arc::new(Event::PictureUpdate(removed)));
        protocol_messages::event(&archive, &client, received.recv().await.unwrap())
            .await
            .unwrap();
        assert!(
            !archive
                .avatar_entries(vec![own_chat.into()], true)
                .await
                .unwrap()[0]
                .has_photo
        );
        archive.blocking(|c|{c.execute_batch("CREATE TRIGGER fail_ingest BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'synthetic disk failure'); END;")?;Ok(())}).unwrap();
        assert!(hook.on_messages(client.clone(), &[message]).await.is_err());
        assert_eq!(*failed.borrow(), Some(archive::Error::Database));
        drop(hook);
        let release = client.store_release();
        drop(bot);
        timeout(Duration::from_secs(5), async {
            let report = client.shutdown().await;
            drop(client);
            finish_shutdown(report, release, &test_services()).await
        })
        .await
        .unwrap()
        .unwrap();
    }

    #[test]
    fn qr_has_quiet_zone_and_expired_codes_are_removed() {
        let image = qr_image("synthetic-heya-pairing-payload").unwrap();
        assert!(
            image.pixels[..(image.width * 16 * 4) as usize]
                .iter()
                .all(|byte| *byte == 255)
        );
        assert!(image.pixels.contains(&0));
        let now = Instant::now();
        let expires = now + Duration::from_secs(20);
        assert_eq!(qr_snapshot(image.clone(), expires, now).phase, Phase::Qr);
        assert_eq!(qr_snapshot(image.clone(), expires, now).seconds_left, 20);
        let expired = qr_snapshot(image, expires, expires);
        assert!(expired.qr.is_none());
        assert_eq!(expired.phase, Phase::Connecting);
    }

    #[tokio::test]
    async fn production_store_encrypts_resumes_and_holds_its_file_lock() {
        use crate::storage::{prepare_at, tests::MemoryVault};
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::default();
        let files = prepare_at(temp.path(), &vault).unwrap();
        let path = files.path.clone();
        let store = open_store(files).await.unwrap();
        use whatsapp_rust::store::traits::DeviceStore;
        assert!(!store.exists().await.unwrap());
        assert_eq!(store.create().await.unwrap(), 1);
        let mut device = store.load().await.unwrap().unwrap();
        device.push_name = "Synthetic Heya test".to_owned();
        store.save(&device).await.unwrap();
        assert!(matches!(
            prepare_at(temp.path(), &vault),
            Err(Failure::AlreadyRunning)
        ));
        assert_ne!(&std::fs::read(&path).unwrap()[..16], b"SQLite format 3\0");
        let unkeyed = rusqlite::Connection::open(&path).unwrap();
        assert!(
            unkeyed
                .query_row::<i64, _, _>("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))
                .is_err()
        );
        drop(unkeyed);
        drop(store);
        // r2d2 may release idle connections on its worker threads after Drop.
        let files = timeout(Duration::from_secs(3), async {
            loop {
                match prepare_at(temp.path(), &vault) {
                    Ok(files) => break files,
                    Err(Failure::AlreadyRunning) => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(failure) => panic!("unexpected storage failure: {failure:?}"),
                }
            }
        })
        .await
        .unwrap();
        let store = open_store(files).await.unwrap();
        assert_eq!(store.device_id(), 1);
        assert_eq!(
            store.load().await.unwrap().unwrap().push_name,
            "Synthetic Heya test"
        );
    }

    #[tokio::test]
    async fn cancelled_before_network_start_flushes_and_releases_the_sdk_store() {
        use crate::storage::{prepare_at, tests::MemoryVault};
        let temp = tempfile::tempdir().unwrap();
        let vault = MemoryVault::default();
        let store = open_store(prepare_at(temp.path(), &vault).unwrap())
            .await
            .unwrap();
        // Building loads/provisions storage. Only spawn/run/connect starts the
        // transport; none is called in this offline cancellation regression.
        let bot = Bot::builder().with_backend(store).build().await.unwrap();
        let client = bot.client();
        let release = client.store_release();
        let (audio_sources, _) =
            watch::channel(Some(Arc::new(crate::protocol_audio::Audio(client.clone()))
                as Arc<dyn crate::audio::Source>));
        let (download_sources, _) = watch::channel(Some(Arc::new(
            crate::protocol_downloads::Downloads(client.clone()),
        )
            as Arc<dyn crate::downloads::Source>));
        drop(bot);
        timeout(Duration::from_secs(5), async {
            let report = client.shutdown().await;
            drop(client);
            finish_shutdown(
                report,
                release,
                &LiveServices {
                    audio_sources,
                    download_sources,
                    ..test_services()
                },
            )
            .await
        })
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(3), async {
            loop {
                match prepare_at(temp.path(), &vault) {
                    Ok(files) => {
                        drop(files);
                        break;
                    }
                    Err(Failure::AlreadyRunning) => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                    Err(failure) => panic!("unexpected storage failure: {failure:?}"),
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn idle_or_cancelled_startup_never_opens_an_account() {
        let (requests, receiver) = watch::channel(Request::Idle);
        let (updates, snapshots) = watch::channel(Snapshot::new(Phase::Idle));
        requests.send_replace(Request::Connect);
        requests.send_replace(Request::Disconnect);
        drop(requests);
        run(receiver, updates, watch::channel(None).0, test_services()).await;
        assert_eq!(snapshots.borrow().phase, Phase::Idle);
    }

    pub(super) fn test_services() -> LiveServices {
        LiveServices {
            preferences: watch::channel(crate::preferences::Preferences::default()).1,
            avatar_demand: watch::channel(Vec::new()).1,
            photo_demand: watch::channel(crate::photos::Demand::default()).1,
            thumbnail_demand: watch::channel(Vec::new()).1,
            audio_sources: watch::channel(None).0,
            video_sources: watch::channel(None).0,
            call_sources: watch::channel(None).0,
            history_sources: watch::channel(None).0,
            chat_sources: watch::channel(None).0,
            download_sources: watch::channel(None).0,
            upload_sources: watch::channel(None).0,
            presence: crate::presence::Channels {
                selected: watch::channel(0).1,
                snapshots: watch::channel(crate::presence::Snapshot::default()).0,
            },
        }
    }

    #[test]
    fn terminal_cause_survives_trailing_events() {
        use whatsapp_rust::types::events::{Connected, PairingQrCode, PairingQrCodesExhausted};
        let (sender, receiver) = watch::channel(Observation::Phase(Phase::Connecting));
        let handler = ConnectionEvents(sender);
        handler.handle_event(Arc::new(Event::PairingQrCode(
            PairingQrCode::builder()
                .code("synthetic-only".into())
                .timeout(Duration::from_secs(10))
                .build(),
        )));
        assert!(matches!(*receiver.borrow(), Observation::Qr { .. }));
        handler.handle_event(Arc::new(Event::PairingQrCodesExhausted(
            PairingQrCodesExhausted::builder()
                .disconnected(true)
                .build(),
        )));
        handler.handle_event(Arc::new(Event::Connected(Connected::builder().build())));
        assert!(matches!(
            *receiver.borrow(),
            Observation::Phase(Phase::Failed(Failure::QrExpired))
        ));
    }
}

#[cfg(test)]
#[path = "protocol_shutdown_tests.rs"]
mod shutdown_tests;
