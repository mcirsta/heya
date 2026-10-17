//! Authenticated document previews: use a sharp remote image when available,
//! otherwise render the actual PDF's first-page strip outside the UI process.
use crate::document_previews::{MAX_BYTES, Source};
use std::sync::Arc;
use whatsapp_rust::{
    Client,
    download::{DownloadParams, MediaType},
    prelude::MessageExt,
    waproto::{codec, whatsapp::message::DocumentMessage},
};

fn reference(doc: &DocumentMessage) -> Option<DownloadParams> {
    let path = doc
        .thumbnail_direct_path
        .as_deref()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))?;
    DownloadParams::encrypted(
        path,
        doc.media_key.as_deref()?,
        doc.thumbnail_sha256.as_deref()?,
        doc.thumbnail_enc_sha256.as_deref()?,
        0,
        MediaType::DocumentThumbnail,
    )
    .ok()
}
fn thumbnail_available(doc: &DocumentMessage) -> bool {
    // History ingestion only checks borrowed metadata; allocate download
    // parameters later, when this particular card is actually requested.
    doc.thumbnail_direct_path
        .as_deref()
        .is_some_and(|p| p.starts_with('/') && !p.starts_with("//"))
        && [
            &doc.media_key,
            &doc.thumbnail_sha256,
            &doc.thumbnail_enc_sha256,
        ]
        .iter()
        .all(|v| v.as_ref().is_some_and(|v| v.len() == 32))
}

fn pdf_available(doc: &DocumentMessage) -> bool {
    let is_pdf = doc
        .mimetype
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("application/pdf"))
        || (doc.mimetype.as_deref().is_none_or(str::is_empty)
            && doc.file_name.as_deref().is_some_and(|n| {
                n.get(n.len().saturating_sub(4)..)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".pdf"))
            }));
    is_pdf
        && !doc
            .file_length
            .is_some_and(|n| n > crate::pdf_preview::MAX_PDF_BYTES as u64)
        && doc
            .direct_path
            .as_deref()
            .is_some_and(|p| p.starts_with('/') && !p.starts_with("//"))
        && [&doc.media_key, &doc.file_sha256, &doc.file_enc_sha256]
            .iter()
            .all(|v| v.as_ref().is_some_and(|v| v.len() == 32))
}

fn pdf_reference(doc: &DocumentMessage) -> Option<DownloadParams> {
    if !pdf_available(doc) {
        return None;
    }
    let path = doc
        .direct_path
        .as_deref()
        .filter(|p| p.starts_with('/') && !p.starts_with("//"))?;
    DownloadParams::encrypted(
        path,
        doc.media_key.as_deref()?,
        doc.file_sha256.as_deref()?,
        doc.file_enc_sha256.as_deref()?,
        doc.file_length.unwrap_or(0),
        MediaType::Document,
    )
    .ok()
}

