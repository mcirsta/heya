//! Page-one thumbnails for outgoing PDFs, in the sizes WhatsApp Web sends: a
//! small JPEG embedded in the message and a larger one uploaded separately as
//! the document thumbnail. Rendering stays in the sandboxed PDF helper. Any
//! failure only omits the images; the document itself is still sent.
#[cfg(test)]
#[path = "document_thumbnails_tests.rs"]
mod tests;
use crate::{downloads::Control, pdf_preview};

/// WhatsApp Web keeps embedded document thumbnails at or under this size.
pub const EMBEDDED_BYTES: usize = 1300;
/// WhatsApp Web's default JPEG quality for both images.
const QUALITY: u8 = 92;
/// Halvings tried before giving up on an embedded image, as WhatsApp Web does.
const ATTEMPTS: usize = 10;

pub struct Thumbnails {
    /// The message's `jpegThumbnail`; empty if no halving fits.
    pub embedded: Vec<u8>,
    /// The separate thumbnail, longest edge at most `pdf_preview::PAGE_EDGE`.
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pages: Option<u32>,
}

/// Blocking: called on the attachment preparation thread, which has no Tokio
/// runtime. Cancellation drops the render, killing its helper.
pub fn prepare(pdf: Vec<u8>, control: &Control) -> Option<Thumbnails> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    let page = runtime.block_on(async {
        tokio::select! {
            biased;
            _ = control.cancelled() => None,
            page = pdf_preview::first_page(pdf) => page.ok(),
        }
    })?;
    drop(runtime);
    encode(&page.png, page.pages)
}

fn encode(png: &[u8], pages: Option<u32>) -> Option<Thumbnails> {
    // The same bounded whole-image decode as video posters: at most
    // PAGE_EDGE on the longest edge, never scaled up.
    let pixels = crate::document_previews::video_decode(png)?;
    let page = image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.rgba)?;
    let page = image::DynamicImage::ImageRgba8(page).to_rgb8();
    let jpeg = encode_jpeg(&page)?;
    let mut small = page.clone();
    let mut embedded = jpeg.clone();
    for _ in 0..ATTEMPTS {
        if embedded.len() <= EMBEDDED_BYTES || small.width() < 2 || small.height() < 2 {
            break;
        }
        small = image::imageops::thumbnail(&small, small.width() / 2, small.height() / 2);
        embedded = encode_jpeg(&small)?;
    }
    if embedded.len() > EMBEDDED_BYTES {
        embedded.clear();
    }
    Some(Thumbnails {
        embedded,
        jpeg,
        width: page.width(),
        height: page.height(),
        pages,
    })
}

fn encode_jpeg(image: &image::RgbImage) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY)
        .encode_image(image)
        .ok()?;
    Some(out)
}
