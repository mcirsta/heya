//! Explicit selection, bounded preparation, review, upload and durable queueing.
#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
use crate::{
    archive::Archive,
    downloads::Control,
    message_actions::{Prepared, Source as Reply},
    photos::Pixels,
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{mpsc, watch};

pub const MAX_FILES: usize = 10;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Photo,
    Document,
    Voice,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Clipboard,
    PhotoUnavailable,
    Picker,
    File,
    Limit,
    Image,
    Changed,
    Offline,
    Upload,
    Archive,
    Cancelled,
    Caption,
}
impl Error {
    pub fn label(self) -> &'static str {
        match self {
            Self::PhotoUnavailable => {
                "This photo is no longer available to copy. Open it again and retry."
            }
            Self::Clipboard => {
                "The clipboard could not be read or updated. Copy the item again and retry."
            }
            Self::Caption => {
                "Captions can contain up to 1,024 characters. Your text has been kept."
            }
            Self::Picker => "The native file chooser is unavailable.",
            Self::File => "The selected file could not be read. Choose a regular, readable file.",
            Self::Limit => {
                "Choose up to 10 files, at most 2 GiB in total. Photos must be under 20 MiB and 32 megapixels."
            }
            Self::Image => {
                "This photo could not be decoded. Choose a JPEG/PNG image or send it as a document."
            }
            Self::Changed => {
                "The file changed while it was being prepared. Please select it again."
            }
            Self::Offline => "Connect to WhatsApp to send these attachments.",
            Self::Upload => {
                "Upload failed. Retry sends only the attachments that have not been queued."
            }
            Self::Archive => {
                "The attachment could not be queued in the encrypted archive. Your selection is retained."
            }
            Self::Cancelled => "Upload cancelled. Your remaining selection is retained.",
        }
    }
}
// No Debug: payloads, keys, names, captions and destinations are private.
pub struct Staged {
    pub id: String,
    pub blob: Arc<std::fs::File>,
    pub key: [u8; 32],
    pub hash: [u8; 32],
    pub encrypted_hash: [u8; 32],
    pub sidecar: Option<Vec<u8>>,
    pub encrypted_len: u64,
    pub size: u64,
    pub kind: Kind,
    pub name: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub thumbnail: Vec<u8>,
    pub preview: Option<Arc<Pixels>>,
    pub voice: Option<crate::voice_recorder::Clip>,
}
pub struct Destination {
    pub chat: i32,
    pub title: String,
    pub reply: Option<Reply>,
    pub owner: String,
}
#[derive(Clone)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub preview: Option<Arc<Pixels>>,
}
#[derive(Clone, Default)]
pub struct Snapshot {
    pub voice: bool,
    pub phase: i32, // 0 hidden, 1 choosing, 2 preparing, 3 review, 4 uploading, 5 queueing, 6 error
    pub title: String,
    pub items: Arc<[Item]>,
    pub selected: usize,
    pub caption: String,
    pub error: String,
    pub bytes: u64,
    pub total: u64,
}
pub enum Input {
    Paths(Vec<PathBuf>, Option<Kind>),
    Png(Vec<u8>),
}
pub enum Command {
    Import {
        destination: Destination,
        input: Input,
    },
    Voice {
        destination: Destination,
        file: Arc<Staged>,
        accepted: tokio::sync::oneshot::Sender<Result<(), Error>>,
    },
    Pick {
        chat: i32,
        title: String,
        parent: String,
        kind: Kind,
    },
    Select(usize, String),
    Remove(String),
    Send(String),
    Cancel,
}
#[async_trait::async_trait]
pub trait Picker: Send + Sync {
    async fn open(
        &self,
        kind: Kind,
        parent: &str,
        control: &Control,
    ) -> Result<Vec<PathBuf>, Error>;
}
pub struct NativePicker;
#[async_trait::async_trait]
impl Picker for NativePicker {
    async fn open(
        &self,
        kind: Kind,
        parent: &str,
        control: &Control,
    ) -> Result<Vec<PathBuf>, Error> {
        crate::downloads_picker::open(kind == Kind::Photo, parent, control)
            .await
            .map_err(|_| {
                if control.is_cancelled() {
                    Error::Cancelled
                } else {
                    Error::Picker
                }
            })
    }
}
#[async_trait::async_trait]
pub trait Uploader: Send + Sync {
    async fn upload(
        &self,
        file: Arc<Staged>,
        caption: String,
        destination: &Destination,
        control: Control,
        progress: watch::Sender<u64>,
    ) -> Result<Prepared, Error>;
}
pub type Sources = Option<Arc<dyn Uploader>>;
struct CancelOnDrop(Control);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Selection {
    destination: Destination,
    files: Vec<Arc<Staged>>,
    captions: Vec<String>,
    selected: usize,
}
impl Selection {
    fn show(&self, updates: &watch::Sender<Snapshot>, phase: i32, error: String) {
        let items = self
            .files
            .iter()
            .map(|f| Item {
                id: f.id.clone(),
                name: f.name.clone(),
                size: f.size,
                preview: f.preview.clone(),
            })
            .collect::<Vec<_>>();
        updates.send_replace(Snapshot {
            voice: self.files.first().is_some_and(|f| f.kind == Kind::Voice),
            phase,
            title: self.destination.title.clone(),
            items: items.into(),
            selected: self.selected,
            caption: self
                .captions
                .get(self.selected)
                .cloned()
                .unwrap_or_default(),
            error,
            ..Default::default()
        });
    }
    fn caption(&mut self, value: String) {
        if let Some(v) = self.captions.get_mut(self.selected) {
            *v = value;
        }
    }
}

