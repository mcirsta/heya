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

fn platform() -> (Rc<CpuWindow>, Rc<RefCell<Vec<u8>>>) {
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
    (adapter, pixels)
}

#[test]
fn app_renders_and_resizes_on_skia_cpu_without_a_gpu() {
    let (adapter, pixels) = platform();
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

slint::slint! {
    export component OpacityScene inherits Window {
        width: 400px; height: 200px; background: white;
        // Overflow must participate in group opacity, including its overlap.
        Rectangle {
            x: 20px; y: 20px; width: 40px; height: 40px;
            background: red; opacity: 0.5;
            Rectangle {x: -10px; y: 10px; width: 70px; height: 20px; background: blue;}
        }
        // Transform the overflowing child, not the opacity item's geometry.
        Rectangle {
            x: 100px; y: 20px; width: 20px; height: 20px; opacity: 0.5;
            Rectangle {x: 0px; y: 0px; width: 40px; height: 20px; background: blue; transform-rotation: 90deg; transform-origin: {x: 0px, y: 0px};}
            Rectangle {width: 10px; height: 10px; background: blue;}
        }
        // An offscreen parent may have a visible child inside an ancestor clip.
        Rectangle {
            x: 200px; y: 20px; width: 50px; height: 50px; clip: true;
            Rectangle {
                x: 2000px; y: -8000px; width: 50px; height: 50px;
                background: red; opacity: 0.5;
                Rectangle {x: -1990px; y: 8030px; width: 50px; height: 50px; background: blue;}
            }
        }
    }
}

#[test]
fn bounded_opacity_layers_preserve_overflow_transforms_and_clipping() {
    let (adapter, pixels) = platform();
    let ui = OpacityScene::new().unwrap();
    ui.show().unwrap();
    for (width, height, scale) in [
        (400, 200, 1.),
        (900, 600, 1.),
        (400, 200, 2.),
        (400, 200, 1.),
    ] {
        ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        let physical = PhysicalSize::new(
            (width as f32 * scale) as u32,
            (height as f32 * scale) as u32,
        );
        adapter.size.set(physical);
        ui.window().dispatch_event(WindowEvent::Resized {
            size: slint::LogicalSize::new(width as f32, height as f32),
        });
        assert!(matches!(
            adapter.renderer.render().unwrap(),
            i_slint_core::renderer::DrawOutcome::Success
        ));
        let pixels = pixels.borrow();
        let check = |x: usize, y: usize, bgra: [u8; 4]| {
            let offset = ((y as f32 * scale) as usize * physical.width as usize
                + (x as f32 * scale) as usize)
                * 4;
            let actual = &pixels[offset..offset + 4];
            assert!(
                actual.iter().zip(bgra).all(|(&a, b)| a.abs_diff(b) <= 2),
                "pixel ({x},{y}) at {width}x{height}: {actual:?}, expected {bgra:?}"
            );
        };
        check(25, 25, [128, 128, 255, 255]);
        check(15, 35, [255, 128, 128, 255]);
        check(25, 35, [255, 128, 128, 255]);
        check(90, 50, [255, 128, 128, 255]);
        check(220, 60, [255, 128, 128, 255]);
        check(260, 60, [255, 255, 255, 255]);
    }
    ui.hide().unwrap();
}
