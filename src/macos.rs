//! Narrow FFI boundary to Apple frameworks. No Objective-C objects cross threads.
// SAFETY: all raw pointers below refer to owned, bounded buffers for the duration
// of each call; AppKit calls are dispatched exclusively to Slint's main thread.
#![allow(unsafe_code)]

use crate::downloads::{Control, Error};
use std::{
    collections::{HashMap, HashSet},
    ffi::{CStr, CString, c_char, c_void},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::oneshot;

type Reply = Result<Vec<PathBuf>, Error>;
type Requests = Mutex<HashMap<u64, oneshot::Sender<Reply>>>;
static REQUESTS: OnceLock<Requests> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static EMOJI_SUPPORT: OnceLock<HashSet<&'static str>> = OnceLock::new();

unsafe extern "C" {
    fn heya_startup_system_info(output: *mut c_char, capacity: usize);
    fn heya_emoji_support(catalogue: *const c_char, supported: *mut u8, count: usize);
    fn heya_panel_begin(
        id: u64,
        kind: u32,
        name: *const c_char,
        folder: *const c_char,
        reply: extern "C" fn(u64, *const *const c_char, usize),
    );
    fn heya_panel_cancel(id: u64);
    fn heya_pdf_render(
        bytes: *const u8,
        length: usize,
        width: u32,
        height: u32,
        limit: usize,
        output: *mut *mut u8,
        size: *mut usize,
    ) -> i32;
    fn heya_native_free(bytes: *mut c_void);
}

/// Fixed OS/CPU/model fields only; no hostname, username, serial or device UUID.
pub fn startup_system_info() -> String {
    let mut output = [0u8; 512];
    // The bridge writes at most capacity bytes, including a terminating NUL.
    unsafe { heya_startup_system_info(output.as_mut_ptr().cast(), output.len()) };
    let end = output
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(output.len());
    String::from_utf8_lossy(&output[..end]).into_owned()
}

pub fn prepare_emoji_support() {
    EMOJI_SUPPORT.get_or_init(|| {
        let mut values = Vec::new();
        for emoji in emojis::iter() {
            if let Some(tones) = emoji.skin_tones() {
                values.extend(tones.map(|e| e.as_str()));
            } else {
                values.push(emoji.as_str());
            }
        }
        let catalogue = CString::new(values.join("\n")).expect("Emoji contain no NUL bytes");
        let mut supported = vec![0; values.len()];
        // Both buffers are owned here and live throughout the synchronous call.
        unsafe { heya_emoji_support(catalogue.as_ptr(), supported.as_mut_ptr(), values.len()) };
        values
            .into_iter()
            .zip(supported)
            .filter_map(|(emoji, available)| (available != 0).then_some(emoji))
            .collect()
    });
}

pub fn supports_emoji(emoji: &str) -> bool {
    prepare_emoji_support();
    EMOJI_SUPPORT.get().unwrap().contains(emoji)
}

#[cfg(test)]
mod emoji_tests {
    #[test]
    fn native_picker_coverage_retains_joined_flags_keycaps_and_skin_tones() {
        for emoji in ["😀", "👩🏽‍💻", "🫱🏻‍🫲🏼", "🇷🇴", "1️⃣", "🙂‍↔️", "👨‍👩‍👧‍👦"]
        {
            assert!(
                super::supports_emoji(emoji),
                "Missing native emoji: {emoji}"
            );
        }
    }
}

extern "C" fn panel_reply(id: u64, paths: *const *const c_char, count: usize) {
    let result = if count > 100 || (count > 0 && paths.is_null()) {
        Err(Error::Picker)
    } else {
        let mut selected = Vec::with_capacity(count);
        for i in 0..count {
            // The bridge retains all URL strings through this callback.
            let path = unsafe { *paths.add(i) };
            if path.is_null() {
                send_reply(id, Err(Error::Picker));
                return;
            }
            let bytes = unsafe { CStr::from_ptr(path) }.to_bytes().to_vec();
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
            if !path.is_absolute() {
                send_reply(id, Err(Error::Picker));
                return;
            }
            selected.push(path);
        }
        Ok(selected)
    };
    send_reply(id, result);
}

fn send_reply(id: u64, result: Reply) {
    if let Ok(mut requests) = REQUESTS.get_or_init(Default::default).lock()
        && let Some(reply) = requests.remove(&id)
    {
        let _ = reply.send(result);
    }
}

struct Panel(u64);
impl Drop for Panel {
    fn drop(&mut self) {
        if let Ok(mut requests) = REQUESTS.get_or_init(Default::default).lock() {
            requests.remove(&self.0);
        }
        let id = self.0;
        // Closing an already completed request is harmless. This also closes
        // the native sheet when its future is aborted during app shutdown.
        let _ = slint::invoke_from_event_loop(move || unsafe { heya_panel_cancel(id) });
    }
}

pub async fn choose(kind: u32, name: &str, folder: Option<&Path>, control: &Control) -> Reply {
    if control.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let name = CString::new(name).map_err(|_| Error::Picker)?;
    let folder = CString::new(folder.map_or(b"".as_slice(), |p| p.as_os_str().as_bytes()))
        .map_err(|_| Error::Destination)?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    REQUESTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| Error::Picker)?
        .insert(id, tx);
    let _panel = Panel(id);
    let cancelled = control.clone();
    slint::invoke_from_event_loop(move || {
        let pending = REQUESTS
            .get_or_init(Default::default)
            .lock()
            .is_ok_and(|requests| requests.contains_key(&id));
        if !pending {
            return;
        }
        if cancelled.is_cancelled() {
            send_reply(id, Err(Error::Cancelled));
            return;
        }
        // CString storage lives through this call; AppKit copies each value.
        unsafe {
            heya_panel_begin(id, kind, name.as_ptr(), folder.as_ptr(), panel_reply);
        }
    })
    .map_err(|_| Error::Picker)?;
    tokio::select! {
        biased;
        _ = control.cancelled() => Err(Error::Cancelled),
        result = rx => result.map_err(|_| Error::Picker)?,
    }
}

