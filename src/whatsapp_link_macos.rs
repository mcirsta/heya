//! AppKit delivers URLs to the main thread on both cold and warm launches.
#![allow(unsafe_code)]

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    sync::OnceLock,
};

type Callback = Box<dyn Fn(String) + Send + Sync>;
static OPEN_URL: OnceLock<Callback> = OnceLock::new();

unsafe extern "C" {
    fn heya_whatsapp_links_install(open_url: extern "C" fn(*const c_char)) -> c_int;
    fn heya_whatsapp_links_set_default(
        context: *mut c_void,
        reply: extern "C" fn(*mut c_void, c_int),
    );
}

/// Install after Winit initializes AppKit and before the native event loop starts.
pub(crate) fn install(callback: impl Fn(String) + Send + Sync + 'static) {
    if OPEN_URL.set(Box::new(callback)).is_ok() {
        // The delegate retains the function pointer for the process lifetime.
        let installed = unsafe { heya_whatsapp_links_install(open_url) };
        if installed == 0 {
            eprintln!("Heya: could not install the macOS WhatsApp link handler.");
        }
    }
}

/// Tell macOS to make this bundle the preferred handler from a Settings action.
/// The caller runs on the main thread, and the completion may run on any thread.
pub(crate) fn set_default<F: FnOnce(bool) + Send + 'static>(callback: F) {
    extern "C" fn completed<F: FnOnce(bool) + Send + 'static>(context: *mut c_void, status: c_int) {
        // AppKit's completion owns the box and calls this exactly once, including
        // the native bridge's synchronous failure path.
        let callback = unsafe { Box::from_raw(context.cast::<F>()) };
        callback(status != 0);
    }

    let context = Box::into_raw(Box::new(callback)).cast();
    unsafe { heya_whatsapp_links_set_default(context, completed::<F>) };
}

extern "C" fn open_url(url: *const c_char) {
    // The delegate passes the non-null, NUL-terminated string from NSURL and
    // keeps it alive through this call. Give the application its own copy.
    let url = unsafe { CStr::from_ptr(url) };
    if let (Ok(url), Some(callback)) = (url.to_str(), OPEN_URL.get()) {
        callback(url.to_owned());
    }
}
