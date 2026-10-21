//! SDK crypto/upload/message construction; file I/O and decoding never run in Slint.
#[cfg(test)]
#[path = "protocol_uploads_tests.rs"]
pub(crate) mod tests;
use crate::{
    attachments::{self, Destination, Error, Kind, Staged, Uploader},
    downloads::Control,
    message_actions::Prepared,
};
use std::{
    fs::Metadata,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Instant,
};
use tokio::sync::watch;
use whatsapp_rust::{
    Client,
    prelude::{MessageBuilderExt, MessageExt, wa},
    wacore::{
        download::MediaType,
        upload::{EncryptedMediaInfo, UploadSource},
    },
    waproto::{buffa::MessageField, codec},
};
static PREPARE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static UPLOAD: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(1)));
fn cancelled(c: &Control) -> Result<(), Error> {
    if c.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
pub async fn stage(
    paths: Vec<PathBuf>,
    kind: Kind,
    control: Control,
) -> Result<Vec<Arc<Staged>>, Error> {
    stage_input(attachments::Input::Paths(paths, Some(kind)), control).await
}
pub async fn stage_input(
    input: attachments::Input,
    control: Control,
) -> Result<Vec<Arc<Staged>>, Error> {
    let permit = tokio::select! {_ = control.cancelled()=>return Err(Error::Cancelled),p=PREPARE.acquire()=>p.map_err(|_|Error::File)?};
    crate::attachment_worker::blocking(control.clone(), move || {
        let _permit = permit;
        cancelled(&control)?;
        let (paths, kind) = match input {
            attachments::Input::Png(bytes) => {
                return Ok(vec![Arc::new(prepare_reader(
                    Box::new(io::Cursor::new(bytes)),
                    "Pasted image.png".into(),
                    Kind::Photo,
                    &control,
                )?)]);
            }
            attachments::Input::Paths(paths, kind) => (paths, kind),
        };
        if kind == Some(Kind::Voice) {
            return Err(Error::File);
        }
        if paths.is_empty() || paths.len() > attachments::MAX_FILES {
            return Err(Error::Limit);
        }
        let mut files = Vec::new();
        let mut total = 0u64;
        for path in &paths {
            let metadata = std::fs::symlink_metadata(path).map_err(|_| Error::File)?;
            if !metadata.is_file() {
                return Err(Error::File);
            }
            total = total.checked_add(metadata.len()).ok_or(Error::Limit)?;
            if total > attachments::MAX_BYTES {
                return Err(Error::Limit);
            }
        }
        let mut prepared_total = 0u64;
        for path in paths {
            cancelled(&control)?;
            let kind = kind.unwrap_or_else(|| {
                match path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("jpg" | "jpeg" | "png") => Kind::Photo,
                    _ => Kind::Document,
                }
            });
            let file = prepare(&path, kind, &control)?;
            prepared_total = prepared_total.checked_add(file.size).ok_or(Error::Limit)?;
            if prepared_total > attachments::MAX_BYTES {
                return Err(Error::Limit);
            }
            files.push(Arc::new(file));
        }
        Ok(files)
    })
    .await
}
fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    a.is_file()
        && b.is_file()
        && a.len() == b.len()
        && a.modified()
            .ok()
            .zip(b.modified().ok())
            .is_some_and(|(a, b)| a == b)
}
fn prepare(path: &Path, kind: Kind, control: &Control) -> Result<Staged, Error> {
    let before = std::fs::symlink_metadata(path).map_err(|_| Error::File)?;
    if !before.is_file() {
        return Err(Error::File);
    }
    if before.len() > attachments::MAX_BYTES
        || (kind == Kind::Photo && before.len() > crate::photos::MAX_BYTES as u64)
    {
        return Err(Error::Limit);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let input = options.open(path).map_err(|_| Error::File)?;
    let before = input.metadata().map_err(|_| Error::File)?;
    if !before.is_file() {
        return Err(Error::File);
    }
    if before.len() > attachments::MAX_BYTES
        || (kind == Kind::Photo && before.len() > crate::photos::MAX_BYTES as u64)
    {
        return Err(Error::Limit);
    }
    let name = path
        .file_name()
        .ok_or(Error::File)?
        .to_string_lossy()
        .chars()
        .filter(|c| !c.is_control() && !matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
        .collect::<String>();
    let file = prepare_reader(
        Box::new(input.try_clone().map_err(|_| Error::File)?),
        name,
        kind,
        control,
    )?;
    if file.size != before.len() || !unchanged(&before, &input.metadata().map_err(|_| Error::File)?)
    {
        return Err(Error::Changed);
    }
    Ok(file)
}
fn prepare_reader(
    mut input: Box<dyn Read>,
    name: String,
    kind: Kind,
    control: &Control,
) -> Result<Staged, Error> {
    let (mut width, mut height) = (0, 0);
    let mut thumbnail = Vec::new();
    let mut preview = None;
    let photo = if kind == Kind::Photo {
        let mut bytes = Vec::new();
        (&mut input)
            .take(crate::photos::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::File)?;
        if bytes.len() > crate::photos::MAX_BYTES {
            return Err(Error::Limit);
        }
        cancelled(control)?;
        let (dimensions, pixels) =
            crate::photos::decode_with_dimensions(&bytes, 512).ok_or(Error::Image)?;
        (width, height) = dimensions;
        let mut image =
            image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba.clone())
                .ok_or(Error::Image)?;
        for pixel in image.pixels_mut() {
            let alpha = pixel[3] as u16;
            for c in 0..3 {
                pixel[c] = ((pixel[c] as u16 * alpha + 255 * (255 - alpha)) / 255) as u8;
            }
            pixel[3] = 255;
        }
        let thumb = image::DynamicImage::ImageRgba8(image)
            .thumbnail(240, 240)
            .to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut thumbnail, 80)
            .encode_image(&thumb)
            .map_err(|_| Error::Image)?;
        preview = Some(Arc::new(pixels));
        Some(bytes)
    } else {
        None
    };
    let mime = if let Some(bytes) = &photo {
        match image::guess_format(bytes).map_err(|_| Error::Image)? {
            image::ImageFormat::Jpeg => "image/jpeg",
            image::ImageFormat::Png => "image/png",
            _ => return Err(Error::Image),
        }
    } else {
        mime(&name)
    }
    .to_owned();
    cancelled(control)?;
    let mut blob = tempfile::tempfile().map_err(|_| Error::File)?;
    let media = if kind == Kind::Photo {
        MediaType::Image
    } else {
        MediaType::Document
    };
    let reader: Box<dyn Read> = if let Some(bytes) = photo {
        Box::new(io::Cursor::new(bytes))
    } else {
        input
    };
    let info = whatsapp_rust::wacore::upload::encrypt_media_streaming(
        CheckRead {
            inner: reader,
            control: control.clone(),
            left: attachments::MAX_BYTES + 1,
        },
        &mut blob,
        media,
    )
    .map_err(|_| {
        if control.is_cancelled() {
            Error::Cancelled
        } else {
            Error::File
        }
    })?;
    cancelled(control)?;
    if info.file_length > attachments::MAX_BYTES {
        return Err(Error::Limit);
    }
    let encrypted_len = blob.metadata().map_err(|_| Error::File)?.len();
    Ok(Staged {
        id: crate::messaging::fresh_id().map_err(|_| Error::File)?,
        blob: Arc::new(blob),
        key: info.media_key,
        hash: info.file_sha256,
        encrypted_hash: info.file_enc_sha256,
        sidecar: info.streaming_sidecar,
        encrypted_len,
        size: info.file_length,
        kind,
        name,
        mime,
        width,
        height,
        thumbnail,
        preview,
        voice: None,
    })
}
fn mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "zip" => "application/zip",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
}
struct CheckRead<R> {
    inner: R,
    control: Control,
    left: u64,
}
impl<R: Read> Read for CheckRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.control.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        if self.left == 0 {
            return Err(io::Error::other("size limit"));
        }
        let len = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..len])?;
        self.left -= n as u64;
        Ok(n)
    }
}
struct Body {
    file: Arc<Staged>,
    control: Control,
    progress: watch::Sender<u64>,
    _permit: Arc<tokio::sync::OwnedSemaphorePermit>,
}
struct Reader {
    _body: Arc<Body>,
    at: u64,
    last: Instant,
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self._body.control.is_cancelled() {
            return Err(io::Error::other("cancelled"));
        }
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(&*self._body.file.blob, bytes, self.at)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(&*self._body.file.blob, bytes, self.at)?;
        self.at += n as u64;
        if self.last.elapsed() >= std::time::Duration::from_millis(100) {
            self.last = Instant::now();
            self._body.progress.send_replace(self.at);
        }
        Ok(n)
    }
}
struct UploadBody(Arc<Body>);
impl UploadSource for UploadBody {
    fn len(&self) -> u64 {
        self.0.file.encrypted_len
    }
    fn reader_from(&self, offset: u64) -> io::Result<Box<dyn Read + Send>> {
        if offset > self.len() {
            return Err(io::Error::other("invalid offset"));
        }
        Ok(Box::new(Reader {
            _body: self.0.clone(),
            at: offset,
            last: Instant::now(),
        }))
    }
}
pub struct Uploads(pub Arc<Client>);
#[async_trait::async_trait]
impl Uploader for Uploads {
    async fn upload(
        &self,
        file: Arc<Staged>,
        caption: String,
        destination: &Destination,
        control: Control,
        progress: watch::Sender<u64>,
    ) -> Result<Prepared, Error> {
        let permit = UPLOAD
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Error::Upload)?;
        cancelled(&control)?;
        let info = EncryptedMediaInfo {
            media_key: file.key,
            file_sha256: file.hash,
            file_enc_sha256: file.encrypted_hash,
            file_length: file.size,
            streaming_sidecar: file.sidecar.clone(),
        };
        let body = Arc::new(Body {
            file: file.clone(),
            control,
            progress,
            _permit: Arc::new(permit),
        });
        let client = self.0.clone();
        let cancel = body.control.clone();
        let media = match file.kind {
            Kind::Photo => MediaType::Image,
            Kind::Document => MediaType::Document,
            Kind::Voice => MediaType::Audio,
        };
        let uploaded = crate::attachment_worker::upload(cancel, async move {
            client
                .upload_stream(UploadBody(body), info, media)
                .await
                .map_err(|_| Error::Upload)
        })
        .await?;
        message(
            &file,
            caption,
            destination,
            Uploaded {
                url: uploaded.url,
                direct_path: uploaded.direct_path,
                timestamp: uploaded.media_key_timestamp,
            },
        )
    }
}
struct Uploaded {
    url: String,
    direct_path: String,
    timestamp: i64,
}
fn message(
    file: &Staged,
    caption: String,
    destination: &Destination,
    uploaded: Uploaded,
) -> Result<Prepared, Error> {
    let mut message = wa::Message::default();
    if let Some(voice) = &file.voice {
        let mut audio = wa::message::AudioMessage::default();
        audio.url = Some(uploaded.url);
        audio.direct_path = Some(uploaded.direct_path);
        audio.media_key = Some(file.key.to_vec());
        audio.file_sha256 = Some(file.hash.to_vec());
        audio.file_enc_sha256 = Some(file.encrypted_hash.to_vec());
        audio.file_length = Some(file.size);
        audio.media_key_timestamp = Some(uploaded.timestamp);
        audio.mimetype = Some(file.mime.clone());
        audio.streaming_sidecar = file.sidecar.clone();
        audio.ptt = Some(true);
        audio.seconds = Some(voice.seconds);
        audio.waveform = Some(voice.waveform.clone());
        message.audio_message = MessageField::some(audio);
    } else if file.kind == Kind::Photo {
        let mut image = wa::message::ImageMessage::default();
        image.url = Some(uploaded.url);
        image.direct_path = Some(uploaded.direct_path);
        image.media_key = Some(file.key.to_vec());
        image.file_sha256 = Some(file.hash.to_vec());
        image.file_enc_sha256 = Some(file.encrypted_hash.to_vec());
        image.file_length = Some(file.size);
        image.media_key_timestamp = Some(uploaded.timestamp);
        image.mimetype = Some(file.mime.clone());
        image.caption = Some(caption);
        image.width = Some(file.width);
        image.height = Some(file.height);
        image.jpeg_thumbnail = Some(file.thumbnail.clone());
        message.image_message = MessageField::some(image);
    } else {
        let mut doc = wa::message::DocumentMessage::default();
        doc.url = Some(uploaded.url);
        doc.direct_path = Some(uploaded.direct_path);
        doc.media_key = Some(file.key.to_vec());
        doc.file_sha256 = Some(file.hash.to_vec());
        doc.file_enc_sha256 = Some(file.encrypted_hash.to_vec());
        doc.file_length = Some(file.size);
        doc.media_key_timestamp = Some(uploaded.timestamp);
        doc.mimetype = Some(file.mime.clone());
        doc.file_name = Some(file.name.clone());
        doc.title = Some(file.name.clone());
        doc.caption = Some(caption);
        message.document_message = MessageField::some(doc);
    }
    if let Some(source) = &destination.reply {
        let original = if source.raw.is_empty() {
            wa::Message::text(source.body.clone())
        } else {
            codec::message_decode(&source.raw).map_err(|_| Error::Archive)?
        };
        if original.is_view_once() {
            return Err(Error::Archive);
        }
        let mut context = wa::ContextInfo::default();
        context.stanza_id = Some(source.wire.clone());
        context.participant = Some(if source.outgoing {
            destination.owner.clone()
        } else {
            source.sender.clone()
        });
        context.quoted_message =
            MessageField::from_box(original.get_base_message().prepare_for_quote());
        if !message.set_context_info(context) {
            return Err(Error::Archive);
        }
    }
    let (text, content, thumbnail) = crate::protocol_content::extract(&message);
    let text = text.unwrap_or_else(|| {
        if file.kind == Kind::Voice {
            "[Audio message]".into()
        } else {
            format!("[Photo] {}", content.body.as_deref().unwrap_or(""))
        }
    });
    Ok(Prepared {
        raw: codec::message_to_vec(&message),
        text,
        content,
        thumbnail,
        visible: true,
    })
}

