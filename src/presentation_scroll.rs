//! Translate wheel notches once at the window boundary; Slint owns smoothing,
//! hit testing, scrollbars and touchpad gestures. No per-row input machinery.
use crate::AppWindow;
use slint::{
    ComponentHandle, LogicalPosition, Window,
    winit_031::{EventResult, WinitWindowAccessor, winit::event},
};

// Same logical distance as the pinned ZapFast/fastframe reference.
pub(crate) const WHEEL_STEP: f32 = 120.;

pub fn bind(ui: &AppWindow) {
    let mut pointer = None;
    let mut context_pressed = None;
    let weak = ui.as_weak();
    ui.window().on_winit_window_event(move |window, event| {
        match event {
            event::WindowEvent::Focused(focused) => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_read_window_active(*focused);
                }
            }
            event::WindowEvent::Occluded(occluded) => {
                if let Some(ui) = weak.upgrade() {
                    ui.set_read_window_occluded(*occluded);
                }
            }
            event::WindowEvent::PointerMoved {
                position,
                source: event::PointerSource::Mouse,
                ..
            } => pointer = Some(*position),
            event::WindowEvent::PointerLeft {
                kind: event::PointerKind::Mouse,
                ..
            } => {
                pointer = None;
            }
            event::WindowEvent::PointerButton {
                state,
                position,
                button: event::ButtonSource::Mouse(event::MouseButton::Right),
                ..
            } => {
                if let Some(ui) = weak.upgrade() {
                    let p = position.to_logical::<f32>(f64::from(window.scale_factor()));
                    if context_pointer(
                        &ui,
                        LogicalPosition::new(p.x, p.y),
                        *state == event::ElementState::Pressed,
                        &mut context_pressed,
                    ) {
                        return EventResult::PreventDefault;
                    }
                }
            }
            event::WindowEvent::MouseWheel { delta, .. } => {
                // Pixel deltas (including high-resolution touchpads) and their
                // gesture phases go through the backend completely unchanged.
                if let (Some(position), Some((x, y))) = (pointer, wheel_delta(delta)) {
                    let p = position.to_logical::<f32>(f64::from(window.scale_factor()));
                    dispatch_wheel(window, LogicalPosition::new(p.x, p.y), x, y);
                    return EventResult::PreventDefault;
                }
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

/// Share the exact native input transition with synthetic layout tests. A child
/// TouchArea consumes right clicks in Slint, so handle them before dispatch.
pub(crate) fn context_pointer(
    ui: &AppWindow,
    p: LogicalPosition,
    pressed: bool,
    target: &mut Option<i32>,
) -> bool {
    let hit = ui
        .invoke_context_allowed(p.x, p.y)
        .then(|| crate::presentation::timeline::context_target(ui, p.y))
        .flatten();
    if pressed {
        *target = hit;
        return hit.is_some();
    }
    if let Some(id) = target.take() {
        if hit == Some(id) {
            ui.invoke_message_action(id, 17, p.x, p.y);
        }
        return true;
    }
    false
}

fn wheel_delta(delta: &event::MouseScrollDelta) -> Option<(f32, f32)> {
    match delta {
        event::MouseScrollDelta::LineDelta(x, y) => Some((x * WHEEL_STEP, y * WHEEL_STEP)),
        _ => None,
    }
}

// Slint's public PointerScrolled dispatch hardcodes Cancelled, which
// bypasses Flickable's 180 ms wheel animation. Keep this version-specific bridge
// here until the public event can express phases or a wheel-step setting exists.
// This is the same event path as its winit backend, with only the distance changed.
pub(crate) fn dispatch_wheel(window: &Window, position: LogicalPosition, x: f32, y: f32) {
    use i_slint_core::input::{BackendMouseEvent, TouchPhase};
    window.dispatch_event(slint::platform::WindowEvent::internal(
        BackendMouseEvent::Wheel {
            position: i_slint_core::lengths::LogicalPoint::new(position.x, position.y),
            delta_x: x,
            delta_y: y,
            phase: TouchPhase::Moved,
        },
    ));
}

/// Synthetic Xvfb input diagnostic. Called only by the explicit --demo harness;
/// never logs account positions, message IDs or other account state.
pub(crate) fn trace(ui: &AppWindow) -> slint::Timer {
    use slint::{Model, TimerMode};
    use std::time::{Duration, Instant};
    if std::env::var_os("HEYA_TEST_DEFAULT_WHEEL").is_some() {
        // Isolate Slint's unmodified native wheel path in the same test binary.
        ui.window()
            .on_winit_window_event(|_, _| EventResult::Propagate);
    }
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let start = Instant::now();
    let mut last = None;
    timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
        let Some(ui) = weak.upgrade() else { return };
        let y = ui.get_timeline_y();
        let rows = ui.get_timeline_rows();
        let visible = rows.iter().find(|r| r.top + y >= 0.);
        let position = visible.map(|r| (r.message.id, r.top + y));
        if last != Some((y, position)) {
            println!(
                "Wheel trace: ms={} y={y:.2} anchor={position:?} rows={}",
                start.elapsed().as_millis(),
                rows.row_count()
            );
            last = Some((y, position));
        }
    });
    timer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_units_are_scaled_without_guessing_from_delta_magnitude() {
        assert_eq!(
            wheel_delta(&event::MouseScrollDelta::LineDelta(0., 1.)),
            Some((0., 120.))
        );
        assert_eq!(
            wheel_delta(&event::MouseScrollDelta::LineDelta(-0.5, -2.)),
            Some((-60., -240.))
        );
        for y in [0.5, 60., 120., -240.] {
            assert_eq!(
                wheel_delta(&event::MouseScrollDelta::PixelDelta((0., y).into())),
                None
            );
        }
    }
}
