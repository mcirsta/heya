//! AVFoundation objects belong to the disposable decoder process, never Slint.
#![allow(unsafe_code)]
use crate::{
    video::{self, Error},
    video_process::{Control, Emitter, Header, Progress},
};
use std::{
    ffi::{c_char, c_void},
    io::Write,
    ptr::NonNull,
    time::{Duration, Instant},
};
use tokio::sync::watch;
unsafe extern "C" {
    fn heya_video_new(
        bytes: *const u8,
        len: usize,
        uid: *const c_char,
        volume: f32,
        silent: i32,
    ) -> *mut c_void;
    fn heya_video_control(handle: *mut c_void, playing: i32, muted: i32, seek: f64) -> i32;
    fn heya_video_frame(
        handle: *mut c_void,
        bytes: *mut u8,
        capacity: usize,
        width: *mut u32,
        height: *mut u32,
        position: *mut f64,
        duration: *mut f64,
        phase: *mut i32,
    ) -> i32;
    fn heya_video_free(handle: *mut c_void);
}
struct Player(NonNull<c_void>);
impl Drop for Player {
    fn drop(&mut self) {
        unsafe {
            heya_video_free(self.0.as_ptr());
        }
    }
}
pub fn play(
    bytes: Vec<u8>,
    header: Header,
    control: &mut watch::Receiver<Control>,
    output: &mut Emitter<impl Write>,
) -> Result<(), Error> {
    let uid = crate::macos_audio::selected_uid(false, &header.settings.output.id)
        .map_err(|_| Error::Playback)?;
    // `bytes` remains allocated until after Player::drop releases NSData's view.
    let player = Player(
        NonNull::new(unsafe {
            heya_video_new(
                bytes.as_ptr(),
                bytes.len(),
                uid.as_ptr(),
                f32::from(header.settings.output_volume) / 100.,
                i32::from(header.silent),
            )
        })
        .ok_or(Error::Codec)?,
    );
    let mut pixels = vec![0; (video::EDGE * video::EDGE * 4) as usize];
    let mut previous = Control::default();
    let mut progress = Progress {
        phase: 1,
        ..Default::default()
    };
    let started = Instant::now();
    let mut saw_frame = false;
    let mut reported = Instant::now() - Duration::from_secs(1);
    loop {
        let tick = Instant::now();
        let c = control.borrow_and_update().clone();
        if c.stop || control.has_changed().is_err() {
            return Ok(());
        }
        if c.playing != previous.playing || c.muted != previous.muted || c.seek != previous.seek {
            let seek = if c.seek != previous.seek {
                c.seek.map_or(-1., |(_, t)| t)
            } else {
                -1.
            };
            if unsafe {
                heya_video_control(
                    player.0.as_ptr(),
                    i32::from(c.playing),
                    i32::from(c.muted),
                    seek,
                )
            } == 0
            {
                return Err(Error::Playback);
            }
        }
        previous = c.clone();
        let (mut width, mut height) = (0, 0);
        let result = unsafe {
            heya_video_frame(
                player.0.as_ptr(),
                pixels.as_mut_ptr(),
                pixels.len(),
                &mut width,
                &mut height,
                &mut progress.position,
                &mut progress.duration,
                &mut progress.phase,
            )
        };
        if result < 0 {
            return Err(Error::Codec);
        }
        if result == 1 {
            output.frame(width, height, &pixels[..(width * height * 4) as usize])?;
            saw_frame = true;
        }
        if progress.phase == 0 && saw_frame {
            return Ok(());
        }
        if !saw_frame && started.elapsed() > Duration::from_secs(15) {
            return Err(Error::Codec);
        }
        if reported.elapsed() >= Duration::from_millis(100) {
            progress.muted = c.muted;
            output.progress(&progress)?;
            reported = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(40).saturating_sub(tick.elapsed()));
    }
}
