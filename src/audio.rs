//! Received audio orchestration. One explicit request, one player, bounded bytes.
use crate::{
    archive::Archive,
    audio_player::{self, Desired, Output, Worker},
};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};

pub const MAX_BYTES: usize = 20 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NotBuilt,
    Offline,
    Download,
    Unavailable,
    TooLarge,
    #[cfg_attr(not(feature = "media-gstreamer"), allow(dead_code))]
    TooLong,
    Archive,
    #[cfg_attr(not(feature = "media-gstreamer"), allow(dead_code))]
    Codec,
    Playback,
    #[cfg_attr(not(feature = "media-gstreamer"), allow(dead_code))]
    Seek,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotBuilt => "Audio playback is disabled in this build",
            Self::Offline => "Connect to download this audio",
            Self::Download => "Download failed · click to retry",
            Self::Unavailable => "Audio unavailable · ask the sender to resend",
            Self::TooLarge => "Audio exceeds the 20 MiB limit",
            Self::TooLong => "Audio exceeds the six-hour playback limit",
            Self::Archive => "Could not read or save audio",
            Self::Codec => "Required GStreamer audio plugin is missing",
            Self::Playback => "Could not play audio · check the format and audio output",
            Self::Seek => "Seeking or speed change is unavailable for this audio",
        }
    }
}
#[derive(Clone, Default, PartialEq)]
pub struct Status {
    pub generation: u64,
    pub id: i32,
    /// 0 ended/idle, 1 loading, 2 playing, 3 paused, 4 error.
    pub phase: i32,
    pub position: f64,
    pub duration: f64,
    pub speed: f64,
    pub error: Option<Error>,
}
impl Status {
    pub fn fail(&mut self, error: Error) {
        self.phase = 4;
        self.error = Some(error);
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub id: i32,
    pub raw: Vec<u8>,
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    async fn fetch(&self, raw: &[u8]) -> Result<Vec<u8>, Error>;
}
pub type Sources = Option<Arc<dyn Source>>;
pub enum Command {
    Toggle(i32),
    Seek(i32, f64),
    Speed,
    Stop,
}
type Loading = Pin<Box<dyn Future<Output = Result<(Option<Request>, Arc<[u8]>), Error>> + Send>>;

async fn load(
    archive: Option<Archive>,
    source: Sources,
    id: i32,
    demo: bool,
) -> Result<(Option<Request>, Arc<[u8]>), Error> {
    if !cfg!(feature = "media-gstreamer") {
        return Err(Error::NotBuilt);
    }
    if demo {
        return Ok((None, crate::fixtures::audio_bytes().into()));
    }
    let archive = archive.ok_or(Error::Archive)?;
    let request = archive
        .audio_request(id)
        .await
        .map_err(|_| Error::Archive)?
        .ok_or(Error::Unavailable)?;
    let bytes = if let Some(bytes) = archive
        .cached_audio(&request)
        .await
        .map_err(|_| Error::Archive)?
    {
        bytes
    } else {
        let bytes = source.ok_or(Error::Offline)?.fetch(&request.raw).await?;
        if bytes.len() > MAX_BYTES || bytes.is_empty() {
            return Err(Error::TooLarge);
        }
        if !archive
            .save_audio(&request, bytes.clone())
            .await
            .map_err(|_| Error::Archive)?
        {
            return Err(Error::Unavailable);
        }
        bytes
    };
    if archive
        .audio_request(id)
        .await
        .map_err(|_| Error::Archive)?
        .as_ref()
        != Some(&request)
    {
        return Err(Error::Unavailable);
    }
    Ok((Some(request), bytes.into()))
}

/// The async service owns download/archive operations. GStreamer and sink
/// shutdown live on a separate blocking worker; neither can stall Slint.
pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<Sources>,
    commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) {
    run_with_player(
        archives,
        sources,
        commands,
        updates,
        demo,
        move |desired, updates| {
            let output = if demo || cfg!(test) {
                Output::Silent
            } else {
                Output::Desktop(preferences.borrow().audio.clone())
            };
            audio_player::Worker::start(desired, updates, output)
        },
    )
    .await;
}