/// Called before UI/account initialization, in a bounded, kill-on-drop child.
pub fn pdf_worker() -> ! {
    use std::io::{Read, Write};
    let mut bytes = Vec::new();
    let read = std::io::stdin()
        .take((crate::pdf_preview::MAX_PDF_BYTES + 1) as u64)
        .read_to_end(&mut bytes);
    if read.is_err()
        || bytes.len() > crate::pdf_preview::MAX_PDF_BYTES
        || !bytes.starts_with(b"%PDF-")
    {
        std::process::exit(1);
    }
    let mut png = std::ptr::null_mut();
    let mut size = 0;
    let ok = unsafe {
        heya_pdf_render(
            bytes.as_ptr(),
            bytes.len(),
            crate::document_previews::WIDTH,
            crate::document_previews::HEIGHT,
            crate::document_previews::MAX_BYTES,
            &mut png,
            &mut size,
        )
    };
    if ok == 0 || png.is_null() {
        std::process::exit(1);
    }
    let pixels = unsafe { std::slice::from_raw_parts(png, size) };
    let written = if cfg!(test) {
        std::io::stderr().write_all(pixels)
    } else {
        std::io::stdout().write_all(pixels)
    };
    unsafe {
        heya_native_free(png.cast());
    }
    std::process::exit(if written.is_ok() { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> (u64, oneshot::Receiver<Reply>) {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        REQUESTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(id, tx);
        (id, rx)
    }

    #[tokio::test]
    async fn native_paths_preserve_unicode_and_reject_invalid_results() {
        let path = CString::new("/tmp/Synthetic résumé 你好.pdf").unwrap();
        let (id, rx) = request();
        panel_reply(id, [path.as_ptr()].as_ptr(), 1);
        assert_eq!(
            rx.await.unwrap().unwrap(),
            vec![PathBuf::from("/tmp/Synthetic résumé 你好.pdf")]
        );
        assert!(!REQUESTS.get().unwrap().lock().unwrap().contains_key(&id));

        let relative = CString::new("relative.pdf").unwrap();
        let (id, rx) = request();
        panel_reply(id, [relative.as_ptr()].as_ptr(), 1);
        assert_eq!(rx.await.unwrap(), Err(Error::Picker));

        let (id, rx) = request();
        panel_reply(id, std::ptr::null(), usize::MAX);
        assert_eq!(rx.await.unwrap(), Err(Error::Picker));

        let (id, rx) = request();
        panel_reply(id, std::ptr::null(), 0);
        assert!(rx.await.unwrap().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelled_request_never_opens_a_panel() {
        let control = Control::default();
        control.cancel();
        assert_eq!(choose(1, "", None, &control).await, Err(Error::Cancelled));
    }
}
