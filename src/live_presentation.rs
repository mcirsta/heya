//! Presentation of the live adapter's application-owned snapshots.
use crate::{
    AppWindow,
    connection::{Failure, Phase, Request, Snapshot},
    messaging::{self, Command},
    presentation, protocol,
};
use slint::{ComponentHandle, Model};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, watch},
    task::JoinHandle,
};

// A busy UI (for example during a resize) needs one pending callback per
// snapshot stream, not a callback for every history/receipt update. The callback
// reads the newest value; no account content is copied into the event queue.
pub(crate) async fn present_latest<T: Clone + Send + Sync + 'static>(
    weak: slint::Weak<AppWindow>,
    mut snapshots: watch::Receiver<T>,
    apply: impl Fn(&AppWindow, &T) + Send + Sync + 'static,
) {
    let pending = Arc::new(AtomicBool::new(false));
    let apply = Arc::new(apply);
    while snapshots.changed().await.is_ok() {
        if pending.swap(true, Ordering::AcqRel) {
            continue;
        }
        let latest = snapshots.clone();
        let pending = pending.clone();
        let apply = apply.clone();
        if weak
            .upgrade_in_event_loop(move |ui| {
                // Clear before reading: updates arriving during/after this read
                // must be able to schedule the next callback.
                pending.store(false, Ordering::Release);
                // Views and QR buffers are shared immutable data. Release the
                // watch lock before updating Slint so producers never wait for
                // UI layout while publishing the next snapshot.
                let snapshot = latest.borrow().clone();
                apply(&ui, &snapshot);
            })
            .is_err()
        {
            break;
        }
    }
}

pub(crate) fn publish(ui: &AppWindow, snapshot: &Snapshot) {
    let state = match snapshot.phase {
        Phase::Idle => 0,
        Phase::Opening => 1,
        Phase::Connecting => 2,
        Phase::Qr => 3,
        Phase::Pairing => 4,
        Phase::Connected => 5,
        Phase::Reconnecting => 6,
        Phase::Stopping => 7,
        Phase::Stopped => 8,
        Phase::Failed(_) => 9,
    };
    if ui.get_connection_state() != state {
        // Only fixed lifecycle names: never credentials, JIDs, message text or QR data.
        eprintln!("Heya connection: {:?}", snapshot.phase);
    }
    ui.set_connection_can_relink(matches!(
        snapshot.phase,
        Phase::Failed(Failure::LoggedOut | Failure::ArchiveIdentity)
    ));
    ui.set_connection_state(state);
    if state != 5 {
        crate::presentation_presence::clear(ui);
    }
    ui.set_connection_error(match snapshot.phase {
        Phase::Failed(failure) => failure.message().into(),
        _ => "".into(),
    });
    ui.set_connection_blocked(matches!(
        snapshot.phase,
        Phase::Failed(
            Failure::Shutdown | Failure::LoggedOut | Failure::Outdated | Failure::ArchiveIdentity
        )
    ));
    if ui.get_archive_ready()
        && (snapshot.phase == Phase::Connected || ui.get_chats().row_count() > 0)
    {
        ui.set_live_chat_view(true);
    }
    if matches!(
        snapshot.phase,
        Phase::Qr | Phase::Pairing | Phase::Failed(Failure::ArchiveIdentity)
    ) {
        ui.set_live_chat_view(false);
    }
    ui.set_qr_seconds(snapshot.seconds_left as i32);
    ui.set_pairing_qr(
        snapshot
            .qr
            .as_ref()
            .map_or_else(slint::Image::default, |qr| {
                let pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    &qr.pixels, qr.width, qr.width,
                );
                slint::Image::from_rgba8(pixels)
            }),
    );
}

pub(crate) fn bind(ui: &AppWindow, requests: watch::Sender<Request>) {
    ui.on_connect_account({
        let requests = requests.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade()
                && !ui.get_connection_blocked()
                && !matches!(ui.get_connection_state(), 1..=7)
            {
                requests.send_replace(Request::Connect);
                publish(&ui, &Snapshot::new(Phase::Opening));
            }
        }
    });
    ui.on_relink_account({
        let requests = requests.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade()
                && ui.get_connection_can_relink()
            {
                requests.send_replace(Request::Relink);
                ui.set_live_chat_view(false);
                publish(&ui, &Snapshot::new(Phase::Opening));
            }
        }
    });
    ui.on_logout_account({
        let requests = requests.clone();
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade()
                && ui.get_connection_state() == 5
            {
                requests.send_replace(Request::Logout);
                publish(&ui, &Snapshot::new(Phase::Stopping));
            }
        }
    });
    let weak = ui.as_weak();
    ui.on_disconnect_account(move || {
        requests.send_replace(Request::Disconnect);
        if let Some(ui) = weak.upgrade() {
            publish(&ui, &Snapshot::new(Phase::Stopping));
        }
    });
}

