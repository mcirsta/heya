use super::*;
use std::time::{Duration, Instant};

/// Pixels with a strong colour or dark tone; a blank white page has none.
fn ink(jpeg: &[u8]) -> usize {
    image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .unwrap()
        .to_rgb8()
        .pixels()
        .filter(|p| p.0.iter().any(|c| *c < 128))
        .count()
}

#[test]
fn actual_pdfs_yield_both_thumbnails_at_whatsapp_web_sizes() {
    for (name, pdf, size) in [
        (
            "portrait",
            include_bytes!("../tests/fixtures/preview-portrait.pdf").as_slice(),
            (360, 480),
        ),
        (
            "landscape",
            include_bytes!("../tests/fixtures/preview-landscape.pdf").as_slice(),
            (480, 360),
        ),
        (
            // A 1024x768 page rotated by 90 degrees displays as portrait.
            "rotated",
            include_bytes!("../tests/fixtures/preview-rotated.pdf").as_slice(),
            (360, 480),
        ),
    ] {
        let started = Instant::now();
        let thumbnails = prepare(pdf.to_vec(), &Control::default())
            .expect("the platform PDF renderer must be available");
        assert_eq!((thumbnails.width, thumbnails.height), size, "{name}");
        assert_eq!(thumbnails.pages, Some(2), "{name}");
        let large = image::load_from_memory_with_format(&thumbnails.jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        assert_eq!((large.width(), large.height()), size, "{name}");
        assert!(
            ink(&thumbnails.jpeg) > 1000,
            "{name}: page must be rendered"
        );
        assert!(!thumbnails.embedded.is_empty(), "{name}");
        assert!(thumbnails.embedded.len() <= EMBEDDED_BYTES, "{name}");
        let small =
            image::load_from_memory_with_format(&thumbnails.embedded, image::ImageFormat::Jpeg)
                .unwrap();
        // Halving keeps the page's shape.
        let ratio = |w: u32, h: u32| w as f32 / h as f32;
        assert!(
            (ratio(small.width(), small.height()) - ratio(size.0, size.1)).abs() < 0.1,
            "{name}"
        );
        eprintln!(
            "Synthetic PDF {name}: {}x{} {} bytes, embedded {}x{} {} bytes, {} ms",
            thumbnails.width,
            thumbnails.height,
            thumbnails.jpeg.len(),
            small.width(),
            small.height(),
            thumbnails.embedded.len(),
            started.elapsed().as_millis()
        );
    }
}

#[test]
fn invalid_and_cancelled_pdfs_yield_no_thumbnails() {
    assert!(prepare(b"not a PDF".to_vec(), &Control::default()).is_none());
    assert!(
        prepare(
            b"%PDF-1.7 corrupt synthetic file".to_vec(),
            &Control::default()
        )
        .is_none()
    );
    let control = Control::default();
    control.cancel();
    let started = Instant::now();
    let pdf = include_bytes!("../tests/fixtures/preview-portrait.pdf");
    assert!(prepare(pdf.to_vec(), &control).is_none());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn noisy_pages_still_fit_the_embedded_limit() {
    // Random pixels are the worst case for JPEG size.
    let mut seed = 0x2545_f491_u32;
    let noise = image::RgbImage::from_fn(480, 340, |_, _| {
        let mut channel = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        image::Rgb([channel(), channel(), channel()])
    });
    let mut png = Vec::new();
    noise
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let thumbnails = encode(&png, None).unwrap();
    assert_eq!((thumbnails.width, thumbnails.height), (480, 340));
    assert!(!thumbnails.embedded.is_empty());
    assert!(thumbnails.embedded.len() <= EMBEDDED_BYTES);
    assert_eq!(thumbnails.pages, None);

    // A helper's oversized output is reduced, never sent as is.
    let wide = image::RgbImage::from_pixel(1200, 300, image::Rgb([255, 255, 255]));
    let mut png = Vec::new();
    wide.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let thumbnails = encode(&png, Some(3)).unwrap();
    assert_eq!((thumbnails.width, thumbnails.height), (480, 120));
    assert_eq!(thumbnails.pages, Some(3));
    assert!(encode(b"not an image", None).is_none());
}
