use super::*;
use crate::{fixtures, presentation};
use i_slint_backend_testing::ElementHandle;
use std::sync::Arc;

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let (commands, mut requests) = mpsc::channel(16);
    bind(&ui, commands);
    let mut demo = fixtures::demo();
    demo.select(14);
    let view = demo.conversation();
    let id = view.messages.last().unwrap().id;
    let pixels = Arc::new(fixtures::video_pixels(0.));
    for (width, height) in [(1200., 900.), (520., 700.)] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        presentation::conversation(&ui, &view, true);
        ui.show().unwrap();
        ui.invoke_layout_timeline();
        while requests.try_recv().is_ok() {}
        ui.invoke_video_control(id, 0, 0.);
        assert!(matches!(requests.try_recv().unwrap(),Command::Toggle(i) if i==id));
        let geometry = (ui.get_timeline_y(), ui.get_timeline_height());
        presentation::timeline::take_samples();
        let mut status = Status {
            id,
            phase: 2,
            duration: 5.,
            position: 1.,
            frame: Some(pixels.clone()),
            frame_number: 1,
            ..Default::default()
        };
        publish(&ui, &status);
        let image = ui.global::<VideoPlayback>().get_frame();
        status.position = 1.1;
        publish(&ui, &status);
        assert_eq!(
            image,
            ui.global::<VideoPlayback>().get_frame(),
            "a progress tick reuses its existing texture"
        );
        assert_eq!(geometry, (ui.get_timeline_y(), ui.get_timeline_height()));
        assert_eq!(presentation::timeline::take_samples().measured_rows, 0);
        ui.invoke_video_control(id, 4, 0.);
        assert!(ui.get_video_expanded());
        let viewer = ElementHandle::find_by_element_id(&ui, "AppWindow::video-viewer")
            .next()
            .unwrap();
        let rect = viewer.absolute_position();
        let size = viewer.size();
        assert!(
            rect.x >= 0.
                && rect.y >= 0.
                && rect.x + size.width <= width
                && rect.y + size.height <= height
        );
        ui.invoke_video_control(id, 4, 0.);
        assert!(!ui.get_video_expanded());
        ui.invoke_video_control(id, 1, 0.6);
        assert!(
            matches!(requests.try_recv().unwrap(),Command::Seek(i,v) if i==id && (v-0.6).abs()<0.001)
        );
        ui.invoke_video_control(id, 2, 0.);
        assert!(matches!(requests.try_recv().unwrap(),Command::Mute(i) if i==id));
        ui.invoke_video_control(id, 3, 0.);
        assert!(matches!(requests.try_recv().unwrap(), Command::Stop));
        publish(&ui, &Status::default());
        assert_eq!(ui.global::<VideoPlayback>().get_frame().size().width, 0);
        assert_eq!(geometry, (ui.get_timeline_y(), ui.get_timeline_height()));
    }
    ui.hide().unwrap();
}
