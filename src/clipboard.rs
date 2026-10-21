//! Native clipboard payloads. Called only from a finite preparation worker.
use crate::{
    attachments::{Error, Input},
    photos::Pixels,
};
use std::{borrow::Cow, path::PathBuf};

pub enum Content {
    Text(String),
    Attachments(Input),
}
// File managers often also offer an icon/thumbnail and a text URI. File data wins.
pub fn read() -> Result<Content, Error> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| Error::Clipboard)?;
    if let Ok(paths) = clipboard.get().file_list()
        && !paths.is_empty()
    {
        return Ok(Content::Attachments(paths_input(paths)?));
    }
    match clipboard.get_image() {
        Ok(image) => return Ok(Content::Attachments(Input::Png(encode(image)?))),
        Err(arboard::Error::ContentNotAvailable) => {}
        Err(_) => return Err(Error::Clipboard),
    }
    let text = clipboard.get_text().map_err(|_| Error::Clipboard)?;
    Ok(Content::Text(text))
}
pub fn paths_input(paths: Vec<PathBuf>) -> Result<Input, Error> {
    // Native transfers are read only up to limit + 1; reject an oversized
    // offer before deduplication so no trailing selections can disappear.
    if paths.len() > crate::attachments::MAX_FILES {
        return Err(Error::Limit);
    }
    // A repeated URI in one drop is not a second attachment.
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    if unique.is_empty() {
        return Err(Error::File);
    }
    Ok(Input::Paths(unique, None))
}
fn encode(image: arboard::ImageData<'_>) -> Result<Vec<u8>, Error> {
    use image::ImageEncoder;
    let count = image.width.checked_mul(image.height).ok_or(Error::Limit)?;
    if count == 0 || count > 32_000_000 || image.width > 16384 || image.height > 16384 {
        return Err(Error::Limit);
    }
    if image.bytes.len() != count * 4 {
        return Err(Error::Image);
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        &image.bytes,
        image.width as u32,
        image.height as u32,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|_| Error::Image)?;
    if png.len() > crate::photos::MAX_BYTES {
        return Err(Error::Limit);
    }
    Ok(png)
}
pub fn copy(pixels: Pixels) -> Result<arboard::Clipboard, Error> {
    let mut clipboard = arboard::Clipboard::new().map_err(|_| Error::Clipboard)?;
    clipboard
        .set_image(arboard::ImageData {
            width: pixels.width as usize,
            height: pixels.height as usize,
            bytes: Cow::Owned(pixels.rgba),
        })
        .map_err(|_| Error::Clipboard)?;
    // X11 serves the offer while this handle lives, even after the worker exits.
    Ok(clipboard)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rgba_roundtrip_preserves_resolution_and_alpha() {
        let bytes = [17, 45, 200, 89].repeat(1200 * 700);
        let png = encode(arboard::ImageData {
            width: 1200,
            height: 700,
            bytes: Cow::Borrowed(&bytes),
        })
        .unwrap();
        let decoded = crate::photos::decode(&png, u32::MAX).unwrap();
        assert_eq!((decoded.width, decoded.height), (1200, 700));
        assert_eq!(decoded.rgba, bytes);
        for (width, height, data) in [
            (0, 1, vec![]),
            (40000, 1, vec![]),
            (1, 1, vec![0; 3]),
            (8000, 8000, vec![]),
        ] {
            assert!(
                encode(arboard::ImageData {
                    width,
                    height,
                    bytes: Cow::Owned(data)
                })
                .is_err()
            );
        }
    }
    #[test]
    fn a_drop_is_bounded_and_deduplicated_without_losing_unicode() {
        let path = PathBuf::from("/tmp/Some file 你好.png");
        let Input::Paths(paths, None) = paths_input(vec![path.clone(), path.clone()]).unwrap()
        else {
            panic!()
        };
        assert_eq!(paths, vec![path]);
        assert!(paths_input((0..11).map(|i| PathBuf::from(i.to_string())).collect()).is_err());
        assert!(paths_input(vec![PathBuf::from("same.png"); 11]).is_err());
        assert!(paths_input(vec![]).is_err());
    }
    #[test]
    #[ignore = "requires a private display via scripts/ui-offscreen.sh --clipboard-check or --wayland-clipboard-check"]
    fn native_clipboard_roundtrip() {
        assert_eq!(
            std::env::var("HEYA_PRIVATE_CLIPBOARD_TEST").as_deref(),
            Ok("1")
        );
        let mut owner = arboard::Clipboard::new().unwrap();
        let text = "/tmp/this is plain text.png\nHello 👩🏽‍💻";
        owner.set_text(text).unwrap();
        assert!(matches!(read().unwrap(),Content::Text(value) if value==text));
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("clipboard 你好.png");
        std::fs::write(&path, b"synthetic").unwrap();
        owner.set().file_list(&[&path]).unwrap();
        assert!(
            matches!(read().unwrap(),Content::Attachments(Input::Paths(value,None)) if value==vec![path])
        );
        use std::io::Write;
        let wayland = std::env::var_os("HEYA_TEST_WAYLAND_CLIPBOARD").is_some();
        if wayland {
            assert!(std::env::var_os("DISPLAY").is_none(), "no X11 fallback");
        }
        let (writer, args): (&str, &[&str]) = if wayland {
            ("wl-copy", &["--type", "text/uri-list"])
        } else {
            (
                "xclip",
                &["-selection", "clipboard", "-in", "-target", "text/uri-list"],
            )
        };
        let mut external = std::process::Command::new(writer)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        external.stdin.take().unwrap().write_all(b"# files\r\nfile:///tmp/space%20name.png\r\nfile://localhost/tmp/second.txt\r\nfile://remote/tmp/ignored\r\n").unwrap();
        assert!(external.wait().unwrap().success());
        assert!(
            matches!(read().unwrap(),Content::Attachments(Input::Paths(value,None)) if value==vec![PathBuf::from("/tmp/space name.png"),PathBuf::from("/tmp/second.txt")])
        );
        let rgba = [24, 80, 160, 93].repeat(1234 * 650);
        let expected = rgba.clone();
        drop(owner);
        // The coordinator retains the returned offer, with no idle preparation
        // worker and no dependence on Slint keeping a second handle alive.
        let mut owner = std::thread::spawn(move || {
            copy(Pixels {
                width: 1234,
                height: 650,
                rgba,
            })
            .unwrap()
        })
        .join()
        .unwrap();
        let image = owner.get_image().unwrap();
        assert_eq!((image.width, image.height), (1234, 650));
        assert_eq!(image.bytes.as_ref(), expected);
        assert!(matches!(
            read().unwrap(),
            Content::Attachments(Input::Png(_))
        ));
        // Another application can read the offered image/png after our worker exits.
        let (reader, args): (&str, &[&str]) = if wayland {
            ("wl-paste", &["--no-newline", "--type", "image/png"])
        } else {
            (
                "xclip",
                &["-selection", "clipboard", "-out", "-target", "image/png"],
            )
        };
        let output = std::process::Command::new(reader)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            crate::photos::decode(&output.stdout, u32::MAX)
                .unwrap()
                .rgba,
            expected
        );
    }
}
