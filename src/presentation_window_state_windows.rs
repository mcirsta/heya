//! Win32 owns placement coordinates, normal bounds and off-screen adjustment.
#![allow(unsafe_code)]
use crate::window_state::Saved;
use slint::winit_031::winit::{raw_window_handle::RawWindowHandle, window::Window};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{
        GetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWNORMAL, SetWindowPlacement, WINDOWPLACEMENT,
        WPF_RESTORETOMAXIMIZED,
    },
};
fn hwnd(window: &dyn Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    if let RawWindowHandle::Win32(handle) = handle.as_raw() {
        Some(handle.hwnd.get() as HWND)
    } else {
        None
    }
}
pub fn capture(window: &dyn Window) -> Option<([i32; 4], bool)> {
    let mut p = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    // The borrowed native handle and stack output live through this UI-thread call.
    if unsafe { GetWindowPlacement(hwnd(window)?, &mut p) } == 0 {
        return None;
    }
    let r = p.rcNormalPosition;
    Some((
        [r.left, r.top, r.right, r.bottom],
        p.showCmd == SW_SHOWMAXIMIZED as u32 || p.flags & WPF_RESTORETOMAXIMIZED != 0,
    ))
}
pub fn restore(window: &dyn Window, saved: &Saved) {
    let Some(hwnd) = hwnd(window) else {
        return;
    };
    let Some([left, top, right, bottom]) = saved.windows_normal else {
        return;
    };
    let p = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: if saved.maximized {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        } as u32,
        rcNormalPosition: RECT {
            left,
            top,
            right,
            bottom,
        },
        ..Default::default()
    };
    // Never restore minimization; SetWindowPlacement also handles removed monitors.
    unsafe { SetWindowPlacement(hwnd, &p) };
}
