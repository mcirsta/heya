//! Exercise Skia's actual CPU surface without a native window or graphics device.
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;

use i_slint_core::partial_renderer::DirtyRegion;
use i_slint_renderer_skia::software_surface::{RenderBuffer, SoftwareSurface};
use i_slint_renderer_skia::{SkiaRenderer, skia_safe::ColorType};
use slint::platform::{Platform, PlatformError, Renderer, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize, Window};

struct CpuBuffer(Rc<RefCell<Vec<u8>>>);

impl RenderBuffer for CpuBuffer {
    fn with_buffer(
        &self,
        _window: &Window,
        size: PhysicalSize,
        render: &mut dyn FnMut(
            NonZeroU32,
            NonZeroU32,
            ColorType,
            u8,
            &mut [u8],
        ) -> Result<Option<DirtyRegion>, PlatformError>,
    ) -> Result<(), PlatformError> {
        let mut pixels = self.0.borrow_mut();
        pixels.resize(size.width as usize * size.height as usize * 4, 0);
        render(
            size.width.try_into().unwrap(),
            size.height.try_into().unwrap(),
            ColorType::BGRA8888,
            0,
            &mut pixels,
        )?;
        Ok(())
    }
}

struct CpuWindow {
    window: Window,
    renderer: SkiaRenderer,
    size: Cell<PhysicalSize>,
}

impl WindowAdapter for CpuWindow {
    fn window(&self) -> &Window {
        &self.window
    }
    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }
    fn size(&self) -> PhysicalSize {
        self.size.get()
    }
}

struct CpuPlatform(Rc<CpuWindow>);

impl Platform for CpuPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

#[test]
fn app_renders_and_resizes_on_skia_cpu_without_a_gpu() {
    let pixels = Rc::new(RefCell::new(Vec::new()));
    let adapter = Rc::new_cyclic(|weak| CpuWindow {
        window: Window::new(weak.clone()),
        renderer: SkiaRenderer::new_with_surface(
            &Default::default(),
            Box::new(SoftwareSurface::from(CpuBuffer(pixels.clone()))),
        ),
        size: Cell::new(PhysicalSize::new(1200, 800)),
    });
    slint::platform::set_platform(Box::new(CpuPlatform(adapter.clone()))).unwrap();
    let ui = crate::AppWindow::new().unwrap();
    ui.set_show_conversation(true);
    ui.set_draft("Synthetic text 😅\nUnicode: sănătoși, 日本語".into());
    ui.show().unwrap();
    for (width, height) in [(1200, 800), (520, 700), (1000, 600)] {
        adapter.size.set(PhysicalSize::new(width, height));
        ui.window().dispatch_event(WindowEvent::Resized {
            size: slint::LogicalSize::new(width as f32, height as f32),
        });
        slint::platform::update_timers_and_animations();
        assert!(matches!(
            adapter.renderer.render().unwrap(),
            i_slint_core::renderer::DrawOutcome::Success
        ));
        let pixels = pixels.borrow();
        assert_eq!(pixels.len(), width as usize * height as usize * 4);
        let (colors, _) = pixels.as_chunks::<4>();
        assert!(colors.iter().any(|pixel| pixel != &colors[0]));
    }
    ui.hide().unwrap();
}
