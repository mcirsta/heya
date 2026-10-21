//! Attachment metadata, authenticated CDN transfers and SDK errors stay here.
#[cfg(test)]
#[path = "protocol_downloads_tests.rs"]
pub(crate) mod tests;
use crate::downloads::{Error, Info, Kind, Source};
use crate::downloads_file::Sink;
use std::{
    io::{self, Seek, SeekFrom, Write},
    sync::Arc,
};
use whatsapp_rust::{
    Client,
    download::{
        ClientDownloadError, DownloadParams, DownloadWriter, Downloadable, MediaDecryptionError,
    },
    prelude::MessageExt,
    waproto::codec,
};

fn payload(raw: &[u8]) -> Result<(DownloadParams, Info), Error> {
    let message = codec::message_decode(raw).map_err(|_| Error::Unavailable)?;
    if message.is_view_once() {
        return Err(Error::Unavailable);
    }
    let base = message.get_base_message();
    let (media, kind, name, mime): (&dyn Downloadable, Kind, Option<&str>, &str) =
        if let Some(v) = base.document_message.as_option() {
            (
                v,
                Kind::Document,
                v.file_name.as_deref().or(v.title.as_deref()),
                v.mimetype.as_deref().unwrap_or(""),
            )
        } else if let Some(v) = base.image_message.as_option() {
            (
                v,
                Kind::Photo,
                None,
                v.mimetype.as_deref().unwrap_or("image/jpeg"),
            )
        } else if let Some(v) = base.audio_message.as_option() {
            if v.view_once == Some(true) {
                return Err(Error::Unavailable);
            }
            (
                v,
                Kind::Audio,
                None,
                v.mimetype.as_deref().unwrap_or("audio/ogg"),
            )
        } else if let Some(v) = base.video_message.as_option() {
            if v.view_once == Some(true) {
                return Err(Error::Unavailable);
            }
            (
                v,
                Kind::Video,
                None,
                v.mimetype.as_deref().unwrap_or("video/mp4"),
            )
        } else if let Some(v) = base.sticker_message.as_option() {
            (
                v,
                Kind::Sticker,
                None,
                v.mimetype.as_deref().unwrap_or("image/webp"),
            )
        } else {
            return Err(Error::Unavailable);
        };
    if media.static_url().is_some() {
        return Err(Error::Unavailable);
    }
    let path = media
        .direct_path()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))
        .ok_or(Error::Unavailable)?;
    let sha256: [u8; 32] = media
        .file_sha256()
        .and_then(|v| v.try_into().ok())
        .ok_or(Error::Unavailable)?;
    let size = media.file_length().filter(|n| *n > 0);
    let reference = DownloadParams::encrypted(
        path,
        media.media_key().ok_or(Error::Unavailable)?,
        &sha256,
        media.file_enc_sha256().ok_or(Error::Unavailable)?,
        size.unwrap_or(0),
        media.app_info(),
    )
    .map_err(|_| Error::Unavailable)?;
    let name = filename(name, mime, kind);
    Ok((
        reference,
        Info {
            name,
            size,
            sha256,
            kind,
        },
    ))
}
pub fn info(raw: &[u8]) -> Result<Info, Error> {
    payload(raw).map(|(_, info)| info)
}

fn filename(name: Option<&str>, mime: &str, kind: Kind) -> String {
    let fallback = match kind {
        Kind::Photo => "Photo",
        Kind::Audio => "Audio",
        Kind::Document => "Document",
        Kind::Video => "Video",
        Kind::Sticker => "Sticker",
    };
    let mut name=name.unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("").chars()
        .filter(|c|!c.is_control()&&!matches!(c,'<'|'>'|':'|'"'|'|'|'?'|'*'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
        .collect::<String>().trim_matches([' ','.']).to_owned();
    if name.is_empty() {
        name = format!(
            "{fallback}-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        name.insert(0, '_');
    }
    let extension = match mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "audio/wav" | "audio/x-wav" => "wav",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        _ => "",
    };
    if !name.contains('.') && !extension.is_empty() {
        name.push('.');
        name.push_str(extension);
    }
    if name.len() > 220 {
        let extension = name
            .rsplit_once('.')
            .filter(|(_, e)| e.len() <= 16)
            .map(|(_, e)| format!(".{e}"))
            .unwrap_or_default();
        let mut end = 220 - extension.len();
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
        name.push_str(&extension);
    }
    name
}
pub struct Downloads(pub Arc<Client>);
// A manual large save cannot hold up automatic previews or audio downloads.
// Retain this permit in the blocking SDK writer across cancellation/reconnect.
static DOWNLOAD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
struct Writer {
    sink: Sink,
    _permit: tokio::sync::SemaphorePermit<'static>,
}
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.sink.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}
impl Seek for Writer {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.sink.seek(p)
    }
}
impl DownloadWriter for Writer {
    fn truncate(&mut self, len: u64) -> io::Result<()> {
        self.sink.truncate(len)
    }
}
fn error(error: ClientDownloadError) -> Error {
    match error {
        ClientDownloadError::ReferenceRejected(_) | ClientDownloadError::Preparation(_) => {
            Error::Unavailable
        }
        ClientDownloadError::WriterIo(e) => {
            if e.chain()
                .filter_map(|e| e.downcast_ref::<io::Error>())
                .any(|e| e.kind() == io::ErrorKind::FileTooLarge)
            {
                Error::TooLarge
            } else {
                Error::Storage
            }
        }
        ClientDownloadError::HostsUnreachable(e)
            if e.chain().any(|e| e.is::<MediaDecryptionError>()) =>
        {
            Error::Integrity
        }
        ClientDownloadError::WriterCleanup { .. } => Error::Storage,
        _ => Error::Network,
    }
}
#[async_trait::async_trait]
impl Source for Downloads {
    async fn fetch(&self, raw: &[u8], sink: Sink) -> Result<Sink, Error> {
        let (media, info) = payload(raw)?;
        if info.size.is_some_and(|n| n > crate::downloads::MAX_BYTES) {
            return Err(Error::TooLarge);
        }
        let permit = DOWNLOAD.acquire().await.map_err(|_| Error::Network)?;
        self.0
            .download_to_writer(
                &media,
                Writer {
                    sink,
                    _permit: permit,
                },
            )
            .await
            .map(|w| w.sink)
            .map_err(error)
    }
}
