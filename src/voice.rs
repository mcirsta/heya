//! Recording/review orchestration. Capture and preview workers exist only on demand.
use crate::{
    archive::Archive,
    attachments::{self, Destination},
    downloads::Control,
    media_process,
    voice_recorder::{self as recorder, Clip, Desired, Input},
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
#[derive(Clone, Default, PartialEq)]
pub struct Status {
    /// 0 idle, 1 recording, 2 paused, 3 finishing, 4 review, 5 send handoff, 6 checking input.
    pub phase: i32,
    pub seconds: f64,
    pub position: f64,
    pub playing: bool,
    pub levels: Vec<u8>,
    pub error: String,
    pub failure: Option<recorder::Error>,
}
pub enum Command {
    Start { chat: i32, title: String },
    Cancel,
    Pause,
    Finish,
    Preview,
    Send,
    ClearReply,
}
type Preparing = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;
pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    mut sources: watch::Receiver<attachments::Sources>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    uploads: mpsc::Sender<attachments::Command>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) {
    let (progress, mut progress_rx) = watch::channel(recorder::Progress::default());
    let (play_updates, mut play_rx) = watch::channel(recorder::Progress::default());
    let mut worker: Option<recorder::Worker> = None;
    let mut player: Option<media_process::Worker> = None;
    let mut clip: Option<Clip> = None;
    let mut destination: Option<Destination> = None;
    let mut preparing: Option<Preparing> = None;
    let mut status = Status::default();
    let mut sources_open = true;
    loop {
        tokio::select! {
            result = async {
                match &mut worker {
                    Some(w) => w.finished().await,
                    None => std::future::pending().await,
                }
            } => {
                worker = None;
                match result {
                    Ok(c) => {
                        status = Status {
                            phase: 4,
                            seconds: f64::from(c.seconds),
                            levels: c.waveform.clone(),
                            ..Default::default()
                        };
                        clip = Some(c);
                    }
                    Err(e) => {
                        clip = None;
                        destination = None;
                        status = Status {
                            failure: (e != recorder::Error::Cancelled).then_some(e),
                            error: if e == recorder::Error::Cancelled {
                                String::new()
                            } else {
                                e.label().into()
                            },
                            ..Default::default()
                        };
                    }
                }
            }
            result = async {
                match &mut player {
                    Some(p) => p.finished().await,
                    None => std::future::pending().await,
                }
            } => {
                player = None;
                status.playing = false;
                status.position = 0.;
                if let Err(e) = result
                    && e != recorder::Error::Cancelled
                {
                    status.error = e.label().into();
                }
            }
            result = async {
                match &mut preparing {
                    Some(p) => p.await,
                    None => std::future::pending().await,
                }
            } => {
                preparing = None;
                match result {
                    Ok(()) => {
                        clip = None;
                        destination = None;
                        status = Status::default();
                    }
                    Err(e) => {
                        status.phase = 4;
                        status.error = e;
                    }
                }
            }
            changed = progress_rx.changed(), if worker.is_some() => {
                if changed.is_ok() {
                    let p = progress_rx.borrow_and_update();
                    if status.phase == 6 && p.seconds > 0. {
                        status.phase = 1;
                    }
                    status.seconds = p.seconds;
                    status.levels = p.levels.clone();
                }
            }
            changed = play_rx.changed(), if player.is_some() => {
                if changed.is_ok() {
                    let p = play_rx.borrow_and_update();
                    status.position = p.seconds;
                }
            }
            changed = sources.changed(), if sources_open => {
                sources_open = changed.is_ok();
                if !demo && sources.borrow().is_none()
                    && let Some(w) = &worker
                {
                    w.set(Desired::Finish);
                    status.phase = 3;
                    status.error =
                        "Recording stopped while disconnected. You can send after reconnecting.".into();
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    if let Some(w) = &mut worker {
                        w.set(Desired::Cancel);
                        let _ = tokio::time::timeout(Duration::from_secs(5), w.finished()).await;
                    }
                    if let Some(p) = &mut player {
                        p.stop();
                        let _ = tokio::time::timeout(Duration::from_secs(5), p.finished()).await;
                    }
                    return;
                };
                match command {
                    Command::Start { chat, title }
                        if worker.is_none() && clip.is_none() && preparing.is_none() =>
                    {
                        status = Status::default();
                        if !demo && sources.borrow().is_none() {
                            status.error = "Connect to record a voice message.".into();
                        } else {
                            let d = if demo {
                                Ok(Destination {
                                    chat,
                                    title: title.clone(),
                                    reply: None,
                                    owner: String::new(),
                                })
                            } else {
                                let archive = archives.borrow().clone();
                                match archive {
                                    Some(a) => a.attachment_destination(chat).await.map_err(|_| ()),
                                    None => Err(()),
                                }
                            };
                            match d {
                                Ok(mut d) => {
                                    d.title = title;
                                    progress.send_replace(recorder::Progress::default());
                                    match recorder::Worker::start(
                                        if demo && std::env::var_os("HEYA_TEST_NO_MICROPHONE").as_deref() == Some(std::ffi::OsStr::new("1")) {
                                            Input::Unavailable
                                        } else if demo || cfg!(test) {
                                            Input::Synthetic
                                        } else {
                                            Input::Microphone
                                        },
                                        progress.clone(),
                                        preferences.borrow().audio.clone(),
                                    ) {
                                        Ok(w) => {
                                            worker = Some(w);
                                            destination = Some(d);
                                            status = Status {
                                                phase: 6,
                                                ..Default::default()
                                            };
                                        }
                                        Err(e) => {
                                            status = Status {
                                                error: e.label().into(),
                                                failure: Some(e),
                                                ..Default::default()
                                            };
                                        }
                                    }
                                }
                                Err(_) => {
                                    status.error = "This conversation cannot receive a voice message.".into()
                                }
                            }
                        }
                    }
                    Command::ClearReply if preparing.is_none() => {
                        if let Some(d) = &mut destination {
                            d.reply = None;
                        }
                    }
                    Command::Pause => {
                        if let Some(w) = &worker
                            && matches!(status.phase, 1 | 2)
                        {
                            let pause = status.phase == 1;
                            w.set(if pause {
                                Desired::Pause
                            } else {
                                Desired::Record
                            });
                            status.phase = if pause { 2 } else { 1 };
                        }
                    }
                    Command::Finish => {
                        if let Some(w) = &worker {
                            w.set(Desired::Finish);
                            status.phase = 3;
                        }
                    }
                    Command::Cancel => {
                        preparing = None;
                        if let Some(p) = &mut player {
                            p.stop();
                            let _ = tokio::time::timeout(Duration::from_secs(5), p.finished()).await;
                        }
                        player = None;
                        if let Some(w) = &worker {
                            w.set(Desired::Cancel);
                            status.phase = 3;
                        } else {
                            clip = None;
                            destination = None;
                            status = Status::default();
                        }
                    }
                    Command::Preview if preparing.is_none() && clip.is_some() => {
                        if let Some(p) = &mut player {
                            p.stop();
                            let _ = tokio::time::timeout(Duration::from_secs(5), p.finished()).await;
                            player = None;
                            status.playing = false;
                            status.position = 0.;
                        } else {
                            play_updates.send_replace(recorder::Progress::default());
                            status.position = 0.;
                            let mode = media_process::Mode::Preview {
                                bytes: clip.as_ref().unwrap().bytes.clone(),
                                silent: demo || cfg!(test),
                                settings: preferences.borrow().audio.clone(),
                            };
                            match media_process::Worker::start(mode, play_updates.clone()) {
                                Ok(p) => {
                                    player = Some(p);
                                    status.playing = true;
                                    status.error.clear();
                                }
                                Err(e) => status.error = e.label().into(),
                            }
                        }
                    }
                    Command::Send if status.phase == 4 && !demo => {
                        if sources.borrow().is_none() {
                            status.error = "Connect to send this voice message.".into();
                        } else if let (Some(c), Some(d)) = (clip.clone(), destination.take()) {
                            if let Some(p) = &mut player {
                                p.stop();
                                let _ = tokio::time::timeout(Duration::from_secs(5), p.finished()).await;
                            }
                            player = None;
                            let uploads = uploads.clone();
                            let control = Control::default();
                            status.phase = 5;
                            status.playing = false;
                            status.error.clear();
                            // Keep the destination and plaintext review data until handoff succeeds.
                            destination = Some(Destination {
                                chat: d.chat,
                                title: d.title.clone(),
                                reply: d.reply.clone(),
                                owner: d.owner.clone(),
                            });
                            preparing = Some(Box::pin(async move {
                                let file = crate::protocol_uploads::stage_voice(c, control)
                                    .await
                                    .map_err(|_| {
                                        "Could not prepare the voice message. Your recording is retained."
                                            .to_string()
                                    })?;
                                let (accepted, result) = tokio::sync::oneshot::channel();
                                uploads
                                    .send(attachments::Command::Voice {
                                        destination: d,
                                        file,
                                        accepted,
                                    })
                                    .await
                                    .map_err(|_| "Attachment service is unavailable.".to_string())?;
                                result
                                    .await
                                    .map_err(|_| "Could not hand off the voice message.".to_string())?
                                    .map_err(|_| {
                                        "Finish the current attachment transfer before sending this recording."
                                            .to_string()
                                    })
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        updates.send_if_modified(|s| {
            if *s == status {
                false
            } else {
                *s = status.clone();
                true
            }
        });
    }
}

#[cfg(all(test, feature = "media-gstreamer"))]
#[path = "voice_tests.rs"]
mod tests;
