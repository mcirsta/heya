//! Real pointer routing against the toolkit's private test backend.
use crate::{AppWindow, Theme, fixtures, presentation};
use i_slint_backend_testing::ElementHandle;
use slint::{
    ComponentHandle, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::{cell::Cell, rc::Rc, sync::Arc};

fn child(parent: &ElementHandle, id: &str) -> ElementHandle {
    parent
        .query_descendants()
        .match_id(id)
        .find_first()
        .unwrap_or_else(|| panic!("missing {id}"))
}
fn center(item: &ElementHandle) -> slint::LogicalPosition {
    let p = item.absolute_position();
    let s = item.size();
    slint::LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.)
}
fn move_to(ui: &AppWindow, p: slint::LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: p });
}
fn click(ui: &AppWindow, p: slint::LogicalPosition) {
    move_to(ui, p);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: p,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: p,
        button: PointerEventButton::Left,
    });
}
fn painted(button: &ElementHandle) -> bool {
    button
        .query_descendants()
        .match_id("HoverButton::glyph")
        .find_first()
        .is_some()
}

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let clicked = Rc::new(Cell::new((0, -1)));
    ui.on_message_action({
        let clicked = clicked.clone();
        move |id, action, _, _| clicked.set((id, action))
    });
    let content = Rc::new(Cell::new((0, -1)));
    ui.on_activate_photo({
        let content = content.clone();
        move |id| content.set((id, -5))
    });
    ui.on_activate_message({
        let content = content.clone();
        move |id, a| content.set((id, a))
    });
    ui.on_audio_control({
        let content = content.clone();
        move |id, a, _| content.set((id, a))
    });
    let mut demo = fixtures::demo();
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for width in [1200., 800., 620., 520.] {
            ui.window().set_size(slint::LogicalSize::new(width, 1000.));
            for chat in [1, 7, 9, 10] {
                demo.select(chat);
                let mut view = demo.conversation();
                view.messages = Arc::from([view.messages[0].clone()]);
                for outgoing in [false, true] {
                    Arc::make_mut(&mut view.messages)[0].outgoing = outgoing;
                    presentation::conversation(&ui, &view, true);
                    ui.show().unwrap();
                    ui.invoke_layout_timeline();
                    let model = ui.get_timeline_rows();
                    let geometry = (ui.get_timeline_height(), ui.get_timeline_y());
                    let item = ElementHandle::find_by_element_type_name(&ui, "MessageBubble")
                        .next()
                        .unwrap();
                    let bubble = child(&item, "MessageBubble::bubble");
                    let controls = child(&item, "MessageBubble::hover-controls");
                    let react = child(&controls, "MessageHoverControls::react");
                    let reply = child(&controls, "MessageHoverControls::reply");
                    let forward = child(&controls, "MessageHoverControls::forward");
                    let b = bubble.absolute_position();
                    let bs = bubble.size();
                    let r = react.absolute_position();
                    assert_eq!(react.size(), slint::LogicalSize::new(26., 26.));
                    assert!((r.y + 13. - b.y - bs.height / 2.).abs() < 0.5);
                    if outgoing {
                        assert!((b.x - r.x - 32.).abs() < 0.5);
                        assert!((r.x - reply.absolute_position().x - 30.).abs() < 0.5);
                        assert!((r.x - forward.absolute_position().x - 66.).abs() < 0.5);
                    } else {
                        assert!((r.x - b.x - bs.width - 6.).abs() < 0.5);
                        assert!((reply.absolute_position().x - r.x - 30.).abs() < 0.5);
                        assert!((forward.absolute_position().x - r.x - 66.).abs() < 0.5);
                    }
                    for button in [&react, &reply, &forward] {
                        assert!(
                            button.absolute_position().x >= controls.absolute_position().x + 1.5
                        );
                        assert!(
                            button.absolute_position().x + 26.
                                <= controls.absolute_position().x + controls.size().width - 1.5
                        );
                    }
                    move_to(&ui, slint::LogicalPosition::new(5., 5.));
                    // A previously keyboard-focused action may remain painted.
                    click(
                        &ui,
                        center(
                            &ElementHandle::find_by_accessible_label(&ui, "Message draft")
                                .next()
                                .unwrap(),
                        ),
                    );
                    assert!(!painted(&react) && !painted(&reply) && !painted(&forward));
                    move_to(&ui, center(&bubble));
                    assert!(painted(&react) && painted(&reply) && painted(&forward));
                    // Cross the gap, then both further buttons. The controls
                    // remain revealed throughout, including over child media.
                    let gap = if outgoing {
                        b.x - 3.
                    } else {
                        b.x + bs.width + 3.
                    };
                    move_to(&ui, slint::LogicalPosition::new(gap, r.y + 13.));
                    assert!(painted(&react) && painted(&reply) && painted(&forward));
                    for (action, button) in [&react, &reply, &forward].into_iter().enumerate() {
                        click(&ui, center(button));
                        assert_eq!(clicked.get(), (view.messages[0].id, action as i32));
                    }
                    move_to(&ui, slint::LogicalPosition::new(5., 5.));
                    assert!(!painted(&react) && !painted(&reply) && painted(&forward));
                    clicked.set((0, -1));
                    ui.window()
                        .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
                    ui.window()
                        .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
                    assert_eq!(clicked.get(), (view.messages[0].id, 2));
                    assert_eq!(model, ui.get_timeline_rows());
                    assert_eq!(geometry, (ui.get_timeline_height(), ui.get_timeline_y()));
                    // Modal layers must remove the actions, not just dim them.
                    ui.set_new_chat_open(true);
                    assert!(
                        ElementHandle::find_by_element_type_name(&ui, "HoverButton")
                            .next()
                            .is_none()
                    );
                    ui.set_new_chat_open(false);
                    if chat == 9 {
                        click(&ui, center(&child(&item, "MessageBubble::photo")));
                        assert_eq!(content.get(), (view.messages[0].id, -5));
                    }
                    if chat == 10 {
                        click(&ui, center(&child(&item, "AudioPlayer::play")));
                        assert_eq!(content.get(), (view.messages[0].id, 0));
                    }
                    if chat == 7 {
                        click(&ui, center(&child(&item, "MessageBubble::preview")));
                        assert_eq!(content.get(), (view.messages[0].id, -1));
                    }
                }
            }
        }
    }
    // Controls follow the visible slice of a tall message, and disappear
    // once less than their own height remains. No clipped row catches clicks.
    demo.select(1);
    let mut view = demo.conversation();
    let mut message = view.messages[0].clone();
    message.text = "Synthetic long paragraph\n".repeat(200);
    view.messages = Arc::from([message]);
    ui.window().set_size(slint::LogicalSize::new(800., 600.));
    presentation::conversation(&ui, &view, true);
    ui.invoke_layout_timeline();
    let item = ElementHandle::find_by_element_type_name(&ui, "MessageBubble")
        .next()
        .unwrap();
    let react = child(&item, "MessageHoverControls::react");
    let p = react.absolute_position();
    assert!(p.y > 60. && p.y + 26. < 550., "visible-slice anchor {p:?}");
    demo.select(6);
    let mut many = demo.conversation();
    many.messages = demo.chats[5].messages.clone().into();
    presentation::conversation(&ui, &many, true);
    assert!(ui.get_timeline_rows().row_count() < 32);
    assert!(ElementHandle::find_by_element_type_name(&ui, "HoverButton").count() < 96);
    let before = ui.get_timeline_y();
    let row = ElementHandle::find_by_element_id(&ui, "MessageBubble::body")
        .find(|item| center(item).y > 100. && center(item).y < 400.)
        .unwrap();
    move_to(&ui, center(&row));
    crate::presentation_scroll::dispatch_wheel(ui.window(), center(&row), 0., 120.);
    std::thread::sleep(std::time::Duration::from_millis(250));
    slint::platform::update_timers_and_animations();
    ui.invoke_layout_timeline();
    assert!(
        ui.get_timeline_y() > before,
        "hover observer must pass wheel events to the timeline"
    );
    ui.hide().unwrap();
}

