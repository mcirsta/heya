//! Native AVFoundation/VideoToolbox objects belong only to the finite helper.
#![allow(unsafe_code)]
use crate::{
    call_video::{Device, EDGE, Error, MAX_UNIT, Packet},
    photos::Pixels,
};
use std::{ffi::c_void, ptr::NonNull};
unsafe extern "C" {
    fn heya_call_video_new(synthetic: i32, error: *mut i32) -> *mut c_void;
    fn heya_call_video_poll(
        handle: *mut c_void,
        bytes: *mut u8,
        capacity: usize,
        tag: *mut u32,
        width: *mut u32,
        height: *mut u32,
        timestamp: *mut u32,
    ) -> i32;
    fn heya_call_video_receive(handle: *mut c_void, bytes: *const u8, length: usize) -> i32;
    fn heya_call_video_camera(handle: *mut c_void, enabled: i32) -> i32;
    fn heya_call_video_keyframe(handle: *mut c_void);
    fn heya_call_video_free(handle: *mut c_void);
}
pub struct Video {
    handle: NonNull<c_void>,
    buffer: Vec<u8>,
    orientation: u8,
}
impl Video {
    pub fn new(synthetic: bool) -> Result<Self, Error> {
        let mut error = 4;
        let handle = NonNull::new(unsafe { heya_call_video_new(i32::from(synthetic), &mut error) })
            .ok_or(match error {
                1 => Error::NoCamera,
                2 => Error::Permission,
                3 => Error::Codec,
                _ => Error::Device,
            })?;
        Ok(Self {
            handle,
            buffer: vec![0; MAX_UNIT.max((EDGE * EDGE * 4) as usize)],
            orientation: 0,
        })
    }
}
impl Drop for Video {
    fn drop(&mut self) {
        unsafe {
            heya_call_video_free(self.handle.as_ptr());
        }
    }
}
impl Device for Video {
    fn poll(&mut self) -> Result<Vec<Packet>, Error> {
        let mut packets = Vec::new();
        for _ in 0..6 {
            let (mut tag, mut width, mut height, mut timestamp) = (0, 0, 0, 0);
            let count = unsafe {
                heya_call_video_poll(
                    self.handle.as_ptr(),
                    self.buffer.as_mut_ptr(),
                    self.buffer.len(),
                    &mut tag,
                    &mut width,
                    &mut height,
                    &mut timestamp,
                )
            };
            if count < 0 || count as usize > self.buffer.len() {
                return Err(Error::Device);
            }
            if count == 0 {
                break;
            }
            let bytes = &self.buffer[..count as usize];
            packets.push(match tag {
                1 if bytes.len() <= MAX_UNIT => Packet::Encoded(bytes.to_vec(), timestamp),
                2 | 3
                    if width > 0
                        && height > 0
                        && width <= EDGE
                        && height <= EDGE
                        && bytes.len() == (width * height * 4) as usize =>
                {
                    let pixels = Pixels {
                        width,
                        height,
                        rgba: bytes.to_vec(),
                    };
                    if tag == 2 {
                        Packet::Local(pixels)
                    } else {
                        Packet::Remote(crate::call_video::rotate(pixels, self.orientation))
                    }
                }
                7 => Packet::NeedKeyframe,
                _ => return Err(Error::Device),
            });
        }
        Ok(packets)
    }
    fn receive(&mut self, bytes: &[u8], orientation: u8) -> Result<(), Error> {
        self.orientation = orientation;
        if unsafe { heya_call_video_receive(self.handle.as_ptr(), bytes.as_ptr(), bytes.len()) }
            == 0
        {
            return Err(Error::Codec);
        }
        Ok(())
    }
    fn camera(&mut self, enabled: bool) -> Result<(), Error> {
        if unsafe { heya_call_video_camera(self.handle.as_ptr(), i32::from(enabled)) } == 0 {
            return Err(Error::Device);
        }
        Ok(())
    }
    fn keyframe(&mut self) {
        unsafe {
            heya_call_video_keyframe(self.handle.as_ptr());
        }
    }
}