pub fn available(doc: &DocumentMessage) -> bool {
    thumbnail_available(doc) || pdf_available(doc)
}
pub struct Previews(pub Arc<Client>);
#[async_trait::async_trait]
impl Source for Previews {
    async fn fetch(&self, raw: &[u8]) -> Option<Vec<u8>> {
        let message = codec::message_decode(raw).ok()?;
        if message.is_view_once() {
            return None;
        }
        let doc = message.get_base_message().document_message.as_option()?;
        let thumbnail = match reference(doc) {
            Some(r) => crate::protocol_media::download(&self.0, &r, MAX_BYTES)
                .await
                .ok(),
            None => None,
        };
        if thumbnail
            .as_deref()
            .is_some_and(crate::document_previews::sharp_enough)
        {
            return thumbnail;
        }
        if let Some(r) = pdf_reference(doc)
            && let Ok(bytes) =
                crate::protocol_media::download(&self.0, &r, crate::pdf_preview::MAX_PDF_BYTES)
                    .await
            && let Ok(image) = crate::pdf_preview::render(bytes).await
        {
            return Some(image);
        }
        thumbnail
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatsapp_rust::{
        download::{DownloadUtils, Downloadable},
        wacore::upload::encrypt_media_with_key,
    };
    #[test]
    fn pdf_without_thumbnail_reserves_preview_and_uses_document_crypto_and_limits() {
        let mut doc = DocumentMessage::default();
        doc.mimetype = Some("application/pdf".into());
        doc.file_name = Some("Synthetic.PDF".into());
        doc.direct_path = Some("/synthetic-document".into());
        doc.media_key = Some(vec![7; 32]);
        doc.file_sha256 = Some(vec![8; 32]);
        doc.file_enc_sha256 = Some(vec![9; 32]);
        doc.file_length = Some(33455);
        assert!(available(&doc));
        assert!(!thumbnail_available(&doc));
        let r = pdf_reference(&doc).unwrap();
        assert_eq!(r.app_info(), MediaType::Document);
        assert_eq!(r.file_length(), Some(33455));
        assert_eq!(r.file_sha256(), doc.file_sha256.as_deref());
        let mut message = whatsapp_rust::waproto::whatsapp::Message::default();
        message.document_message = whatsapp_rust::waproto::buffa::MessageField::some(doc.clone());
        let (_, content, embedded) = crate::protocol_content::extract(&message);
        assert!(content.document.unwrap().has_thumbnail);
        assert!(embedded.is_empty());
        doc.file_length = Some(crate::pdf_preview::MAX_PDF_BYTES as u64 + 1);
        assert!(!available(&doc));
        doc.file_length = None;
        assert!(
            available(&doc),
            "unknown length is enforced by the streaming writer"
        );
        doc.mimetype = Some("text/plain".into());
        assert!(!available(&doc), "do not parse other files as PDF");
        doc.mimetype = None;
        assert!(
            available(&doc),
            "allow absent MIME with a PDF filename; renderer still checks magic"
        );
        doc.media_key = Some(vec![1; 31]);
        assert!(!available(&doc));
    }
    #[test]
    fn thumbnail_reference_uses_its_own_hashes_path_and_crypto_context() {
        let mut doc = DocumentMessage::default();
        doc.thumbnail_direct_path = Some("/synthetic-preview?token=fake".into());
        doc.direct_path = Some("/never-download-pdf".into());
        doc.media_key = Some(vec![7; 32]);
        doc.thumbnail_sha256 = Some(vec![8; 32]);
        doc.thumbnail_enc_sha256 = Some(vec![9; 32]);
        doc.file_sha256 = Some(vec![10; 32]);
        doc.file_enc_sha256 = Some(vec![11; 32]);
        doc.file_length = Some(u64::MAX);
        let mut message = whatsapp_rust::waproto::whatsapp::Message::default();
        message.document_message = whatsapp_rust::waproto::buffa::MessageField::some(doc.clone());
        let (_, content, embedded) = crate::protocol_content::extract(&message);
        assert!(content.document.unwrap().has_thumbnail);
        assert!(
            embedded.is_empty(),
            "remote-only previews reserve geometry before downloading"
        );
        let r = reference(&doc).unwrap();
        assert_eq!(r.direct_path(), doc.thumbnail_direct_path.as_deref());
        assert_eq!(r.file_sha256(), doc.thumbnail_sha256.as_deref());
        assert_eq!(r.file_enc_sha256(), doc.thumbnail_enc_sha256.as_deref());
        assert_eq!(r.file_length(), Some(0));
        assert_eq!(r.app_info().app_info(), "WhatsApp Document Thumbnail Keys");
        assert_eq!(r.app_info().mms_type(), "thumbnail-document");
        let encrypted =
            encrypt_media_with_key(b"synthetic image bytes", r.app_info(), Some(&[7; 32])).unwrap();
        assert_eq!(
            DownloadUtils::verify_and_decrypt(&encrypted.data_to_upload, &[7; 32], r.app_info())
                .unwrap(),
            b"synthetic image bytes"
        );
        assert!(
            DownloadUtils::verify_and_decrypt(
                &encrypted.data_to_upload,
                &[7; 32],
                MediaType::Document
            )
            .is_err()
        );
        let mut invalid = doc.clone();
        invalid.thumbnail_sha256 = Some(vec![1; 31]);
        assert!(!available(&invalid));
        invalid = doc.clone();
        invalid.thumbnail_direct_path = Some("https://example.invalid/image".into());
        assert!(!available(&invalid));
        invalid = doc;
        invalid.thumbnail_enc_sha256 = None;
        assert!(!available(&invalid));
    }
}