pub(crate) fn check_action_flows() {
    use crate::message_actions::{Command, OperationReply};
    use std::cell::RefCell;
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    ui.window().set_size(slint::LogicalSize::new(800., 700.));
    let pending = Rc::new(RefCell::new(Vec::new()));
    crate::presentation_actions::bind(&ui, {
        let pending = pending.clone();
        move |command| {
            pending.borrow_mut().push(command);
            true
        }
    });
    let mut demo = fixtures::demo();
    demo.select(1);
    presentation::conversation(&ui, &demo.conversation(), true);
    ui.show().unwrap();
    ui.invoke_layout_timeline();
    let id = ui.get_timeline_rows().row_data(0).unwrap().message.id;
    // Reply enters an explicit quoted draft without sending anything.
    ui.invoke_message_action(id, 3, 400., 300.);
    assert!(pending.borrow().is_empty());
    ui.invoke_choose_message_menu(1);
    assert!(ui.get_action_pending());
    let command = pending.borrow_mut().pop().unwrap();
    assert!(matches!(command, Command::Reply {message,..} if message==id));
    let (result, targets) = demo.message_action(command);
    presentation::conversation(&ui, &demo.conversation(), false);
    crate::presentation_actions::publish(&ui, Some(&result), &targets);
    assert_eq!(ui.get_reply_id(), id);
    assert!(!ui.get_reply_text().is_empty());
    ui.invoke_cancel_reply();
    let (result, targets) = demo.message_action(pending.borrow_mut().pop().unwrap());
    presentation::conversation(&ui, &demo.conversation(), false);
    crate::presentation_actions::publish(&ui, Some(&result), &targets);
    assert_eq!(ui.get_reply_id(), 0);

    // Opening a picker is inert. Searching creates a bounded model, and only
    // clicking an emoji issues an action for the captured message/chat.
    let react_button = ElementHandle::find_by_element_id(&ui, "MessageHoverControls::react")
        .next()
        .unwrap();
    click(&ui, center(&react_button));
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_message_dialog(),
        1,
        "physical hover-button click must keep picker open"
    );
    assert!(pending.borrow().is_empty());
    let panel = ElementHandle::find_by_element_id(&ui, "EmojiPicker::panel")
        .next()
        .unwrap();
    assert!((280.0..=440.0).contains(&panel.size().width));
    assert_eq!(panel.size().height, 420.);
    assert!(panel.absolute_position().x >= 12.);
    assert!(panel.absolute_position().y + panel.size().height <= 688.);
    ui.invoke_search_message_actions("thumbs up".into());
    assert!(ui.get_emoji_rows().row_count() > 0 && ui.get_emoji_rows().row_count() <= 20);
    let quick = ElementHandle::find_by_accessible_label(&ui, "React with thumbs up")
        .next()
        .unwrap();
    click(&ui, center(&quick));
    let command = pending.borrow_mut().pop().unwrap();
    assert!(matches!(&command, Command::React {message,emoji,..} if *message==id && emoji=="👍"));
    let (result, targets) = demo.message_action(command);
    presentation::conversation(&ui, &demo.conversation(), false);
    crate::presentation_actions::publish(&ui, Some(&result), &targets);
    assert_eq!(ui.get_message_dialog(), 0);
    assert!(
        ui.get_timeline_rows()
            .iter()
            .any(|r| r.message.id == id && r.message.reactions == "👍")
    );
    ui.invoke_message_action(id, 0, 400., 300.);
    let remove = ElementHandle::find_by_accessible_label(&ui, "Remove your reaction")
        .next()
        .unwrap();
    click(&ui, center(&remove));
    let command = pending.borrow_mut().pop().unwrap();
    assert!(matches!(&command, Command::React {emoji,..} if emoji.is_empty()));
    let (result, targets) = demo.message_action(command);
    presentation::conversation(&ui, &demo.conversation(), false);
    crate::presentation_actions::publish(&ui, Some(&result), &targets);

    // A destination must be selected and confirmed. Neither opening nor
    // searching forwards anything. Stale results cannot settle a newer action.
    ui.invoke_message_action(id, 3, 400., 300.);
    assert!(pending.borrow().is_empty());
    ui.invoke_choose_message_menu(2);
    let command = pending.borrow_mut().pop().unwrap();
    assert!(matches!(command, Command::Destinations { .. }));
    let (result, targets) = demo.message_action(command);
    let stale = OperationReply {
        request: result.request - 1,
        ..result.clone()
    };
    crate::presentation_actions::publish(&ui, Some(&stale), &targets);
    assert!(ui.get_action_pending());
    crate::presentation_actions::publish(&ui, Some(&result), &targets);
    let destination = ui
        .get_action_destinations()
        .iter()
        .find(|d| d.id != 1)
        .unwrap();
    let recipient = ElementHandle::find_by_accessible_label(&ui, &destination.title)
        .next()
        .unwrap();
    click(&ui, center(&recipient));
    assert!(pending.borrow().is_empty());
    let confirm = ElementHandle::find_by_accessible_label(&ui, "Forward")
        .next()
        .unwrap();
    click(&ui, center(&confirm));
    let command = pending.borrow_mut().pop().unwrap();
    assert!(
        matches!(&command, Command::Forward {message,destination:to,..} if *message==id && *to==destination.id)
    );
    let (result, targets) = demo.message_action(command);
    crate::presentation_actions::publish(&ui, Some(&result), &targets);
    assert_eq!(ui.get_message_dialog(), 0);

    ui.invoke_message_action(id, 0, 400., 300.);
    ui.set_selected_id(2);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_message_dialog(), 0);
    ui.invoke_react_message("👍".into());
    assert!(pending.borrow().is_empty());
    ui.hide().unwrap();
}
