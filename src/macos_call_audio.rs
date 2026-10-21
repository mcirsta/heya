//! Native duplex PCM; AVAudioEngine and its device threads live only in the helper.
#![allow(unsafe_code)]
use crate::{
    audio_devices::Settings,
    call_audio::{Device, SAMPLES},
    voice_recorder::Error,
};
use std::{
    ffi::{c_char, c_void},
    ptr::NonNull,
    time::Duration,
};
unsafe extern "C" {
    fn heya_call_audio_new(
        input: *const c_char,
        output: *const c_char,
        input_gain: f32,
        output_gain: f32,
        error: *mut i32,
    ) -> *mut c_void;
    fn heya_call_audio_read(handle: *mut c_void, pcm: *mut i16, capacity: u32) -> i32;
    fn heya_call_audio_write(handle: *mut c_void, pcm: *const i16, count: u32) -> i32;
    fn heya_call_audio_free(handle: *mut c_void);
}
pub struct Audio(NonNull<c_void>);
impl Audio {
    pub fn new(settings: Settings) -> Result<Self, Error> {
        let input = crate::macos_audio::selected_uid(true, &settings.input.id)?;
        let output = crate::macos_audio::selected_uid(false, &settings.output.id)?;
        let mut error = 6;
        NonNull::new(unsafe {
            heya_call_audio_new(
                input.as_ptr(),
                output.as_ptr(),
                settings.input_volume as f32 / 100.,
                settings.output_volume as f32 / 100.,
                &mut error,
            )
        })
        .map(Self)
        .ok_or_else(|| crate::media_process::decode_error(error as u8))
    }
}
impl Drop for Audio {
    fn drop(&mut self) {
        unsafe { heya_call_audio_free(self.0.as_ptr()) }
    }
}
impl Device for Audio {
    fn capture(&mut self) -> Result<Option<Vec<i16>>, Error> {
        std::thread::sleep(Duration::from_millis(10));
        let mut pcm = vec![0; SAMPLES];
        match unsafe { heya_call_audio_read(self.0.as_ptr(), pcm.as_mut_ptr(), SAMPLES as u32) } {
            0 => Ok(None),
            n if n == SAMPLES as i32 => Ok(Some(pcm)),
            _ => Err(Error::Device),
        }
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), Error> {
        if unsafe { heya_call_audio_write(self.0.as_ptr(), pcm.as_ptr(), pcm.len() as u32) } == 1 {
            Ok(())
        } else {
            Err(Error::Playback)
        }
    }
}
