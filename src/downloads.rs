//! One explicit file save at a time, independent of chat navigation and rendering.
#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
use crate::{
    archive::Archive,
    downloads_file::{Partial, Sink},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{mpsc, watch};

pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const SAVED_FOR: std::time::Duration = std::time::Duration::from_secs(3);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Offline,
    TooLarge,
    Network,
    Integrity,
    Storage,
    Destination,
    DestinationChanged,
    Picker,
    Cancelled,
    Archive,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => {
                "This attachment is unavailable. Try again or ask the sender to resend it."
            }
            Self::Offline => {
                "Connect to download this attachment. Cached photos and audio can be saved offline."
            }
            Self::TooLarge => "This attachment exceeds the 2 GiB download limit.",
            Self::Network => "Download failed. Check your connection and retry.",
            Self::Integrity => "The attachment could not be verified. No file was saved.",
            Self::Storage => {
                "The file could not be written. Check free space and folder permissions."
            }
            Self::Destination => "Choose a writable folder and a regular file destination.",
            Self::DestinationChanged => {
                "The destination changed during download. Choose it again to retry."
            }
            Self::Picker => {
                if cfg!(target_os = "macos") {
                    "The native file chooser is unavailable. Close other dialogs and try again."
                } else {
                    "The native Save As dialog is unavailable. Check your desktop file chooser portal."
                }
            }
            Self::Cancelled => "Download cancelled.",
            Self::Archive => "The encrypted attachment could not be read.",
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Photo,
    Audio,
    Document,
    Video,
    Sticker,
}
pub struct Info {
    pub name: String,
    pub size: Option<u64>,
    pub sha256: [u8; 32],
    pub kind: Kind,
}
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub chat: i32,
    pub id: i32,
    pub raw: Vec<u8>,
}
#[derive(Clone)]
pub struct Control {
    cancel: watch::Sender<bool>,
    flag: Arc<AtomicBool>,
}
impl Default for Control {
    fn default() -> Self {
        Self {
            cancel: watch::channel(false).0,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl Control {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        self.cancel.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
    pub async fn cancelled(&self) {
        let mut rx = self.cancel.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }
}
struct CancelOnDrop(Control);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[derive(Clone, Copy, Default)]
pub struct Progress {
    pub bytes: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum Phase {
    #[default]
    Hidden = 0,
    Choosing = 1,
    Downloading = 2,
    Committing = 3,
    Saved = 4,
    Failed = 5,
    Cancelled = 6,
}
#[derive(Clone, Default)]
pub struct Status {
    pub phase: Phase,
    pub name: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub error: Option<Error>,
}
#[derive(Clone)]
pub struct Target {
    pub chat: i32,
    pub message: i32,
    pub parent: String,
    pub demo: Option<Kind>,
}
pub enum Command {
    Start(Target),
    Cancel,
    Retry,
    Dismiss,
}
#[async_trait::async_trait]
pub trait Source: Send + Sync {
    /// Return success only after MAC and both declared hashes are verified.
    async fn fetch(&self, raw: &[u8], sink: Sink) -> Result<Sink, Error>;
}
pub type Sources = Option<Arc<dyn Source>>;
#[async_trait::async_trait]
pub trait Picker: Send + Sync {
    async fn save(
        &self,
        name: &str,
        parent: &str,
        control: &Control,
    ) -> Result<Option<PathBuf>, Error>;
}

struct Job {
    archive: Option<Archive>,
    sources: watch::Receiver<Sources>,
    picker: Arc<dyn Picker>,
    target: Target,
    control: Control,
    updates: watch::Sender<Status>,
}
async fn save(job: Job) -> Result<(), Error> {
    let Job {
        archive,
        mut sources,
        picker,
        target,
        control,
        updates,
    } = job;
    let Target {
        chat,
        message,
        parent,
        demo,
    } = target;
    let _cancel = CancelOnDrop(control.clone());
    let (request, mut info, cached) = if let Some(kind) = demo {
        let (info, bytes) = tokio::task::spawn_blocking(move || demo_file(kind))
            .await
            .map_err(|_| Error::Storage)??;
        (None, info, Some(bytes))
    } else {
        let archive = archive.as_ref().ok_or(Error::Archive)?;
        let request = archive
            .download_request(chat, message)
            .await
            .map_err(|_| Error::Archive)?
            .ok_or(Error::Unavailable)?;
        let info = crate::protocol_downloads::info(&request.raw)?;
        if info.size.is_some_and(|n| n > MAX_BYTES) {
            return Err(Error::TooLarge);
        }
        let cached = archive
            .download_cached(&request, info.kind)
            .await
            .map_err(|_| Error::Archive)?;
        (Some(request), info, cached)
    };
    updates.send_replace(Status {
        phase: Phase::Choosing,
        name: info.name.clone(),
        total: info.size,
        ..Default::default()
    });
    if control.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if info.size.is_some_and(|n| n > MAX_BYTES) {
        return Err(Error::TooLarge);
    }
    let cached = if let Some(bytes) = cached {
        let hash = info.sha256;
        let checked = tokio::task::spawn_blocking(move || {
            use sha2::{Digest, Sha256};
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            (digest == hash).then_some(bytes)
        })
        .await
        .map_err(|_| Error::Storage)?;
        if checked.is_none() && sources.borrow().is_none() {
            return Err(Error::Integrity);
        }
        checked
    } else {
        None
    };
    if cached.is_none() && sources.borrow().is_none() {
        return Err(Error::Offline);
    }
    let path = picker
        .save(&info.name, &parent, &control)
        .await?
        .ok_or(Error::Cancelled)?;
    if control.is_cancelled() {
        return Err(Error::Cancelled);
    }
    info.name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or(info.name);
    let (progress, mut progress_rx) = watch::channel(Progress::default());
    let c = control.clone();
    let (partial, sink) = tokio::task::spawn_blocking(move || Partial::create(path, c, progress))
        .await
        .map_err(|_| Error::Storage)??;
    updates.send_replace(Status {
        phase: Phase::Downloading,
        name: info.name.clone(),
        total: info.size,
        ..Default::default()
    });
    let work = async {
        let sink = if let Some(bytes) = cached {
            tokio::task::spawn_blocking(move || {
                use std::io::Write;
                let mut sink = sink;
                for chunk in bytes.chunks(64 * 1024) {
                    sink.write_all(chunk).map_err(|_| {
                        if sink.control_cancelled() {
                            Error::Cancelled
                        } else {
                            Error::Storage
                        }
                    })?;
                }
                Ok(sink)
            })
            .await
            .map_err(|_| Error::Storage)??
        } else {
            let source = sources.borrow_and_update().clone().ok_or(Error::Offline)?;
            tokio::select! {
                biased;
                _=control.cancelled()=>return Err(Error::Cancelled),
                _=sources.changed()=>return Err(Error::Offline),
                result=tokio::time::timeout(std::time::Duration::from_secs(600),source.fetch(&request.as_ref().ok_or(Error::Unavailable)?.raw,sink))=>result.map_err(|_|Error::Network)??,
            }
        };
        if info.size.is_some_and(|n| n > 0 && n != sink.bytes) {
            return Err(Error::Integrity);
        }
        if let Some(request) = &request
            && archive
                .as_ref()
                .ok_or(Error::Archive)?
                .download_request(chat, message)
                .await
                .map_err(|_| Error::Archive)?
                .as_ref()
                != Some(request)
        {
            return Err(Error::Unavailable);
        }
        let bytes = sink.bytes;
        updates.send_replace(Status {
            phase: Phase::Committing,
            name: info.name.clone(),
            bytes,
            total: info.size,
            ..Default::default()
        });
        tokio::task::spawn_blocking(move || partial.publish(sink))
            .await
            .map_err(|_| Error::Storage)??;
        updates.send_replace(Status {
            phase: Phase::Saved,
            name: info.name.clone(),
            bytes,
            total: info.size,
            ..Default::default()
        });
        Ok(())
    };
    tokio::pin!(work);
    let mut progress_open = true;
    loop {
        tokio::select! {
            result = &mut work => return result,
            changed = progress_rx.changed(), if progress_open => {
                if changed.is_ok() {
                    let progress = *progress_rx.borrow_and_update();
                    updates.send_modify(|status| {
                        if status.phase == Phase::Downloading {status.bytes = progress.bytes;}
                    });
                } else {
                    progress_open = false;
                }
            }
        }
    }
}

pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    sources: watch::Receiver<Sources>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    picker: Arc<dyn Picker>,
) {
    type Work = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send>>;
    let mut work: Option<Work> = None;
    let mut active: Option<Target> = None;
    let mut control = Control::default();
    let mut hide_at = None;
    loop {
        tokio::select! {
            result = async {match &mut work {Some(work) => work.await, None => std::future::pending().await}} => {
                work = None;
                if let Err(error) = result {
                    updates.send_modify(|status| {
                        status.phase = if error == Error::Cancelled {Phase::Cancelled} else {Phase::Failed};
                        status.error = Some(error);
                    });
                } else {
                    hide_at = Some(tokio::time::Instant::now() + SAVED_FOR);
                }
            }
            // One deadline after success, with no idle polling. A new transfer
            // or explicit dismissal clears it before it can hide newer state.
            _ = async {
                match hide_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                hide_at = None;
                active = None;
                updates.send_replace(Status::default());
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    control.cancel();
                    if let Some(work) = work {
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), work).await;
                    }
                    return;
                };
                let start = matches!(&command, Command::Start(_) | Command::Retry);
                match command {
                    Command::Cancel => control.cancel(),
                    Command::Dismiss if work.is_none() => {
                        hide_at = None;
                        updates.send_replace(Status::default());
                        active = None;
                    }
                    Command::Start(target) if work.is_none() => {active = Some(target);}
                    Command::Retry if work.is_none() => {},
                    _ => continue,
                }
                if work.is_none() && start
                    && let Some(target) = &active {
                    hide_at = None;
                    control = Control::default();
                    updates.send_replace(Status {phase:Phase::Choosing, ..Default::default()});
                    work = Some(Box::pin(save(Job {
                        archive:archives.borrow().clone(), sources:sources.clone(),
                        picker:picker.clone(), target:target.clone(),
                        control:control.clone(), updates:updates.clone(),
                    })));
                }
            }
        }
    }
}

fn demo_file(kind: Kind) -> Result<(Info, Vec<u8>), Error> {
    use sha2::{Digest, Sha256};
    let (name, bytes) = match kind {
        Kind::Photo => {
            use image::ImageEncoder;
            let p = crate::fixtures::photo_pixels(0, 640);
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(&p.rgba, p.width, p.height, image::ExtendedColorType::Rgba8)
                .map_err(|_| Error::Storage)?;
            ("Heya-demo-photo.png", bytes)
        }
        Kind::Audio => ("Heya-demo-audio.wav", crate::fixtures::audio_bytes()),
        _ => (
            "Heya-demo-attachment.txt",
            b"Heya synthetic demonstration attachment.\nNo account data was used.\n".to_vec(),
        ),
    };
    Ok((
        Info {
            name: name.into(),
            size: Some(bytes.len() as u64),
            sha256: Sha256::digest(&bytes).into(),
            kind,
        },
        bytes,
    ))
}