pub fn attach(
    ui: &AppWindow,
    runtime: &Runtime,
    offline: bool,
    preferences: watch::Receiver<crate::preferences::Preferences>,
) -> JoinHandle<()> {
    let (requests, receiver) = watch::channel(Request::Idle);
    requests.send_replace(if offline {
        Request::OpenOffline
    } else {
        Request::Startup
    });
    let desired = receiver.clone();
    let (updates, snapshots) = watch::channel(Snapshot::new(Phase::Idle));
    publish(ui, &snapshots.borrow());
    bind(ui, requests);
    ui.set_selected_id(0);
    ui.set_chat_title("Select a conversation".into());
    crate::presentation_presence::clear(ui);
    let (archives, archive_rx) = watch::channel(None);
    let (upload_sources, upload_rx) = watch::channel(None);
    let (upload_commands, uploads) =
        crate::presentation_attachments::attach(ui, archive_rx.clone(), upload_rx.clone(), false);
    let voice = crate::presentation_voice::attach(
        ui,
        archive_rx.clone(),
        upload_rx,
        upload_commands,
        false,
        preferences.clone(),
    );
    let (download_sources, download_rx) = watch::channel(None);
    let downloads = crate::presentation_downloads::attach(
        ui,
        archive_rx.clone(),
        download_rx,
        preferences.clone(),
    );
    let (history_sources, history_source_rx) = watch::channel(None);
    let (audio_sources, audio_source_rx) = watch::channel(None);
    let audio_task = crate::presentation_audio::attach(
        ui,
        archive_rx.clone(),
        audio_source_rx,
        false,
        preferences.clone(),
    );
    let (presence_updates, presence_snapshots) =
        watch::channel(crate::presence::Snapshot::default());
    let presence_selected = crate::presentation_presence::bind(ui, presence_snapshots.clone());
    let presence_weak = ui.as_weak();
    let avatar_demand = crate::presentation_avatars::bind(ui);
    let avatar_archive = archive_rx.clone();
    let message_image_archive = archive_rx.clone();
    let photo_archive = archive_rx.clone();
    let photo_demand = crate::presentation_photos::bind(ui);
    let photo_fetch_demand = photo_demand.clone();
    let (photo_updates, photo_snapshots) =
        watch::channel(Arc::new(crate::photos::Snapshot::default()));
    let photo_weak = ui.as_weak();
    let message_image_demand = crate::presentation_content::bind_images(ui);
    let (message_image_updates, message_image_snapshots) =
        watch::channel(crate::message_thumbnails::Snapshot::default());
    let image_weak = ui.as_weak();
    let (avatar_updates, avatar_snapshots) = watch::channel(crate::avatars::Snapshot::default());
    let (commands, command_rx) = mpsc::channel(256);
    let (views, view_rx) = watch::channel(messaging::Snapshot::empty());
    bind_messages(ui, commands);
    let weak = ui.as_weak();
    let message_weak = weak.clone();
    let avatar_weak = weak.clone();
    runtime.spawn(async move {
        let presentation = present_latest(weak, snapshots, move |ui, snapshot| {
            if matches!(*desired.borrow(), Request::Disconnect | Request::Logout)
                && matches!(
                    snapshot.phase,
                    Phase::Opening
                        | Phase::Connecting
                        | Phase::Qr
                        | Phase::Pairing
                        | Phase::Connected
                        | Phase::Reconnecting
                )
            {
                publish(ui, &Snapshot::new(Phase::Stopping));
            } else {
                publish(ui, snapshot);
            }
        });
        let message_presentation = present_latest(message_weak, view_rx, publish_messages);
        let avatar_presentation = present_latest(
            avatar_weak,
            avatar_snapshots,
            crate::presentation_avatars::publish,
        );
        tokio::join!(
            protocol::run(
                receiver,
                updates,
                archives,
                protocol::LiveServices {
                    preferences: preferences.clone(),
                    avatar_demand: avatar_demand.clone(),
                    photo_demand: photo_fetch_demand,
                    thumbnail_demand: message_image_demand.clone(),
                    audio_sources,
                    history_sources,
                    download_sources,
                    upload_sources,
                    presence: crate::presence::Channels {
                        selected: presence_selected,
                        snapshots: presence_updates
                    },
                }
            ),
            present_latest(
                presence_weak,
                presence_snapshots,
                crate::presentation_presence::publish
            ),
            crate::photos::present(photo_archive, photo_demand, photo_updates),
            present_latest(
                photo_weak,
                photo_snapshots,
                crate::presentation_photos::publish
            ),
            messaging::run_with_preferences(
                archive_rx,
                command_rx,
                views,
                history_source_rx,
                preferences.clone()
            ),
            audio_task,
            downloads,
            uploads,
            voice,
            crate::avatars::present(avatar_archive, avatar_demand, avatar_updates),
            crate::message_thumbnails::present(
                message_image_archive,
                message_image_demand,
                message_image_updates
            ),
            present_latest(
                image_weak,
                message_image_snapshots,
                crate::presentation_content::publish_images
            ),
            avatar_presentation,
            presentation,
            message_presentation
        );
    })
}

