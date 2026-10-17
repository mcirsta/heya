use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use whatsapp_rust::{
    download::{MediaDownloader, MediaHost, MediaRoute, MediaType},
    wacore::{
        net::{HttpClient, HttpRequest, HttpResponse, StreamingHttpResponse},
        upload::encrypt_media_with_key,
    },
    waproto::{
        buffa::MessageField,
        whatsapp::{Message, message::*},
    },
};

fn message(kind: Kind, bytes: &[u8]) -> Message {
    let hash = Sha256::digest(bytes).to_vec();
    let mut message = Message::default();
    macro_rules! media {
        ($type:ident,$field:ident) => {{
            let mut v = $type::default();
            v.media_key = Some(vec![7; 32]);
            v.file_sha256 = Some(hash);
            v.file_enc_sha256 = Some(vec![8; 32]);
            v.direct_path = Some("/synthetic-test-media".into());
            v.file_length = Some(bytes.len() as u64);
            message.$field = MessageField::some(v);
        }};
    }
    match kind {
        Kind::Photo => media!(ImageMessage, image_message),
        Kind::Audio => media!(AudioMessage, audio_message),
        Kind::Document => media!(DocumentMessage, document_message),
        Kind::Video => media!(VideoMessage, video_message),
        Kind::Sticker => media!(StickerMessage, sticker_message),
    }
    message
}
pub(crate) fn attachment(kind: Kind, bytes: &[u8]) -> (Vec<u8>, crate::message_content::Content) {
    let message = message(kind, bytes);
    let (_, content, _) = crate::protocol_content::extract(&message);
    (codec::message_to_vec(&message), content)
}
#[test]
fn attachment_metadata_covers_original_media_and_rejects_view_once_and_bad_references() {
    for kind in [
        Kind::Photo,
        Kind::Audio,
        Kind::Document,
        Kind::Video,
        Kind::Sticker,
    ] {
        let message = message(kind, b"synthetic original");
        let (params, info) = payload(&codec::message_to_vec(&message)).unwrap();
        assert!(info.kind == kind);
        assert_eq!(info.size, Some(18));
        assert_eq!(
            params.file_sha256().unwrap(),
            Sha256::digest(b"synthetic original").as_slice()
        );
    }
    let mut image = message(Kind::Photo, b"test");
    image.image_message.as_option_mut().unwrap().view_once = Some(true);
    assert!(matches!(
        info(&codec::message_to_vec(&image)),
        Err(Error::Unavailable)
    ));
    image.image_message.as_option_mut().unwrap().view_once = None;
    image.image_message.as_option_mut().unwrap().static_url =
        Some("https://not-a-cdn.invalid/file".into());
    assert!(info(&codec::message_to_vec(&image)).is_err());
    image.image_message.as_option_mut().unwrap().static_url = None;
    image.image_message.as_option_mut().unwrap().media_key = Some(vec![0; 31]);
    assert!(info(&codec::message_to_vec(&image)).is_err());
    let mut doc = message(Kind::Document, b"test");
    doc.document_message.as_option_mut().unwrap().direct_path =
        Some("//attacker.invalid/path".into());
    assert!(info(&codec::message_to_vec(&doc)).is_err());
}
#[test]
fn suggested_names_keep_unicode_but_never_supply_remote_directory_components() {
    assert_eq!(
        filename(
            Some("../../private/Știință.pdf"),
            "application/pdf",
            Kind::Document
        ),
        "Știință.pdf"
    );
    assert_eq!(
        filename(
            Some("C:\\folder\\note.pdf"),
            "application/pdf",
            Kind::Document
        ),
        "note.pdf"
    );
    assert_eq!(
        filename(Some("CON.txt"), "text/plain", Kind::Document),
        "_CON.txt"
    );
    assert_eq!(
        filename(Some("a\0b\u{202e}.pdf"), "application/pdf", Kind::Document),
        "ab.pdf"
    );
    let long = filename(
        Some(&format!("{}.pdf", "你好".repeat(200))),
        "application/pdf",
        Kind::Document,
    );
    assert!(long.len() <= 220 && long.ends_with(".pdf"));
    assert!(filename(None, "image/png", Kind::Photo).ends_with(".png"));
}
struct ShortReader {
    bytes: Arc<[u8]>,
    position: usize,
}
impl io::Read for ShortReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = out.len().min(997).min(self.bytes.len() - self.position);
        out[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}
struct Http {
    bad: Arc<[u8]>,
    good: Arc<[u8]>,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl HttpClient for Http {
    async fn execute(&self, _: HttpRequest) -> anyhow::Result<HttpResponse> {
        panic!("file download must stream")
    }
    fn supports_streaming(&self) -> bool {
        true
    }
    fn execute_streaming(&self, _: HttpRequest) -> anyhow::Result<StreamingHttpResponse> {
        let bytes = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.bad.clone()
        } else {
            self.good.clone()
        };
        Ok(StreamingHttpResponse::new(
            200,
            Box::new(ShortReader { bytes, position: 0 }),
        ))
    }
}
#[tokio::test]
async fn sdk_streaming_authenticates_before_publish_and_truncates_failed_host_output() {
    use crate::{
        downloads::{Control, Progress},
        downloads_file::Partial,
    };
    let original = vec![0x3d; 4 * 1024 * 1024];
    let key = [7; 32];
    let encrypted = encrypt_media_with_key(&original, MediaType::Document, Some(&key)).unwrap();
    let mut bad = encrypted.data_to_upload.clone();
    *bad.last_mut().unwrap() ^= 1;
    let params = DownloadParams::encrypted(
        "/synthetic-document",
        &key,
        &encrypted.file_sha256,
        &encrypted.file_enc_sha256,
        original.len() as u64,
        MediaType::Document,
    )
    .unwrap();
    let http = Arc::new(Http {
        bad: bad.into(),
        good: encrypted.data_to_upload.into(),
        calls: AtomicUsize::new(0),
    });
    let downloader = MediaDownloader::new(
        http.clone(),
        Arc::new(whatsapp_rust::TokioRuntime),
        MediaRoute::new(vec![
            MediaHost::new("first.invalid"),
            MediaHost::new("second.invalid"),
        ]),
    );
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("synthetic.bin");
    let (partial, sink) = Partial::create(
        destination.clone(),
        Control::default(),
        tokio::sync::watch::channel(Progress::default()).0,
    )
    .unwrap();
    let writer = Writer {
        sink,
        _permit: DOWNLOAD.acquire().await.unwrap(),
    };
    let writer = downloader
        .download_to_writer(&params, writer)
        .await
        .unwrap();
    assert_eq!(http.calls.load(Ordering::SeqCst), 2);
    assert!(!destination.exists());
    let Writer { sink, _permit } = writer;
    drop(_permit);
    partial.publish(sink).unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), original);

    let params = DownloadParams::encrypted(
        "/synthetic-document",
        &key,
        &[0; 32],
        params.file_enc_sha256().unwrap(),
        original.len() as u64,
        MediaType::Document,
    )
    .unwrap();
    let failed = dir.path().join("must-not-exist.bin");
    let (partial, sink) = Partial::create(
        failed.clone(),
        Control::default(),
        tokio::sync::watch::channel(Progress::default()).0,
    )
    .unwrap();
    let result = downloader
        .download_to_writer(
            &params,
            Writer {
                sink,
                _permit: DOWNLOAD.acquire().await.unwrap(),
            },
        )
        .await;
    assert!(result.is_err());
    drop(partial);
    assert!(!failed.exists());
    assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".heya-download-")
    }));
}
