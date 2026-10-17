//! Automatic history paging against actual Slint geometry, with synthetic data.
use super::*;

pub(crate) fn check_ui() {
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let accept = Rc::new(Cell::new(true));
    ui.on_load_older({
        let requests = requests.clone();
        let accept = accept.clone();
        move |chat, request| {
            requests.borrow_mut().push(domain::HistoryReply {
                chat,
                request,
                success: true,
            });
            accept.get()
        }
    });
    let mut demo = crate::fixtures::demo();
    demo.select(6);
    let mut view = demo.conversation();
    let publish = |view: &domain::Conversation, reset| {
        crate::presentation::conversation(&ui, view, reset);
        layout(&ui);
    };
    let scroll = |offset: f32| {
        ui.set_timeline_y(-offset);
        layout(&ui);
    };
    let anchor = || {
        let mut result = (0, 0.);
        with(&ui, |model| {
            let state = model.state.borrow();
            let (index, within) = state.anchor();
            result = (
                state.source[index].id,
                within - state.cache[index].prefix.unwrap(),
            );
        });
        result
    };
    publish(&view, true);
    assert!(
        requests.borrow().is_empty(),
        "opening a long chat starts at the end"
    );
    scroll(81.);
    assert!(requests.borrow().is_empty());
    scroll(79.);
    assert_eq!(requests.borrow().len(), 1);
    let first = requests.borrow()[0];
    let before = anchor();
    for _ in 0..20 {
        publish(&view, false);
    }
    assert_eq!(
        requests.borrow().len(),
        1,
        "updates cannot duplicate an in-flight page"
    );
    demo.load_older(6);
    view = demo.conversation();
    publish(&view, false);
    history_finished(&ui, first);
    let after = anchor();
    assert_eq!(after.0, before.0);
    assert!(
        (after.1 - before.1).abs() < 0.2,
        "prepend must preserve the viewed content"
    );
    assert_eq!(ui.get_messages().row_count(), 200);
    assert_eq!(requests.borrow().len(), 1);
    assert!(
        ui.get_timeline_rows().row_count() < 30,
        "history stays virtualized"
    );

    scroll(0.);
    let failed = requests.borrow()[1];
    history_finished(
        &ui,
        domain::HistoryReply {
            success: false,
            ..failed
        },
    );
    for _ in 0..20 {
        layout(&ui);
    }
    assert_eq!(
        requests.borrow().len(),
        2,
        "failure must not create a retry loop"
    );
    scroll(200.);
    scroll(0.);
    assert_eq!(
        requests.borrow().len(),
        3,
        "a fresh approach retries a failed page"
    );
    let pending = requests.borrow()[2];
    history_finished(&ui, failed);
    layout(&ui);
    assert_eq!(
        requests.borrow().len(),
        3,
        "stale replies cannot release a newer request"
    );

    demo.select(1);
    publish(&demo.conversation(), true);
    history_finished(&ui, pending);
    demo.select(6);
    view = demo.conversation();
    publish(&view, true);
    scroll(0.);
    assert_eq!(requests.borrow().len(), 4);
    let current = requests.borrow()[3];
    history_finished(&ui, pending);
    layout(&ui);
    assert_eq!(
        requests.borrow().len(),
        4,
        "a stale reply after reopening the same chat is ignored"
    );
    ui.window().set_minimized(true);
    history_finished(&ui, current);
    layout(&ui);
    assert_eq!(
        requests.borrow().len(),
        4,
        "minimized chats cannot request pages"
    );
    ui.window().set_minimized(false);
    layout(&ui);
    assert_eq!(requests.borrow().len(), 5);

    // Queue rejection follows the same bounded retry behavior as archive errors.
    let last = requests.borrow()[4];
    scroll(200.);
    history_finished(&ui, last);
    accept.set(false);
    scroll(0.);
    for _ in 0..20 {
        layout(&ui);
    }
    assert_eq!(requests.borrow().len(), 6);
    accept.set(true);
    scroll(200.);
    scroll(0.);
    assert_eq!(requests.borrow().len(), 7);

    // Fitting pages fill automatically, including while offline; reaching the
    // archive's end stops without a button, timer or another request.
    let mut short = view.clone();
    short.messages = view.messages[..2].to_vec().into();
    for message in Arc::make_mut(&mut short.messages) {
        message.text = "Short synthetic message 你好 🙂".into();
        message.date = None;
    }
    ui.set_demo_mode(false);
    ui.set_live_chat_view(true);
    ui.set_live_shown_revision(ui.get_live_revision());
    ui.set_connection_state(8);
    publish(&short, true);
    assert_eq!(
        requests.borrow().len(),
        8,
        "short cached pages fill without a connection"
    );
    let filling = requests.borrow()[7];
    short.older_count = 0;
    publish(&short, false);
    history_finished(&ui, filling);
    for _ in 0..20 {
        layout(&ui);
    }
    assert_eq!(requests.borrow().len(), 8);
    // Cached history ends; connected phone availability can now supply a page.
    ui.set_connection_state(5);
    ui.set_phone_history_available(true);
    layout(&ui);
    assert_eq!(requests.borrow().len(), 9);
    let phone = requests.borrow()[8];
    ui.set_phone_history_available(false);
    ui.set_phone_history_loading(true);
    for _ in 0..20 {
        layout(&ui);
    }
    assert_eq!(requests.borrow().len(), 9, "one pending phone request only");
    for width in [520., 800., 1200.] {
        ui.window().set_size(slint::LogicalSize::new(width, 800.));
        layout(&ui);
        assert!(ui.get_timeline_rows().row_count() < 30);
    }
    ui.set_phone_history_loading(false);
    history_finished(
        &ui,
        domain::HistoryReply {
            success: false,
            ..phone
        },
    );
    ui.set_phone_history_available(true);
    for _ in 0..20 {
        layout(&ui);
    }
    assert_eq!(
        requests.borrow().len(),
        9,
        "a silent phone cannot cause a layout retry loop"
    );
    ui.hide().unwrap();
}
