//! Explicit playback of one received video. Decoders exist only during playback.
use crate::{
    archive::Archive,
    photos::Pixels,
    video_process::{Control, Worker},
};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{mpsc, watch};

pub const MAX_BYTES: usize = 100 * 1024 * 1024;
pub const EDGE: u32 = 960;
pub const MAX_SECONDS: f64 = 6. * 3600.;
pub const SUPPORTED: bool = cfg!(any(
    target_os = "macos",
    all(target_os = "linux", feature = "media-gstreamer")
));

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    NotBuilt,
    Offline,
    Download,
    Unavailable,
    TooLarge,
    Archive,
    Codec,
    Playback,
    Seek,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotBuilt => "Video playback is disabled in this build",
            Self::Offline => "Connect to download this video",
            Self::Download => "Video download failed · click to retry",
            Self::Unavailable => "Video unavailable · ask the sender to resend it",
            Self::TooLarge => "Video exceeds the 100 MiB playback limit. Use Download to save it.",
            Self::Archive => "Could not read or save this video",
            Self::Codec => "This video format could not be decoded",
            Self::Playback => "Video playback failed · click to retry",
            Self::Seek => "Seeking is unavailable for this video",
        }
    }
}
#[derive(Clone, Default)]
pub struct Status {
    pub id: i32,
    /// 0 stopped/ended, 1 loading, 2 playing, 3 paused, 4 error.
    pub phase: i32,
    pub position: f64,
    pub duration: f64,
    pub muted: bool,
    pub frame: Option<Arc<Pixels>>,
    pub frame_number: i32,
    pub error: Option<Error>,
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
    Mute(i32),
    Stop,
}
type Loading = Pin<Box<dyn Future<Output = Result<(Option<Request>, Vec<u8>), Error>> + Send>>;

async fn load(
    db: Option<Archive>,
    source: Sources,
    id: i32,
    demo: bool,
) -> Result<(Option<Request>, Vec<u8>), Error> {
    if !SUPPORTED {
        return Err(Error::NotBuilt);
    }
    if demo {
        // Only the explicit synthetic benchmark supplies a source here. The
        // ordinary demo has no account, files or network and uses generated frames.
        return Ok((
            None,
            match source {
                Some(source) => source.fetch(&[]).await?,
                None => Vec::new(),
            },
        ));
    }
    let db = db.ok_or(Error::Archive)?;
    let request = db
        .video_request(id)
        .await
        .map_err(|_| Error::Archive)?
        .ok_or(Error::Unavailable)?;
    let bytes = if let Some(bytes) = db
        .cached_video(&request)
        .await
        .map_err(|_| Error::Archive)?
    {
        bytes
    } else {
        let bytes = source.ok_or(Error::Offline)?.fetch(&request.raw).await?;
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        if !db
            .save_video(&request, bytes.clone())
            .await
            .map_err(|_| Error::Archive)?
        {
            return Err(Error::Unavailable);
        }
        bytes
    };
    if db
        .video_request(id)
        .await
        .map_err(|_| Error::Archive)?
        .as_ref()
        != Some(&request)
    {
        return Err(Error::Unavailable);
    }
    Ok((Some(request), bytes))
}

