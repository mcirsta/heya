use super::*;
use std::{fs::File, io::Write};
pub(crate) fn text(value: &str) -> Vec<u8> {
    codec::message_to_vec(&wa::Message::text(value))
}
pub(crate) fn prepared(file: &Staged, caption: String, destination: &Destination) -> Prepared {
    message(
        file,
        caption,
        destination,
        Uploaded {
            url: "https://example.invalid/synthetic".into(),
            direct_path: "/synthetic".into(),
            timestamp: 1_700_000_000,
        },
    )
    .unwrap()
}
#[tokio::test]
async fn staged_photo_preserves_original_bytes_and_builds_quoted_image_or_document() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Synthetic 你好.png");
    let pixels = image::RgbaImage::from_pixel(100, 50, image::Rgba([35, 140, 90, 180]));
    pixels.save(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    let mut files = stage(vec![path.clone()], Kind::Photo, Control::default())
        .await
        .unwrap();
    let file = files.remove(0);
    assert_eq!((file.width, file.height), (100, 50));
    assert_eq!(file.mime, "image/png");
    assert!(file.preview.as_ref().unwrap().rgba.len() <= 512 * 512 * 4);
    assert_eq!(
        image::guess_format(&file.thumbnail).unwrap(),
        image::ImageFormat::Jpeg
    );
    let mut output = Vec::new();
    whatsapp_rust::wacore::download::DownloadUtils::decrypt_stream_to_writer_with_hashes(
        test_reader(file.clone()),
        &file.key,
        MediaType::Image,
        Some(&file.encrypted_hash),
        Some(&file.hash),
        &mut output,
    )
    .unwrap();
    assert_eq!(
        output, original,
        "photo sending must not silently recompress the original"
    );
    let destination = Destination {
        chat: 1,
        title: "Synthetic recipient".into(),
        owner: "15550000000@s.whatsapp.net".into(),
        reply: Some(crate::message_actions::Source {
            id: 2,
            chat: 1,
            jid: "15550000001@s.whatsapp.net".into(),
            sender: "15550000001@s.whatsapp.net".into(),
            wire: "synthetic-quote".into(),
            outgoing: false,
            body: "Quoted".into(),
            raw: text("Quoted"),
        }),
    };
    let image_prepared = prepared(&file, "Caption 👩🏽‍💻\nSecond line".into(), &destination);
    let proto = codec::message_decode(&image_prepared.raw).unwrap();
    assert_eq!(
        proto.image_message.caption.as_deref(),
        Some("Caption 👩🏽‍💻\nSecond line")
    );
    assert_eq!(
        proto.context_info().unwrap().stanza_id.as_deref(),
        Some("synthetic-quote")
    );
    assert!(image_prepared.content.photo.is_some());
    assert!(image_prepared.content.quote.is_some());
    std::fs::write(&path, b"changed after preparation").unwrap();
    assert_eq!(file.size, original.len() as u64);
    let docs = stage(vec![path], Kind::Document, Control::default())
        .await
        .unwrap();
    let document = prepared(
        &docs[0],
        "Full original as document".into(),
        &Destination {
            reply: None,
            ..destination
        },
    );
    let proto = codec::message_decode(&document.raw).unwrap();
    assert!(proto.document_message.is_set());
    assert!(!proto.image_message.is_set());
    assert_eq!(
        proto.document_message.file_name.as_deref(),
        Some("Synthetic 你好.png")
    );
}
#[tokio::test]
async fn camera_orientation_is_preserved_in_photo_dimensions_and_original_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Synthetic camera.jpg");
    let image = image::RgbImage::from_pixel(80, 40, image::Rgb([150, 90, 40]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode_image(&image)
        .unwrap();
    let exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut rotated = jpeg[..2].to_vec();
    rotated.extend_from_slice(&[0xff, 0xe1]);
    rotated.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    rotated.extend_from_slice(exif);
    rotated.extend_from_slice(&jpeg[2..]);
    std::fs::write(&path, &rotated).unwrap();
    let files = stage(vec![path], Kind::Photo, Control::default())
        .await
        .unwrap();
    let file = &files[0];
    assert_eq!((file.width, file.height), (40, 80));
    assert_eq!(
        (
            file.preview.as_ref().unwrap().width,
            file.preview.as_ref().unwrap().height
        ),
        (40, 80)
    );
    let mut plain = Vec::new();
    whatsapp_rust::wacore::download::DownloadUtils::decrypt_stream_to_writer_with_hashes(
        test_reader(file.clone()),
        &file.key,
        MediaType::Image,
        Some(&file.encrypted_hash),
        Some(&file.hash),
        &mut plain,
    )
    .unwrap();
    assert_eq!(plain, rotated);
}
#[tokio::test]
async fn staging_rejects_invalid_missing_special_oversized_and_cancelled_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("invalid.png");
    std::fs::write(&path, b"not an image").unwrap();
    assert!(matches!(
        stage(vec![path.clone()], Kind::Photo, Control::default()).await,
        Err(Error::Image)
    ));
    assert!(matches!(
        stage(vec![temp.path().into()], Kind::Document, Control::default()).await,
        Err(Error::File)
    ));
    assert!(matches!(
        stage(
            vec![temp.path().join("missing")],
            Kind::Document,
            Control::default()
        )
        .await,
        Err(Error::File)
    ));
    assert!(matches!(
        stage(vec![path.clone(); 11], Kind::Document, Control::default()).await,
        Err(Error::Limit)
    ));
    let control = Control::default();
    control.cancel();
    assert!(matches!(
        stage(vec![path.clone()], Kind::Document, control).await,
        Err(Error::Cancelled)
    ));
    let huge = File::create(&path).unwrap();
    huge.set_len(attachments::MAX_BYTES + 1).unwrap();
    drop(huge);
    assert!(matches!(
        stage(vec![path.clone()], Kind::Document, Control::default()).await,
        Err(Error::Limit)
    ));
    #[cfg(unix)]
    {
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(matches!(
            stage(vec![link], Kind::Document, Control::default()).await,
            Err(Error::File)
        ));
    }
}
#[tokio::test]
async fn large_document_encrypts_and_upload_readers_resume_without_buffering_plaintext() {
    use sha2::{Digest, Sha256};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("synthetic-large.bin");
    let mut source = File::create(&path).unwrap();
    let chunk = vec![42; 32 * 1024];
    for _ in 0..1024 {
        source.write_all(&chunk).unwrap();
    }
    drop(source);
    let files = stage(vec![path], Kind::Document, Control::default())
        .await
        .unwrap();
    let file = files[0].clone();
    assert_eq!(file.size, 32 * 1024 * 1024);
    let control = Control::default();
    let permit = UPLOAD.clone().acquire_owned().await.unwrap();
    let source = UploadBody(Arc::new(Body {
        file: file.clone(),
        control: control.clone(),
        progress: watch::channel(0).0,
        _permit: Arc::new(permit),
    }));
    let mut start = source.reader_from(0).unwrap();
    let mut small = [0; 32];
    start.read_exact(&mut small).unwrap();
    let mut resumed = source.reader_from(16).unwrap();
    let mut next = [0; 16];
    resumed.read_exact(&mut next).unwrap();
    assert_eq!(&small[16..], &next);
    assert!(source.reader_from(source.len() + 1).is_err());
    struct Hash(Sha256);
    impl Write for Hash {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.update(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut hash = Hash(Sha256::new());
    whatsapp_rust::wacore::download::DownloadUtils::decrypt_stream_to_writer_with_hashes(
        source.reader_from(0).unwrap(),
        &file.key,
        MediaType::Document,
        Some(&file.encrypted_hash),
        Some(&file.hash),
        &mut hash,
    )
    .unwrap();
    let result: [u8; 32] = hash.0.finalize().into();
    assert_eq!(result, file.hash);
    control.cancel();
    assert!(resumed.read(&mut next).is_err());
    let encrypted = Arc::downgrade(&file.blob);
    drop(file);
    drop(files);
    drop(source);
    assert!(
        encrypted.upgrade().is_some(),
        "blocking readers retain their encrypted file"
    );
    drop(start);
    drop(resumed);
    assert!(encrypted.upgrade().is_none());
}

#[tokio::test]
async fn imported_files_share_encrypted_staging_and_preserve_photo_kind() {
    let dir = tempfile::tempdir().unwrap();
    let photo = dir.path().join("Clipboard 你好.PNG");
    image::RgbaImage::from_pixel(900, 700, image::Rgba([30, 150, 200, 160]))
        .save(&photo)
        .unwrap();
    let document = dir.path().join("Notes.txt");
    std::fs::write(&document, b"Synthetic document").unwrap();
    let png = std::fs::read(&photo).unwrap();
    let paths = attachments::Input::Paths(vec![photo, document], None);
    let files = stage_input(paths, Control::default()).await.unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].kind, Kind::Photo);
    assert_eq!(files[1].kind, Kind::Document);
    let pasted = stage_input(attachments::Input::Png(png.clone()), Control::default())
        .await
        .unwrap();
    assert_eq!(pasted[0].size, png.len() as u64);
    let mut original = Vec::new();
    whatsapp_rust::wacore::download::DownloadUtils::decrypt_stream_to_writer_with_hashes(
        test_reader(pasted[0].clone()),
        &pasted[0].key,
        MediaType::Image,
        Some(&pasted[0].encrypted_hash),
        Some(&pasted[0].hash),
        &mut original,
    )
    .unwrap();
    assert_eq!(original, png);
    let cancelled = Control::default();
    cancelled.cancel();
    assert!(matches!(
        stage_input(attachments::Input::Png(png), cancelled).await,
        Err(Error::Cancelled)
    ));
    assert!(
        stage_input(
            attachments::Input::Paths(vec![dir.path().into()], None),
            Control::default()
        )
        .await
        .is_err()
    );
}