pub(crate) fn enqueue(ui: &AppWindow, commands: &mpsc::Sender<Command>, command: Command) -> bool {
    if commands.try_send(command).is_err() {
        ui.set_notice("Heya is busy. Your text is still here; please try again.".into());
        false
    } else {
        true
    }
}

pub(crate) fn bind_messages(ui: &AppWindow, commands: mpsc::Sender<Command>) {
    let timer = std::rc::Rc::new(slint::Timer::default());
    ui.on_read_visible({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move || {
            timer.stop();
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let message = ui.get_read_visible_message();
            if message <= 0 {
                return;
            }
            let (chat, revision) = (ui.get_selected_id(), ui.get_live_revision());
            let weak = weak.clone();
            let tx = tx.clone();
            timer.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(200),
                move || {
                    if let Some(ui) = weak.upgrade()
                        && ui.get_selected_id() == chat
                        && ui.get_live_revision() == revision
                        && ui.get_read_visible_message() == message
                    {
                        enqueue(
                            &ui,
                            &tx,
                            Command::ReadVisible {
                                chat,
                                message,
                                revision,
                            },
                        );
                    }
                },
            );
        }
    });
    crate::presentation_actions::bind(ui, {
        let tx = commands.clone();
        move |command| tx.try_send(Command::Action(command)).is_ok()
    });
    ui.on_select_chat({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |id| {
            if let Some(ui) = weak.upgrade() {
                let revision = ui.get_live_revision() + 1;
                if enqueue(
                    &ui,
                    &tx,
                    Command::Select {
                        id,
                        previous: ui.get_selected_id(),
                        draft: ui.get_draft().into(),
                        draft_version: ui.get_draft_revision(),
                        revision,
                    },
                ) {
                    ui.set_live_revision(revision);
                }
            }
        }
    });
    ui.on_draft_edited({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |text| {
            if let Some(ui) = weak.upgrade() {
                ui.set_draft_revision(ui.get_draft_revision() + 1);
                enqueue(
                    &ui,
                    &tx,
                    Command::Draft(ui.get_selected_id(), text.into(), ui.get_draft_revision()),
                );
            }
        }
    });
    crate::presentation_navigation::bind_live(ui, commands.clone());
    ui.on_search_chats({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                enqueue(&ui, &tx, Command::Search(query.into()));
            }
        }
    });
    ui.on_filter_unread({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |unread| {
            if let Some(ui) = weak.upgrade() {
                enqueue(&ui, &tx, Command::Unread(unread));
            }
        }
    });
    ui.on_fetch_phone_history({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                enqueue(
                    &ui,
                    &tx,
                    Command::FetchPhone {
                        chat: ui.get_selected_id(),
                        revision: ui.get_live_revision(),
                    },
                );
            }
        }
    });
    ui.on_load_older({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |chat, request| {
            weak.upgrade().is_some_and(|ui| {
                let revision = ui.get_live_revision();
                enqueue(
                    &ui,
                    &tx,
                    Command::Older {
                        chat,
                        request,
                        revision,
                    },
                )
            })
        }
    });
    ui.on_new_chat({
        let weak = ui.as_weak();
        let tx = commands.clone();
        move |phone| {
            if let Some(ui) = weak.upgrade() {
                let revision = ui.get_live_revision() + 1;
                if crate::domain::phone_number(&phone).is_none() {
                    ui.set_new_chat_error(
                        "Enter a valid phone number with its country code.".into(),
                    );
                    return;
                }
                if enqueue(
                    &ui,
                    &tx,
                    Command::NewChat {
                        phone: phone.into(),
                        previous: ui.get_selected_id(),
                        draft: ui.get_draft().into(),
                        draft_version: ui.get_draft_revision(),
                        revision,
                    },
                ) {
                    ui.set_live_revision(revision);
                    ui.set_show_conversation(true);
                    ui.set_new_chat_open(false);
                    ui.set_settings_open(false);
                } else {
                    ui.set_new_chat_error("Heya is busy. Please try again.".into());
                }
            }
        }
    });
    ui.on_send_message({
        let weak = ui.as_weak();
        move |chat, text| {
            if let Some(ui) = weak.upgrade() {
                if ui.get_connection_state() != 5
                    || ui.get_action_pending()
                    || !ui.get_live_can_send()
                    || ui.get_send_pending()
                    || ui.get_live_revision() != ui.get_live_shown_revision()
                    || text.trim().is_empty()
                {
                    return;
                }
                let request = ui.get_send_request_id() + 1;
                if enqueue(
                    &ui,
                    &commands,
                    Command::Send {
                        chat,
                        text: text.into(),
                        request,
                        draft_version: ui.get_draft_revision(),
                    },
                ) {
                    ui.set_send_request_id(request);
                    ui.set_send_draft_version(ui.get_draft_revision());
                    ui.set_send_pending(true);
                }
            }
        }
    });
}