#[cfg(test)]
pub(crate) fn test_reader(file: Arc<Staged>) -> impl Read {
    struct Cipher {
        file: Arc<Staged>,
        at: u64,
    }
    impl Read for Cipher {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            #[cfg(unix)]
            let n = std::os::unix::fs::FileExt::read_at(&*self.file.blob, bytes, self.at)?;
            #[cfg(windows)]
            let n = std::os::windows::fs::FileExt::seek_read(&*self.file.blob, bytes, self.at)?;
            self.at += n as u64;
            Ok(n)
        }
    }
    Cipher { file, at: 0 }
}

/// Recorded Ogg/Opus stays in bounded memory; only ciphertext is staged on disk.
pub async fn stage_voice(
    clip: crate::voice_recorder::Clip,
    control: Control,
) -> Result<Arc<Staged>, Error> {
    if clip.bytes.is_empty()
        || clip.bytes.as_ref().len() > crate::voice_recorder::MAX_BYTES
        || clip.seconds == 0
        || clip.seconds > crate::voice_recorder::MAX_SECONDS
        || clip.waveform.len() != 64
        || !clip.bytes.starts_with(b"OggS")
        || !clip.bytes.windows(8).take(512).any(|b| b == b"OpusHead")
    {
        return Err(Error::File);
    }
    let permit = tokio::select! {_=control.cancelled()=>return Err(Error::Cancelled),p=PREPARE.acquire()=>p.map_err(|_|Error::File)?};
    crate::attachment_worker::blocking(control.clone(), move || {
        let _permit = permit;
        cancelled(&control)?;
        let mut blob = tempfile::tempfile().map_err(|_| Error::File)?;
        let info = whatsapp_rust::wacore::upload::encrypt_media_streaming(
            CheckRead {
                inner: io::Cursor::new(clip.bytes.clone()),
                control: control.clone(),
                left: crate::voice_recorder::MAX_BYTES as u64 + 1,
            },
            &mut blob,
            MediaType::Audio,
        )
        .map_err(|_| Error::File)?;
        cancelled(&control)?;
        let encrypted_len = blob.metadata().map_err(|_| Error::File)?.len();
        Ok(Arc::new(Staged {
            id: crate::messaging::fresh_id().map_err(|_| Error::File)?,
            blob: Arc::new(blob),
            key: info.media_key,
            hash: info.file_sha256,
            encrypted_hash: info.file_enc_sha256,
            sidecar: info.streaming_sidecar,
            encrypted_len,
            size: info.file_length,
            kind: Kind::Voice,
            name: "Voice message.ogg".into(),
            mime: "audio/ogg; codecs=opus".into(),
            width: 0,
            height: 0,
            thumbnail: Vec::new(),
            preview: None,
            voice: Some(clip),
        }))
    })
    .await
}
