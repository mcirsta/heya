//! Seekable appsrc shared by audio and video. Compressed bytes never become files.
use gstreamer as gst;
use gstreamer_app::{AppSrc, AppSrcCallbacks, AppStreamType};
use std::sync::{Arc, Mutex};

pub fn source(bytes: Arc<[u8]>) -> AppSrc {
    let source = AppSrc::builder()
        .format(gst::Format::Bytes)
        .stream_type(AppStreamType::RandomAccess)
        .size(bytes.len() as i64)
        .max_bytes(256 * 1024)
        .build();
    let cursor = Arc::new(Mutex::new(0usize));
    let seek = cursor.clone();
    let len = bytes.len();
    source.set_callbacks(
        AppSrcCallbacks::builder()
            .need_data(move |src, length| {
                let buffer = {
                    let mut at = cursor.lock().unwrap();
                    if *at >= bytes.len() {
                        None
                    } else {
                        let length = if length == u32::MAX {
                            64 * 1024
                        } else {
                            length as usize
                        };
                        let end = at.saturating_add(length).min(bytes.len());
                        let buffer = gst::Buffer::from_mut_slice(bytes[*at..end].to_vec());
                        *at = end;
                        Some(buffer)
                    }
                };
                if let Some(buffer) = buffer {
                    let _ = src.push_buffer(buffer);
                } else {
                    let _ = src.end_of_stream();
                }
            })
            .seek_data(move |_, offset| {
                if offset > len as u64 {
                    return false;
                }
                *seek.lock().unwrap() = offset as usize;
                true
            })
            .build(),
    );
    source
}