pub async fn run(
    archives: watch::Receiver<Option<Archive>>,
    mut sources: watch::Receiver<Sources>,
    mut commands: mpsc::Receiver<Command>,
    updates: watch::Sender<Snapshot>,
    picker: Arc<dyn Picker>,
    demo: bool,
) {
    type Work =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Selection, Error>> + Send>>;
    let mut selection: Option<Selection> = None;
    let mut work: Option<Work> = None;
    let mut uploading = false;
    let mut continuation = false;
    let mut control = Control::default();
    let (progress, mut progress_rx) = watch::channel(0u64);
    loop {
        tokio::select! {
            result=async {match &mut work {Some(w)=>w.await,None=>std::future::pending().await}} => {
                work=None;
                if uploading {
                    uploading=false;
                    // Each successful queue removes exactly one file before any retry.
                    match result {
                        Ok(mut s)=>{s.files.remove(0);s.captions.remove(0);s.selected=0;
                            if s.files.is_empty(){selection=None;updates.send_replace(Snapshot::default());}
                            else {s.show(&updates,4,String::new());selection=Some(s);continuation=true;}
                        }
                        Err(error)=>{if let Some(s)=&selection {s.show(&updates,6,error.label().into());}}
                    }
                } else { match result {
                    Ok(s)=>{s.show(&updates,3,String::new());selection=Some(s);}
                    Err(Error::Cancelled)=>{updates.send_replace(Snapshot::default());}
                    Err(e)=>{updates.send_replace(Snapshot{phase:6,error:e.label().into(),..Default::default()});}
                }}
            }
            changed=progress_rx.changed(), if uploading => {
                if changed.is_ok(){let n=*progress_rx.borrow_and_update();updates.send_modify(|s|{if s.phase==4{s.bytes=n;}});}
            }
            command=async {
                if continuation {
                    match commands.try_recv() {
                        Ok(c)=>Some(c),
                        Err(mpsc::error::TryRecvError::Disconnected)=>None,
                        Err(mpsc::error::TryRecvError::Empty)=>Some(Command::Send(updates.borrow().caption.clone())),
                    }
                } else {commands.recv().await}
            }=>{
                continuation=false;
                let Some(command)=command else {
                    control.cancel();
                    if let Some(work)=work {let _=tokio::time::timeout(std::time::Duration::from_secs(5),work).await;}
                    return;
                };
                match command {
                    Command::Voice{destination,file,accepted}=>{
                        if accepted.is_closed(){continue;}
                        if work.is_some() || selection.is_some() {let _=accepted.send(Err(Error::Upload));continue;}
                        let s=Selection{destination,files:vec![file],captions:vec![String::new()],selected:0};
                        s.show(&updates,4,String::new());selection=Some(s);continuation=true;
                        let _=accepted.send(Ok(()));
                    }
                    Command::Cancel => {
                        if work.is_some() {
                            if updates.borrow().phase!=5 {control.cancel();}
                        } else {selection=None;updates.send_replace(Snapshot::default());}
                    }
                    Command::Import{destination,input} if work.is_none() && selection.is_none()=>{
                        control=Control::default();
                        updates.send_replace(Snapshot{phase:2,title:destination.title.clone(),..Default::default()});
                        let c=control.clone();
                        work=Some(Box::pin(async move {
                            let _guard=CancelOnDrop(c.clone());
                            let files=crate::protocol_uploads::stage_input(input,c).await?;
                            let captions=vec![String::new();files.len()];
                            Ok(Selection{destination,files,captions,selected:0})
                        }));
                    }
                    Command::Import{..}=>{
                        updates.send_modify(|s|s.error="Finish the current attachment selection, then paste or drop again.".into());
                    }
                    Command::Pick{chat,title,parent,kind} if work.is_none()=>{
                        selection=None;uploading=false;control=Control::default();
                        updates.send_replace(Snapshot{phase:1,title:title.clone(),..Default::default()});
                        let archive=archives.borrow().clone();let picker=picker.clone();let c=control.clone();let u=updates.clone();
                        work=Some(Box::pin(async move {
                            let _guard=CancelOnDrop(c.clone());
                            let mut destination=if demo {Destination{chat,title:title.clone(),reply:None,owner:String::new()}} else {archive.ok_or(Error::Archive)?.attachment_destination(chat).await.map_err(|_|Error::Archive)?};
                            destination.title=title;
                            let paths=picker.open(kind,&parent,&c).await?;
                            if paths.is_empty() {return Err(Error::Cancelled);}
                            if paths.len()>MAX_FILES {return Err(Error::Limit);}
                            u.send_modify(|s|s.phase=2);
                            let files=crate::protocol_uploads::stage(paths,kind,c).await?;
                            let captions=vec![String::new();files.len()];
                            Ok(Selection{destination,files,captions,selected:0})
                        }));
                    }
                    Command::Select(index,caption) if work.is_none()=>{if let Some(s)=&mut selection {s.caption(caption);if index<s.files.len(){s.selected=index;}s.show(&updates,3,String::new());}}
                    Command::Remove(caption) if work.is_none()=>{if let Some(s)=&mut selection {s.caption(caption);s.files.remove(s.selected);s.captions.remove(s.selected);s.selected=s.selected.min(s.files.len().saturating_sub(1));if s.files.is_empty(){selection=None;updates.send_replace(Snapshot::default());}else{s.show(&updates,3,String::new());}}}
                    Command::Send(caption) if work.is_none()=>{
                        let Some(s)=&mut selection else {continue};s.caption(caption);
                        if s.captions.iter().any(|c|c.chars().count()>1024){s.show(&updates,6,Error::Caption.label().into());continue;}
                        let Some(source)=sources.borrow_and_update().clone() else {s.show(&updates,6,Error::Offline.label().into());continue};
                        let Some(archive)=archives.borrow().clone() else {continue};
                        s.selected=0;s.show(&updates,4,String::new());
                        let file=s.files[0].clone();let cached_audio=file.voice.as_ref().map(|v|v.bytes.clone());let caption=s.captions[0].clone();let id=file.id.clone();
                        updates.send_modify(|s|{s.bytes=0;s.total=file.encrypted_len;});
                        let destination=archive.attachment_destination(s.destination.chat).await;
                        let Ok(destination)=destination else {s.show(&updates,6,Error::Archive.label().into());continue};
                        // Preserve the quote captured before selection, not a later reply draft.
                        let mut destination=destination;destination.reply=s.destination.reply.clone();destination.title=s.destination.title.clone();
                        control=Control::default();let c=control.clone();let u=updates.clone();let p=progress.clone();let mut online=sources.clone();
                        let staged=s.files.clone();let captions=s.captions.clone();
                        uploading=true;
                        work=Some(Box::pin(async move {
                            let _guard=CancelOnDrop(c.clone());
                            let prepared=tokio::select! {
                                biased;
                                _=c.cancelled()=>return Err(Error::Cancelled),
                                _=online.changed()=>return Err(Error::Offline),
                                r=tokio::time::timeout(std::time::Duration::from_secs(600),source.upload(file,caption,&destination,c.clone(),p))=>r.map_err(|_|Error::Upload)??,
                            };
                            if c.is_cancelled(){return Err(Error::Cancelled);}
                            u.send_modify(|s|s.phase=5);
                            archive.queue_media(destination.chat,id,prepared,destination.reply.as_ref().map(|r|r.id),cached_audio).await.map_err(|_|Error::Archive)?;
                            Ok(Selection{destination,files:staged,captions,selected:0})
                        }));
                    }
                    _=>{}
                }
            }
        }
    }
}