async fn run_with_player(
    mut archives: watch::Receiver<Option<Archive>>,
    mut sources: watch::Receiver<Sources>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    demo: bool,
    mut start_player: impl FnMut(Desired, watch::Sender<Status>) -> Result<Worker, Error>,
) {
    let (status_tx, mut status_rx) = watch::channel(Status::default());
    let mut player: Option<Worker> = None;
    let mut desired = Desired {
        speed: 1.,
        ..Default::default()
    };
    let mut status = Status::default();
    let mut loading: Option<Loading> = None;
    let mut request: Option<Request> = None;
    let mut serial = 0u64;
    let mut sources_open = true;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (_idle, mut mutations) = watch::channel(0u64);
    if let Some(archive) = &*archives.borrow() {
        mutations = archive.subscribe();
    }
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    Command::Stop => {
                        loading = None;
                        request = None;
                        status = Status::default();
                        desired.clear();
                    }
                    Command::Toggle(id) if id == status.id && matches!(status.phase, 2 | 3) => {
                        desired.playing = !desired.playing;
                    }
                    Command::Toggle(id) => {
                        // A second click cancels loading. Failed/ended clips
                        // need a fresh explicit attempt; never retry in a loop.
                        desired.clear();
                        request = None;
                        if id == status.id && status.phase == 1 {
                            loading = None;
                            status = Status::default();
                        } else {
                            status = Status { id, phase: 1, speed: desired.speed, ..Default::default() };
                            loading = Some(Box::pin(load(
                                archives.borrow().clone(), sources.borrow().clone(), id, demo,
                            )));
                        }
                    }
                    Command::Seek(id, fraction)
                        if id == status.id && matches!(status.phase, 2 | 3) && fraction.is_finite() =>
                    {
                        serial = serial.wrapping_add(1);
                        desired.seek = Some((serial, fraction.clamp(0., 1.)));
                    }
                    Command::Speed => {
                        desired.speed = if desired.speed < 1.5 { 1.5 } else if desired.speed < 2. { 2. } else { 1. };
                        status.speed = desired.speed;
                    }
                    _ => {}
                }
                updates.send_replace(status.clone());
            }
            result = async { loading.as_mut().expect("guarded load").await }, if loading.is_some() => {
                loading = None;
                match result {
                    Ok((guard, bytes)) => {
                        request = guard;
                        desired = Desired {
                            generation: desired.generation.wrapping_add(1),
                            id: status.id, bytes: Some(bytes), playing: true,
                            speed: desired.speed, seek: None,
                        };
                    }
                    Err(error) => {
                        status.fail(error);
                        updates.send_replace(status.clone());
                    }
                }
            }
            // status_tx stays alive for subsequent players. Completion/failure
            // is observed separately, after the thread tears down its pipeline.
            _ = status_rx.changed(), if player.is_some() => {
                let next = status_rx.borrow_and_update().clone();
                if loading.is_none() && next.generation == desired.generation
                    && next.id == status.id && next.id != 0
                {
                    status = next;
                    if matches!(status.phase, 0 | 4) {
                        request = None;
                        desired.clear();
                    }
                    updates.send_replace(status.clone());
                }
            }
            result = async {
                player.as_mut().expect("guarded player").finished().await
            }, if player.is_some() => {
                let finished = player.take().expect("guarded player");
                if finished.generation == desired.generation {
                    match result {
                        Ok(next) => status = next,
                        Err(error) => status.fail(error),
                    }
                    request = None;
                    desired.clear();
                    updates.send_replace(status.clone());
                }
                // A replacement can start below only after teardown completes.
            }
            result = archives.changed() => {
                if result.is_err() { break; }
                loading = None;
                request = None;
                status = Status::default();
                desired.clear();
                updates.send_replace(status.clone());
                if let Some(archive) = &*archives.borrow_and_update() {
                    mutations = archive.subscribe();
                }
            }
            result = sources.changed(), if !demo && sources_open => {
                if result.is_err() { sources_open = false; }
                if sources.borrow().is_none() && loading.is_some() {
                    loading = None;
                    status.fail(Error::Offline);
                    updates.send_replace(status.clone());
                }
            }
            _ = async {
                tokio::select! { _ = tick.tick() => {}, _ = mutations.changed() => {} }
            }, if request.is_some() => {
                let guard = request.as_ref().expect("guarded request");
                let archive = archives.borrow().clone();
                let valid = if let Some(archive) = archive {
                    archive.audio_request(guard.id).await.is_ok_and(|r| r.as_ref() == Some(guard))
                } else { false };
                if !valid {
                    request = None;
                    loading = None;
                    status.fail(Error::Unavailable);
                    desired.clear();
                    updates.send_replace(status.clone());
                }
            }
        }
        if let Some(player) = &player {
            player.update(&desired);
        } else if desired.bytes.is_some() {
            match start_player(desired.clone(), status_tx.clone()) {
                Ok(started) => player = Some(started),
                Err(error) => {
                    request = None;
                    desired.clear();
                    status.fail(error);
                    updates.send_replace(status.clone());
                }
            }
        }
    }
    // Cancel network work before joining the player, so shutdown never retains
    // a protocol client while a sink/decoder is stopping.
    drop(loading);
    desired.clear();
    if let Some(mut player) = player {
        player.stop();
        let _ = player.finished().await;
    }
}

#[cfg(all(test, feature = "media-gstreamer"))]
#[path = "audio_tests.rs"]
mod tests;