pub(crate) fn publish_messages(ui: &AppWindow, snapshot: &messaging::Snapshot) {
    crate::presentation_actions::publish(ui, snapshot.operation.as_ref(), &snapshot.destinations);
    if let Some(reply) = &snapshot.reply
        && reply.request == ui.get_send_request_id()
        && ui.get_send_pending()
    {
        ui.set_send_pending(false);
        if reply.saved
            && reply.chat == ui.get_selected_id()
            && ui.get_draft() == reply.text
            && ui.get_draft_revision() == ui.get_send_draft_version()
        {
            ui.set_draft("".into());
        }
        if !reply.saved {
            ui.set_notice("The message could not be saved. Your draft has been kept.".into());
        }
    }
    if !snapshot.notice.is_empty() {
        ui.set_notice(snapshot.notice.clone().into());
    }
    if snapshot.revision != ui.get_live_revision() {
        return;
    }
    ui.set_phone_history_available(snapshot.phone.available);
    ui.set_phone_history_loading(snapshot.phone.loading);
    let Some(view) = &snapshot.view else {
        // A failed navigation retains the visible chat and its in-memory draft.
        // Release the navigation gate so editing/retry remains possible. Keep
        // the last chat's capability; the archive rechecks it on every send.
        ui.set_live_shown_revision(snapshot.revision);
        ui.set_navigation_shown(ui.get_navigation_request());
        ui.set_message_search_busy(false);
        if let Some(reply) = snapshot.history {
            presentation::timeline::history_finished(ui, reply);
        }
        return;
    };
    ui.set_archive_ready(true);
    if ui.get_connection_state() == 5
        || (!view.chats.is_empty()
            && !matches!(ui.get_connection_state(), 3 | 4)
            && !ui.get_connection_can_relink())
    {
        ui.set_live_chat_view(true);
    }
    ui.set_live_can_send(view.can_send);
    presentation::sidebar(ui, view.chats.clone());
    if view.history_failed {
        ui.set_notice(
            "Some history could not be imported. The encrypted chunks are retained for recovery."
                .into(),
        );
    }
    crate::presentation_navigation::prepare(ui, snapshot);
    if let Some(conversation) = &view.conversation {
        let restore = ui.get_live_shown_revision() != snapshot.revision
            || ui.get_selected_id() != conversation.chat.id;
        presentation::conversation(ui, conversation, restore);
    } else {
        ui.set_read_tail(0);
        ui.set_selected_id(0);
        ui.set_chat_title("Select a conversation".into());
        crate::presentation_presence::clear(ui);
        ui.set_messages(slint::ModelRc::default());
        ui.set_timeline_rows(slint::ModelRc::default());
        ui.set_timeline_height(0.);
        ui.set_timeline_y(0.);
        ui.set_draft("".into());
        ui.set_transcript("".into());
        ui.set_older_count(0);
    }
    ui.set_live_shown_revision(snapshot.revision);
    crate::presentation_navigation::publish(ui, snapshot);
    if let Some(reply) = snapshot.history {
        presentation::timeline::history_finished(ui, reply);
    }
    ui.invoke_request_avatars();
}