pub async fn run(
    mut archives: watch::Receiver<Option<Archive>>,
    mut sources: watch::Receiver<Sources>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Status>,
    demo: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) {
    let mut db = archives.borrow_and_update().clone();
    let mut changed = db.as_ref().map(Archive::subscribe);
    let mut loading: Option<Loading> = None;
    let mut request: Option<Request> = None;
    let mut worker: Option<Worker> = None;
    let (frames, mut snapshots) = watch::channel(Status::default());
    let mut status = Status::default();
    let mut control = Control::default();
    let mut sources_open = true;
    let mut archives_open = true;
    loop {
        tokio::select! {
            biased;
            command=commands.recv()=>{
                let Some(command)=command else {break;};
                match command {
                    Command::Toggle(id) if id==status.id && worker.is_some()=>{
                        control.playing = !control.playing;
                        worker.as_ref().unwrap().set(&control);
                    }
                    Command::Toggle(id) if id>0=>{
                        drop(loading.take());
                        if let Some(mut old)=worker.take() {old.stop().await;} frames.send_replace(Status::default());
                        request=None;
                        control=Control::default();
                        status=Status {id,phase:1,..Default::default()};
                        updates.send_replace(status.clone());
                        loading=Some(Box::pin(load(db.clone(),sources.borrow().clone(),id,demo)));
                    }
                    Command::Seek(id,fraction) if id==status.id && fraction.is_finite() && status.duration>0.=>{
                        control.seek=Some((control.seek.map_or(0,|s|s.0).wrapping_add(1),fraction.clamp(0.,1.)*status.duration));
                        if let Some(worker)=&worker {worker.set(&control);}
                    }
                    Command::Mute(id) if id==status.id=>{
                        control.muted = !control.muted;
                        if let Some(worker)=&worker {worker.set(&control);}
                    }
                    Command::Stop=>{
                        loading=None;request=None;
                        if let Some(mut old)=worker.take() {old.stop().await;} frames.send_replace(Status::default());
                        status=Status::default();updates.send_replace(status.clone());
                    }
                    _=>{}
                }
            }
            result=async {loading.as_mut().unwrap().await},if loading.is_some()=>{
                loading=None;
                match result {
                    Ok((current,bytes))=>{
                        request=current;
                        frames.send_replace(Status::default());
                        snapshots.borrow_and_update();
                        let settings=preferences.borrow().audio.clone();
                        let generated=demo && bytes.is_empty();
                        match Worker::start(status.id,bytes,generated,demo || cfg!(test),settings,frames.clone()) {
                            Ok(player)=>worker=Some(player),
                            Err(error)=>{status.phase=4;status.error=Some(error);updates.send_replace(status.clone());}
                        }
                    }
                    Err(error)=>{status.phase=4;status.error=Some(error);updates.send_replace(status.clone());}
                }
            }
            result=snapshots.changed()=>{
                if result.is_err() {break;}
                let next=snapshots.borrow_and_update().clone();
                if worker.is_some() && next.id==status.id {status=next;updates.send_replace(status.clone());}
            }
            result=async {worker.as_mut().unwrap().finished().await},if worker.is_some()=>{
                worker=None;request=None;frames.send_replace(Status::default());
                if let Err(error)=result {status.phase=4;status.error=Some(error);}
                else {status.phase=0;status.error=None;}
                status.frame=None;updates.send_replace(status.clone());
            }
            result=archives.changed(),if archives_open=>{
                if result.is_err() {archives_open=false;continue;}
                loading=None;request=None;
                if let Some(mut old)=worker.take() {old.stop().await;} frames.send_replace(Status::default());
                db=archives.borrow_and_update().clone();changed=db.as_ref().map(Archive::subscribe);
                status=Status::default();updates.send_replace(status.clone());
            }
            result=sources.changed(),if sources_open=>{
                if result.is_err() {sources_open=false;}
                // A cached player no longer owns the network client and may
                // continue offline. Cancel only a pending authenticated fetch.
                if loading.take().is_some() {status.phase=4;status.error=Some(Error::Offline);updates.send_replace(status.clone());}
            }
            _=async {match &mut changed {Some(c)=>{let _=c.changed().await;},None=>std::future::pending().await}}=>{
                if let (Some(db),Some(current))=(&db,&request)
                    && db.video_request(current.id).await.ok().flatten().as_ref()!=Some(current) {
                    request=None;
                    if let Some(mut old)=worker.take() {old.stop().await;} frames.send_replace(Status::default());
                    status=Status::default();updates.send_replace(status.clone());
                }
            }
        }
    }
    if let Some(mut worker) = worker {
        worker.stop().await;
    }
}

#[cfg(test)]
#[path = "video_tests.rs"]
mod tests;
