//! Main-window lifecycle only: bounded in-memory samples, one save on exit.
use crate::{
    AppWindow,
    window_state::{self, Saved},
};
use slint::{
    ComponentHandle,
    winit_031::{
        WinitWindowAccessor,
        winit::{event::WindowEvent, window::WindowId},
    },
};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

#[derive(Clone)]
pub struct State(Rc<RefCell<Inner>>);
struct Inner {
    enabled: bool,
    native_macos: bool,
    path: Option<PathBuf>,
    saved: Saved,
    ui: Option<slint::Weak<AppWindow>>,
    window: Option<WindowId>,
    dirty: bool,
    ready: bool,
    restore: bool,
}
impl State {
    pub fn new(demo: bool) -> Self {
        // A synthetic native test must opt into its own file, never owner state.
        let path = if demo {
            std::env::var_os("HEYA_TEST_WINDOW_STATE")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        } else if cfg!(target_os = "macos") {
            None
        } else {
            crate::preferences::path().map(|p| p.with_file_name("window-state.json"))
        };
        Self(Rc::new(RefCell::new(Inner {
            enabled: !demo || path.is_some(),
            native_macos: cfg!(target_os = "macos") && !demo,
            path,
            saved: Saved::default(),
            ui: None,
            window: None,
            dirty: false,
            ready: false,
            restore: true,
        })))
    }
    pub fn enabled(&self) -> bool {
        self.0.borrow().enabled
    }
    pub fn path(&self) -> Option<PathBuf> {
        self.0.borrow().path.clone()
    }
    pub fn attach(&self, ui: &AppWindow, loaded: Saved, explicit_size: bool) {
        let mut state = self.0.borrow_mut();
        state.ui = Some(ui.as_weak());
        state.restore = !explicit_size;
        state.saved = loaded;
        if state.enabled && state.restore && !state.native_macos {
            // Set Slint's explicit initial size before its preferred-size layout.
            ui.window().set_size(slint::LogicalSize::new(
                state.saved.width as f32,
                state.saved.height as f32,
            ));
            if !cfg!(target_os = "windows") {
                ui.window().set_maximized(state.saved.maximized);
            }
        }
    }
    // Restore fullscreen on the realized native window, matching the native
    // placement restoration order on macOS and Windows.
    pub fn shown(&self, ui: &AppWindow) {
        if !self.enabled() || self.0.borrow().ready || !ui.window().has_winit_window() {
            return;
        }
        let (saved, restore, native_macos) = {
            let state = self.0.borrow();
            (state.saved.clone(), state.restore, state.native_macos)
        };
        if restore {
            #[cfg(target_os = "macos")]
            if native_macos {
                ui.window().with_winit_window(|w| {
                    use slint::winit_031::winit::raw_window_handle::{
                        HasWindowHandle, RawWindowHandle,
                    };
                    if let Ok(handle) = w.window_handle()
                        && let RawWindowHandle::AppKit(h) = handle.as_raw()
                    {
                        let (size, flags) = crate::macos::restore_window(h.ns_view);
                        if size[0] >= 520. && size[1] >= 480. {
                            ui.window()
                                .set_size(slint::LogicalSize::new(size[0] as f32, size[1] as f32));
                        }
                        self.0.borrow_mut().saved.maximized = flags & 1 != 0;
                        ui.window().set_maximized(flags & 1 != 0);
                        ui.window().set_fullscreen(flags & 2 != 0);
                    }
                });
            }
            #[cfg(target_os = "windows")]
            ui.window().with_winit_window(|w| {
                super::presentation_window_state_windows::restore(w, &saved);
                ui.window().set_maximized(saved.maximized);
                ui.window().set_fullscreen(saved.fullscreen);
            });
            if !native_macos && !cfg!(target_os = "windows") {
                ui.window().with_winit_window(|w| {
                    let monitor = w
                        .current_monitor()
                        .or_else(|| w.available_monitors().next());
                    let screen = monitor.and_then(|m| {
                        Some((m.current_video_mode()?.size().into(), m.scale_factor()))
                    });
                    let (width, height) = window_state::fit(&saved, screen);
                    if !saved.maximized && !saved.fullscreen {
                        self.0.borrow_mut().saved.width = width;
                        self.0.borrow_mut().saved.height = height;
                        ui.window()
                            .set_size(slint::LogicalSize::new(width as f32, height as f32));
                    }
                });
                ui.window().set_maximized(saved.maximized);
                ui.window().set_fullscreen(saved.fullscreen);
            }
        }
        self.0.borrow_mut().ready = true;
        self.capture();
    }
    pub fn event(&self, id: WindowId, event: &WindowEvent) {
        if !self.enabled() {
            return;
        }
        if !matches!(
            event,
            WindowEvent::SurfaceResized(_)
                | WindowEvent::Moved(_)
                | WindowEvent::ScaleFactorChanged { .. }
                | WindowEvent::CloseRequested
        ) {
            return;
        }
        {
            let mut state = self.0.borrow_mut();
            if state.window.is_some_and(|w| w != id) {
                return;
            }
            state.window = Some(id);
            state.dirty = true;
        }
        // Capture before Slint destroys the native window on an accepted close.
        if matches!(event, WindowEvent::CloseRequested) {
            self.capture();
        }
    }
    pub fn pending(&self) {
        let state = self.0.borrow();
        if !state.dirty {
            return;
        }
        let ready = state.ready;
        let ui = state.ui.as_ref().and_then(|u| u.upgrade());
        drop(state);
        if ready {
            self.capture();
        } else if let Some(ui) = ui {
            // Native handles can appear only after the event loop starts.
            self.shown(&ui);
        }
    }
    fn capture(&self) {
        let ui = {
            let mut state = self.0.borrow_mut();
            if !state.ready {
                return;
            }
            state.dirty = false;
            state.ui.as_ref().and_then(|u| u.upgrade())
        };
        let Some(ui) = ui else {
            return;
        };
        // Slint already processed the resize and queried native mode flags. Do
        // not add more X11 round trips, decode work or file writes during resize.
        let max = ui.window().is_maximized();
        let full = ui.window().is_fullscreen();
        let min = ui.window().is_minimized();
        ui.window().with_winit_window(|w| {
            let size = w.surface_size().to_logical::<f64>(w.scale_factor());
            let mut state = self.0.borrow_mut();
            state.saved.observe(size.width, size.height, max, full, min);
            #[cfg(target_os = "windows")]
            if !full
                && let Some((normal, maximized)) =
                    super::presentation_window_state_windows::capture(w)
            {
                state.saved.windows_normal = Some(normal);
                state.saved.maximized = maximized;
            }
            #[cfg(target_os = "macos")]
            if state.native_macos && !min {
                use slint::winit_031::winit::raw_window_handle::{
                    HasWindowHandle, RawWindowHandle,
                };
                if let Ok(handle) = w.window_handle()
                    && let RawWindowHandle::AppKit(h) = handle.as_raw()
                {
                    crate::macos::save_window(
                        h.ns_view,
                        (!max && !full) as u32,
                        state.saved.maximized as u32 | ((full as u32) << 1),
                    );
                }
            }
        });
    }
    pub fn finish(&self) -> Option<(PathBuf, Saved)> {
        if !self.enabled() {
            return None;
        }
        self.capture();
        let state = self.0.borrow();
        if !state.ready {
            return None;
        }
        Some((state.path.clone()?, state.saved.clone()))
    }
}
