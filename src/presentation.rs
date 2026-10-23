//! The sole boundary between Slint models and application data.
#[path = "presentation_benchmark.rs"]
pub(crate) mod benchmark;
#[cfg(test)]
#[path = "presentation_call_tests.rs"]
mod call_tests;
#[path = "presentation_chat_list.rs"]
pub(crate) mod chat_list;
#[cfg(test)]
#[path = "presentation_chat_management_tests.rs"]
mod chat_management_tests;
#[cfg(test)]
#[path = "presentation_media_tests.rs"]
mod media_tests;
#[cfg(test)]
#[path = "presentation_notice_tests.rs"]
mod notice_tests;
#[cfg(test)]
#[path = "presentation_send_tests.rs"]
mod send_tests;
#[path = "presentation_timeline.rs"]
pub(crate) mod timeline;
use crate::{
    AppWindow, ChatRow, MessageRow, domain, fixtures,
    service::{self, Command, Update},
};
#[cfg(test)]
use slint::VecModel;
use slint::{ComponentHandle, Model, ModelRc};
#[cfg(test)]
use std::time::Duration;
use std::{rc::Rc, sync::Arc};
use tokio::{runtime::Runtime, sync::mpsc, task::JoinHandle};

/// Decode the bundled tile on the startup worker, before the window is shown.
pub fn wallpaper_pixels() -> Result<image::RgbaImage, image::ImageError> {
    image::load_from_memory_with_format(
        include_bytes!("../ui/assets/wallpaper.png"),
        image::ImageFormat::Png,
    )
    .map(image::DynamicImage::into_rgba8)
}

fn chat_row(chat: &domain::ChatSummary) -> ChatRow {
    ChatRow {
        id: chat.id,
        title: domain::single_line_summary(&chat.title).into(),
        initials: chat.initials.as_str().into(),
        tint: slint::Color::from_rgb_u8(
            (chat.tint >> 16) as u8,
            (chat.tint >> 8) as u8,
            chat.tint as u8,
        ),
        preview: domain::single_line_summary(&chat.preview).into(),
        call_kind: chat.call_kind,
        time: chat.time.as_str().into(),
        unread: chat.unread.min(i32::MAX as usize) as i32,
        marked_unread: chat.marked_unread,
        muted: chat.muted,
        archived: chat.archived,
        avatar: slint::Image::default(),
    }
}

fn message_row(message: &domain::Message, previous: Option<&domain::Message>) -> MessageRow {
    let first_in_run = message.starts_run(previous);
    let sender_initials: String = message
        .sender
        .trim_start_matches('~')
        .split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .collect();
    let text = match message.content.notice {
        Some(notice) => notice.label(&domain::single_line_summary(&message.sender)),
        None => message
            .content
            .body
            .as_deref()
            .unwrap_or(&message.text)
            .to_owned(),
    };
    MessageRow {
        long_text: text.len() > 4096,
        large_emoji: crate::presentation_text::large_emoji(&text),
        wire_id: message.content.metadata.wire.as_str().into(),
        sent_at: message.content.metadata.stamp.to_string().into(),
        operation_busy: (message.outgoing
            && !matches!(
                message.content.metadata.delivery.as_str(),
                "accepted" | "delivered" | "read"
            ))
            || message.content.metadata.pending.ends_with("pending")
            || message.content.metadata.pending.ends_with("unconfirmed"),
        system_notice: message.content.notice.is_some(),
        call_kind: match message.content.notice {
            Some(crate::message_content::Notice::Call(c)) => {
                if c.video == Some(true) {
                    2
                } else {
                    1
                }
            }
            _ => 0,
        },
        call_details: match message.content.notice {
            Some(crate::message_content::Notice::Call(c)) => c.details(message.outgoing).into(),
            _ => Default::default(),
        },
        has_attachment: message.content.has_attachment
            || message.content.video.is_some()
            || message.content.photo.is_some()
            || message.content.document.is_some()
            || message.content.audio.is_some(),
        actionable: !message.content.unavailable && message.content.notice.is_none(),
        quote_sender: message
            .content
            .quote
            .as_ref()
            .map_or("", |q| q.sender.as_str())
            .into(),
        quote_text: message
            .content
            .quote
            .as_ref()
            .map_or("", |q| q.text.as_str())
            .into(),
        forwarded: message.content.forwarded,
        reactions: message.content.reaction_summary.as_str().into(),
        own_reaction: message.content.own_reaction.as_str().into(),
        reaction_status: message.content.reaction_status.as_str().into(),
        id: message.id,
        sender: message.sender.as_str().into(),
        sender_id: message.sender_id.as_str().into(),
        sender_avatar: Default::default(),
        sender_initials: sender_initials.into(),
        text: text.into(),
        time: message.time.as_str().into(),
        outgoing: message.outgoing,
        local: message.local,
        first_in_run,
        has_document: message.content.document.is_some(),
        document_name: message
            .content
            .document
            .as_ref()
            .map(|d| domain::single_line_summary(&d.file_name))
            .unwrap_or_default()
            .into(),
        document_type: message
            .content
            .document
            .as_ref()
            .map(|d| d.file_type())
            .unwrap_or_default()
            .into(),
        document_details: message
            .content
            .document
            .as_ref()
            .map(|d| d.details())
            .unwrap_or_default()
            .into(),
        has_audio: message.content.audio.is_some(),
        audio_waveform: message
            .content
            .audio
            .as_ref()
            .map(|a| crate::presentation_audio::waveform(&a.waveform))
            .unwrap_or_default()
            .into(),
        audio_duration: message
            .content
            .audio
            .as_ref()
            .and_then(|a| a.seconds)
            .map(|s| crate::presentation_audio::duration(s as f64))
            .unwrap_or_else(|| "Audio".into())
            .into(),
        has_photo: message.content.photo.is_some(),
        has_video: message.content.video.is_some(),
        video_duration: message
            .content
            .video
            .as_ref()
            .map(|v| crate::presentation_audio::duration(f64::from(v.seconds.unwrap_or(0))))
            .unwrap_or_default()
            .into(),
        photo_width: message.content.photo.as_ref().map_or_else(
            || {
                message
                    .content
                    .video
                    .as_ref()
                    .map_or(640., |v| v.width as f32)
            },
            |p| p.width as f32,
        ),
        photo_height: message.content.photo.as_ref().map_or_else(
            || {
                message
                    .content
                    .video
                    .as_ref()
                    .map_or(480., |v| v.height as f32)
            },
            |p| p.height as f32,
        ),
        photo_image: Default::default(),
        photo_state: 0,
        photo_status: crate::photos::State::Idle.label().into(),
        body_url: crate::message_content::web_url(
            message.content.body.as_deref().unwrap_or(&message.text),
        )
        .unwrap_or_default()
        .into(),
        preview_url: message
            .content
            .preview
            .as_ref()
            .map_or("", |p| p.url.as_str())
            .into(),
        preview_title: message
            .content
            .preview
            .as_ref()
            .map_or("", |p| p.title.as_str())
            .into(),
        preview_description: message
            .content
            .preview
            .as_ref()
            .map_or("", |p| p.description.as_str())
            .into(),
        preview_domain: message
            .content
            .preview
            .as_ref()
            .and_then(|p| url::Url::parse(&p.url).ok())
            .and_then(|u| u.host_str().map(str::to_owned))
            .unwrap_or_default()
            .into(),
        has_thumbnail: message
            .content
            .preview
            .as_ref()
            .is_some_and(|p| p.has_thumbnail)
            || message
                .content
                .document
                .as_ref()
                .is_some_and(|d| d.has_thumbnail)
            || message.content.video.is_some(),
        thumbnail: Default::default(),
        actions: ModelRc::new(slint::VecModel::from(
            message
                .content
                .actions
                .iter()
                .take(crate::message_content::MAX_ACTIONS)
                .map(|action| crate::MessageActionRow {
                    label: action.label.as_str().into(),
                    kind: match action.target {
                        crate::message_content::Target::Unavailable => 0,
                        crate::message_content::Target::Url(_) => 1,
                        crate::message_content::Target::Copy(_) => 2,
                    },
                })
                .collect::<Vec<_>>(),
        )),
    }
}

pub(crate) fn sidebar(ui: &AppWindow, chats: impl Into<Arc<[domain::ChatSummary]>>) {
    let model = ui.get_chats();
    if let Some(model) = model.as_any().downcast_ref::<chat_list::ChatListModel>() {
        model.update(chats.into());
    } else {
        ui.set_chats(ModelRc::new(chat_list::ChatListModel::new(chats.into())));
    }
}

pub(crate) fn conversation(
    ui: &AppWindow,
    view: &domain::Conversation,
    restore_draft: bool,
) -> bool {
    let layout_changed = ui.get_selected_id() != view.chat.id
        || ui.get_older_count() != view.older_count as i32
        || ui.get_chat_is_group() != view.is_group;
    let tint = slint::Color::from_rgb_u8(
        (view.chat.tint >> 16) as u8,
        (view.chat.tint >> 8) as u8,
        view.chat.tint as u8,
    );
    if restore_draft {
        ui.set_message_search_open(false);
        ui.set_message_search_query("".into());
        ui.set_browsing_history(false);
        ui.set_highlighted_message(0);
        ui.set_navigation_shown(ui.get_navigation_request());
        ui.set_message_search_busy(false);
        ui.set_message_search_hits(ModelRc::default());
    }
    ui.set_selected_id(view.chat.id);
    ui.set_read_needs_clear(view.chat.unread > 0 || view.chat.marked_unread);
    ui.set_reply_id(view.reply.as_ref().map_or(0, |r| r.id));
    ui.set_reply_sender(view.reply.as_ref().map_or("", |r| r.sender.as_str()).into());
    ui.set_reply_text(view.reply.as_ref().map_or("", |r| r.text.as_str()).into());
    ui.set_chat_title(view.chat.title.as_str().into());
    ui.set_chat_initials(view.chat.initials.as_str().into());
    ui.set_chat_tint(tint);
    crate::presentation_avatars::refresh(ui);
    crate::presentation_presence::conversation(ui, view.chat.id, view.is_group);
    ui.set_chat_is_group(view.is_group);
    ui.set_older_count(view.older_count as i32);
    let changed = timeline::update(ui, view, restore_draft);
    if ui.get_selection_active() {
        ui.invoke_selection_control(4);
    }
    ui.set_action_source_actionable(
        ui.get_timeline_rows()
            .iter()
            .any(|r| r.message.id == ui.get_action_message() && r.message.actionable),
    );
    if ui.get_message_dialog() != 0
        && (ui.get_action_chat() != view.chat.id
            || (ui.get_message_dialog() != 4
                && !ui.get_selection_active()
                && !ui.get_timeline_rows().iter().any(|r| {
                    r.message.id == ui.get_action_message()
                        && (r.message.actionable
                            || (ui.get_action_context_menu()
                                && matches!(ui.get_message_dialog(), 3 | 7 | 8)
                                && !r.message.system_notice))
                })))
    {
        ui.invoke_close_message_actions();
    }
    let changed = changed || layout_changed;
    if changed || restore_draft {
        ui.set_view_revision(ui.get_view_revision() + 1);
    }
    if restore_draft {
        ui.set_draft(view.draft.as_str().into());
    }
    if changed {
        // A late insertion can change the displayed messages without changing
        // the last message ID or private unread badge. Reuse the visibility gate.
        ui.invoke_read_visible();
    }
    changed
}

fn publish(ui: &AppWindow, update: Update) {
    match update {
        Update::Search(search) => {
            crate::presentation_navigation::publish(
                ui,
                &crate::messaging::Snapshot {
                    search,
                    ..crate::messaging::Snapshot::empty()
                },
            );
        }
        Update::Navigation { view, jump } => {
            let snapshot = crate::messaging::Snapshot {
                jump,
                ..crate::messaging::Snapshot::empty()
            };
            crate::presentation_navigation::prepare(ui, &snapshot);
            conversation(ui, &view, false);
            crate::presentation_navigation::publish(ui, &snapshot);
            if jump.message == 0 && !jump.returning {
                ui.set_notice("The original message is not available in saved history.".into());
            }
        }
        Update::Action {
            view,
            result,
            destinations,
        } => {
            conversation(ui, &view, false);
            crate::presentation_actions::publish(ui, Some(&result), &destinations);
        }
        Update::Sidebar(chats) => sidebar(ui, chats),
        Update::Conversation(view) => {
            conversation(ui, &view, true);
        }
        Update::History { view, reply } => {
            conversation(ui, &view, false);
            timeline::history_finished(ui, reply);
        }
        Update::Sent {
            chat,
            message,
            transcript,
        } => {
            if ui.get_selected_id() == chat {
                timeline::append(ui, message, transcript);
            }
        }
    }
}

fn enqueue(ui: &AppWindow, tx: &mpsc::Sender<Command>, command: Command) -> bool {
    if tx.try_send(command).is_err() {
        ui.set_notice("The demo is busy. Your text is still here; please try again.".into());
        false
    } else {
        true
    }
}

pub fn attach(
    ui: &AppWindow,
    runtime: &Runtime,
    preferences: tokio::sync::watch::Receiver<crate::preferences::Preferences>,
) -> JoinHandle<()> {
    let state = fixtures::demo();
    // Static, synthetic UI capture only; never attaches an account or a device.
    if let Some(phase) = std::env::var("HEYA_TEST_CALL_STATE")
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|p| matches!(p, 1..=6))
    {
        crate::presentation_calling::publish(
            ui,
            &crate::calling::Status {
                chat: 1,
                phase,
                can_mute: matches!(phase, 3..=5),
                title: "Alex Morgan".into(),
                seconds: 125,
                video: std::env::var_os("HEYA_TEST_CALL_VIDEO").is_some(),
                ..Default::default()
            },
        );
        if std::env::var_os("HEYA_TEST_CALL_VIDEO").is_some() {
            crate::presentation_calling::publish_video(
                ui,
                &crate::call_video::Frames {
                    local: Some(std::sync::Arc::new(fixtures::video_pixels(0.))),
                    remote: Some(std::sync::Arc::new(fixtures::video_pixels(1.5))),
                    camera_on: true,
                    peer_on: true,
                    local_number: 1,
                    remote_number: 1,
                },
            );
        }
    }
    let _ = crate::presentation_avatars::bind(ui);
    sidebar(ui, state.sidebar());
    conversation(ui, &state.conversation(), true);
    let mut avatars = (*fixtures::avatar_samples([0, 1, 2, 3])).clone();
    for chat in state.chats.iter().filter(|c| c.is_group) {
        avatars.extend(
            fixtures::group_avatar_samples(&chat.messages)
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
    }
    crate::presentation_avatars::publish(ui, &Arc::new(avatars));
    let image_demand = crate::presentation_content::bind_images(ui);
    let (image_updates, image_snapshots) =
        tokio::sync::watch::channel(crate::message_thumbnails::Snapshot::default());
    let image_weak = ui.as_weak();
    let (tx, rx) = mpsc::channel(128);
    crate::presentation_actions::bind(ui, {
        let tx = tx.clone();
        move |command| tx.try_send(Command::Action(command)).is_ok()
    });
    let tx = Rc::new(tx);
    crate::presentation_navigation::bind(ui, {
        let tx = tx.clone();
        move |ui, command| enqueue(ui, &tx, Command::Navigation(command))
    });

    ui.on_select_chat({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |id| {
            if let Some(ui) = weak.upgrade() {
                enqueue(
                    &ui,
                    &tx,
                    Command::Select {
                        id,
                        previous: ui.get_selected_id(),
                        draft: ui.get_draft().to_string(),
                    },
                );
            }
        }
    });
    ui.on_search_chats({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |query| {
            if let Some(ui) = weak.upgrade() {
                enqueue(&ui, &tx, Command::Search(query.into()));
            }
        }
    });
    ui.on_filter_unread({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |unread| {
            if let Some(ui) = weak.upgrade() {
                enqueue(&ui, &tx, Command::Unread(unread));
            }
        }
    });
    ui.on_send_message({
        let weak = ui.as_weak();
        let tx = tx.clone();
        move |id, text| {
            if text.trim().is_empty() {
                return;
            }
            if let Some(ui) = weak.upgrade()
                && enqueue(&ui, &tx, Command::Send(id, text.into()))
            {
                ui.set_draft("".into());
            }
        }
    });
    ui.on_load_older({
        let weak = ui.as_weak();
        move |chat, request| {
            weak.upgrade()
                .is_some_and(|ui| enqueue(&ui, &tx, Command::Older { chat, request }))
        }
    });
    let weak = ui.as_weak();
    let photo_demand = crate::presentation_photos::bind(ui);
    let (photo_updates, photo_snapshots) =
        tokio::sync::watch::channel(Arc::new(crate::photos::Snapshot::default()));
    let photo_weak = ui.as_weak();
    let (audio_archive, audio_archive_rx) = tokio::sync::watch::channel(None);
    let (audio_source, audio_source_rx) = tokio::sync::watch::channel(None);
    let video_task = crate::presentation_video::attach(
        ui,
        tokio::sync::watch::channel(None).1,
        tokio::sync::watch::channel(None).1,
        true,
        preferences.clone(),
    );
    let audio_task = crate::presentation_audio::attach(
        ui,
        audio_archive_rx,
        audio_source_rx,
        true,
        preferences.clone(),
    );
    let downloads = crate::presentation_downloads::attach(
        ui,
        tokio::sync::watch::channel(None).1,
        tokio::sync::watch::channel(None).1,
        preferences.clone(),
    );
    let (upload_commands, uploads) = crate::presentation_attachments::attach(
        ui,
        tokio::sync::watch::channel(None).1,
        tokio::sync::watch::channel(None).1,
        true,
    );
    let voice = crate::presentation_voice::attach(
        ui,
        tokio::sync::watch::channel(None).1,
        tokio::sync::watch::channel(None).1,
        upload_commands,
        true,
        preferences.clone(),
    );
    runtime.spawn(async move {
        let _audio_channels = (audio_archive, audio_source);
        tokio::join!(
            crate::message_thumbnails::demo_present(image_demand, image_updates),
            crate::live_presentation::present_latest(
                image_weak,
                image_snapshots,
                crate::presentation_content::publish_images
            ),
            audio_task,
            video_task,
            downloads,
            uploads,
            voice,
            service::run(state, rx, move |update| weak
                .upgrade_in_event_loop(move |ui| publish(&ui, update))
                .is_ok()),
            crate::photos::demo_present(photo_demand, photo_updates),
            crate::live_presentation::present_latest(
                photo_weak,
                photo_snapshots,
                crate::presentation_photos::publish
            ),
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_backend_testing::ElementHandle;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn sender_runs_follow_direction_and_group_participant() {
        let mut state = fixtures::demo();
        state.select(6);
        let mut previous = None;
        for (message, should_start) in state
            .conversation()
            .messages
            .iter()
            .zip([true, true, false, true, true, false])
        {
            let row = message_row(message, previous);
            assert_eq!(row.first_in_run, should_start);
            previous = Some(message);
        }
        state.select(2);
        let messages = state.conversation().messages;

        let noah = message_row(&messages[1], Some(&messages[0]));
        assert!(
            noah.first_in_run,
            "different incoming participants need a new run"
        );
        assert!(!message_row(&messages[1], Some(&messages[1])).first_in_run);
        let mut first = messages[0].clone();
        let mut second = messages[1].clone();
        first.sender = "Alex".into();
        second.sender = "Alex".into();
        assert!(
            message_row(&second, Some(&first)).first_in_run,
            "different identities with the same display name need separate headings"
        );
        second.sender_id = first.sender_id.clone();
        second.sender = "Alex renamed".into();
        assert!(
            !message_row(&second, Some(&first)).first_in_run,
            "renaming one participant must not split their run"
        );
    }

    fn check_composer_layout(ui: &AppWindow) {
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
        ui.show().unwrap();
        let composer = ElementHandle::find_by_element_type_name(ui, "Composer")
            .next()
            .unwrap();
        let editor = ElementHandle::find_by_accessible_label(ui, "Message draft")
            .next()
            .unwrap();
        let empty = composer.size().height;
        check_element_positions(ui, true);
        assert!(empty < 85., "single-line composer takes {empty}px");
        ui.set_draft("one\ntwo\nthree".into());
        let multiline = composer.size().height;
        check_element_positions(ui, true);
        assert!(multiline > empty + 15., "composer must grow with the draft");
        ui.set_draft("line\n".repeat(6).into());
        let capped = composer.size().height;
        ui.set_draft("line\n".repeat(50).into());
        assert!((composer.size().height - capped).abs() < 1.);
        check_element_positions(ui, true);
        assert!(
            editor.size().height > composer.size().height,
            "long drafts must scroll"
        );
        ui.set_draft("".into());
        assert!((composer.size().height - empty).abs() < 1.);
        for (width, height) in [(1200., 800.), (800., 600.), (620., 480.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            ui.set_show_conversation(true);
            assert!(editor.size().width > 150., "editor collapsed at {width}px");
            ui.set_draft(
                "A long wrapping sentence with several words. "
                    .repeat(12)
                    .into(),
            );
            assert!(composer.size().height <= capped + 1.);
            check_element_positions(ui, true);
        }
        element(ui, "Composer::dismiss-hint").invoke_accessible_default_action();
        check_element_positions(ui, false);
        ui.set_draft("".into());
        check_element_positions(ui, false);
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    }

    fn element(ui: &AppWindow, id: &str) -> ElementHandle {
        ElementHandle::find_by_element_id(ui, id)
            .next()
            .unwrap_or_else(|| panic!("missing layout element {id}"))
    }

    fn check_wallpaper_coverage() {
        let ui = AppWindow::new().unwrap();
        let pixels = wallpaper_pixels().unwrap();
        assert_eq!(pixels.dimensions(), (374, 666));
        ui.global::<crate::Theme>()
            .set_wallpaper_image(slint::Image::from_rgba8(
                slint::SharedPixelBuffer::clone_from_slice(pixels.as_raw(), 374, 666),
            ));
        ui.set_show_conversation(true);
        ui.show().unwrap();
        for live in [false, true] {
            ui.set_demo_mode(!live);
            ui.set_live_chat_view(live);
            crate::live_presentation::publish(
                &ui,
                &crate::connection::Snapshot::new(crate::connection::Phase::Reconnecting),
            );
            ui.set_notice(
                if live {
                    "Synthetic connection notice"
                } else {
                    ""
                }
                .into(),
            );
            for dark in [false, true] {
                ui.global::<crate::Theme>().set_dark(dark);
                for collapsed in [false, true] {
                    ui.set_sidebar_hidden(collapsed);
                    for (width, height) in [
                        (1200., 800.),
                        (520., 480.),
                        (800., 600.),
                        (1920., 1080.),
                        (3756., 2033.),
                        (620., 480.),
                        (1200., 800.),
                    ] {
                        ui.window().set_size(slint::LogicalSize::new(width, height));
                        let wallpaper = element(&ui, "AppWindow::wallpaper");
                        let conversation = element(&ui, "AppWindow::conversation");
                        let header = element(&ui, "AppWindow::conversation-header");
                        let texture = element(&ui, "Wallpaper::texture");
                        assert_inside(&wallpaper, &conversation);
                        assert_inside(&wallpaper, &texture);
                        assert!((wallpaper.size().width - conversation.size().width).abs() < 0.5);
                        assert!(
                            (wallpaper.absolute_position().y
                                - header.absolute_position().y
                                - header.size().height)
                                .abs()
                                < 0.5
                        );
                        assert!(
                            (wallpaper.absolute_position().y + wallpaper.size().height - height)
                                .abs()
                                < 0.5
                        );
                        // The reference's repeat phase stays anchored to the
                        // window when the sidebar, banner or viewport changes.
                        assert!(texture.absolute_position().x.abs() < 0.5);
                        assert!(texture.absolute_position().y.abs() < 0.5);
                        assert_eq!(
                            ElementHandle::find_by_element_id(&ui, "Wallpaper::texture").count(),
                            1
                        );
                    }
                }
            }
        }
        ui.global::<crate::Theme>().set_show_wallpaper(false);
        assert!(
            ElementHandle::find_by_element_id(&ui, "Wallpaper::texture")
                .next()
                .is_none()
        );
        ui.hide().unwrap();
    }

    fn check_sidebar_resize(ui: &AppWindow) {
        ui.set_show_conversation(false);
        let mut chats = fixtures::demo().sidebar();
        for chat in &mut chats {
            chat.title = "Synthetic group\nwith another line".into();
            chat.preview = "First 👩🏽‍💻\r\nSecond\u{2028}مرحبا\n".repeat(100);
            chat.time = "14 Oct 12:00".into();
        }
        sidebar(ui, chats);
        assert!(ui.get_chats().row_data(0).unwrap().preview.len() < 1024);
        assert!(!ui.get_chats().row_data(0).unwrap().preview.contains('\n'));
        // Also bypass the Rust projection: the component must defend its own
        // bounds even if a future caller provides a multiline label directly.
        let model = ModelRc::new(VecModel::from(ui.get_chats().iter().collect::<Vec<_>>()));
        ui.set_chats(model.clone());
        let mut row = model.row_data(1).unwrap();
        row.title = "Raw\nmultiline\ntitle".into();
        row.preview = "Raw preview\nwith many lines\n".repeat(100).into();
        model.set_row_data(1, row);
        for dark in [true, false] {
            ui.global::<crate::Theme>().set_dark(dark);
            for (width, height) in [
                (1200., 800.),
                (800., 600.),
                (620., 480.),
                (520., 480.),
                (1920., 1080.),
                (620., 480.),
                (1200., 800.),
            ] {
                ui.window().set_size(slint::LogicalSize::new(width, height));
                let rows: Vec<_> =
                    ElementHandle::find_by_element_id(ui, "AppWindow::chat-row").collect();
                assert!(rows.len() >= 2, "sidebar rows must be visible");
                for (index, row) in rows.iter().enumerate() {
                    assert!((row.size().height - 68.).abs() < 0.5);
                    for (id, height) in [
                        ("AppWindow::chat-title", 20.),
                        ("AppWindow::chat-preview", 18.),
                    ] {
                        let text = row.query_descendants().match_id(id).find_first();
                        if index < 2 {
                            assert!(text.is_some(), "first two rows must expose {id}");
                        }
                        if let Some(text) = text {
                            assert_inside(&text, row);
                            assert!(
                                text.size().height <= height + 0.5,
                                "{id} grew beyond one line"
                            );
                        }
                    }
                }
                for pair in rows.windows(2) {
                    assert!(
                        pair[0].absolute_position().y + 68. <= pair[1].absolute_position().y + 0.5
                    );
                }
            }
        }
        ui.global::<crate::Theme>().set_dark(true);
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    }

    fn check_element_positions(ui: &AppWindow, hint_visible: bool) {
        let composer = element(ui, "AppWindow::composer");
        let pill = element(ui, "Composer::pill");
        let composer_top = composer.absolute_position().y;
        let composer_bottom = composer_top + composer.size().height;
        let pill_top = pill.absolute_position().y;
        let pill_bottom = pill_top + pill.size().height;
        if hint_visible {
            let hint = element(ui, "Composer::shortcut-hint");
            let hint_top = hint.absolute_position().y;
            assert!(
                hint_top >= pill_bottom + 7.5,
                "shortcut hint overlaps composer: composer top={composer_top}, pill={pill_top}..{pill_bottom}, hint top={hint_top}"
            );
            assert!((hint_top + hint.size().height + 8. - composer_bottom).abs() < 1.);
            let dismiss = element(ui, "Composer::dismiss-hint");
            let label = element(ui, "Composer::hint-label");
            assert!(
                label.absolute_position().x
                    >= dismiss.absolute_position().x + dismiss.size().width + 7.5,
                "hint text must not cover its dismiss button"
            );
            assert_inside(&dismiss, &hint);
            assert_inside(&label, &hint);
        } else {
            assert!((pill_bottom + 8. - composer_bottom).abs() < 1.);
        }
        assert!(
            (pill_top - composer_top).abs() < 1.,
            "pill must start at composer top"
        );
        assert_inside(&pill, &composer);
        let field = element(ui, "Composer::field");
        let send = element(ui, "Composer::send-button");
        assert_inside(&field, &pill);
        assert_inside(&send, &pill);
        assert!(field.absolute_position().x + field.size().width <= send.absolute_position().x);

        let window = ui.window().size().to_logical(ui.window().scale_factor());
        assert!(
            (composer_bottom - window.height).abs() < 1.,
            "composer must end at the window bottom"
        );
        let conversation = element(ui, "AppWindow::conversation");
        let header = element(ui, "AppWindow::conversation-header");
        let timeline = element(ui, "AppWindow::timeline");
        let header_shadow = element(ui, "AppWindow::header-shadow");
        assert_inside(&header, &conversation);
        assert_inside(&composer, &conversation);
        assert_inside(&header_shadow, &conversation);
        assert!(
            (header_shadow.absolute_position().y
                - header.absolute_position().y
                - header.size().height)
                .abs()
                < 0.5
        );
        assert!(
            timeline.absolute_position().y
                >= header.absolute_position().y + header.size().height - 0.5
        );
        assert!(timeline.absolute_position().y + timeline.size().height <= composer_top + 0.5);
        if window.width >= 620. {
            let sidebar_shadow = element(ui, "AppWindow::sidebar-shadow");
            assert_inside(&sidebar_shadow, &conversation);
            assert!(
                (sidebar_shadow.absolute_position().x - conversation.absolute_position().x).abs()
                    < 0.5
            );
            assert!(
                (sidebar_shadow.absolute_position().y - header_shadow.absolute_position().y).abs()
                    < 0.5
            );
            let sidebar = element(ui, "AppWindow::sidebar");
            let sidebar_header = element(ui, "AppWindow::sidebar-header");
            assert!(
                sidebar.absolute_position().x.abs() < 0.5,
                "sidebar must be left anchored"
            );
            assert!(sidebar.absolute_position().y.abs() < 0.5);
            assert!((sidebar.size().width - conversation.absolute_position().x).abs() < 0.5);
            if (ui.get_demo_mode() || ui.get_connection_state() == 5) && ui.get_notice().is_empty()
            {
                assert!(
                    (sidebar_header.absolute_position().y - header.absolute_position().y).abs()
                        < 0.5
                );
            }
            assert_inside(&element(ui, "AppWindow::search"), &sidebar);
        } else {
            assert!(conversation.absolute_position().x.abs() < 0.5);
            assert!(
                ElementHandle::find_by_element_id(ui, "AppWindow::sidebar-shadow")
                    .next()
                    .is_none()
            );
        }
    }

    fn assert_inside(child: &ElementHandle, parent: &ElementHandle) {
        let c = child.absolute_position();
        let p = parent.absolute_position();
        let cs = child.size();
        let ps = parent.size();
        assert!(
            c.x >= p.x - 0.5
                && c.y >= p.y - 0.5
                && c.x + cs.width <= p.x + ps.width + 0.5
                && c.y + cs.height <= p.y + ps.height + 0.5,
            "{:?} at {c:?}/{cs:?} escapes {:?} at {p:?}/{ps:?}",
            child.id(),
            parent.id()
        );
    }

    fn check_message_positions(ui: &AppWindow) {
        let mut tails = 0;
        for message in ElementHandle::find_by_element_type_name(ui, "MessageBubble") {
            let bubble = message
                .query_descendants()
                .match_id("MessageBubble::bubble")
                .find_first()
                .unwrap();
            if let Some(tail) = message
                .query_descendants()
                .match_id("MessageBubble::tail")
                .find_first()
            {
                tails += 1;
                assert!(
                    (tail.absolute_position().y - bubble.absolute_position().y).abs() < 0.5,
                    "tail must leave the bubble's top corner"
                );
            }
        }
        assert!(tails > 0, "fixture must exercise bubble tails");
    }

    fn check_live_pairing_view(ui: &AppWindow) {
        use crate::{
            connection::{Failure, Phase, QrImage, Request, Snapshot},
            live_presentation,
        };
        let (commands, mut requested) = tokio::sync::watch::channel(Request::Idle);
        live_presentation::bind(ui, commands);
        ui.set_demo_mode(false);
        ui.window().set_size(slint::LogicalSize::new(1200., 800.));
        live_presentation::publish(ui, &Snapshot::new(Phase::Idle));
        ui.show().unwrap();
        assert_eq!(*requested.borrow(), Request::Idle);
        element(ui, "SessionView::action").invoke_accessible_default_action();
        assert_eq!(*requested.borrow_and_update(), Request::Connect);
        assert_eq!(ui.get_connection_state(), 1, "Connect responds before I/O");

        let pairing = Snapshot {
            phase: Phase::Qr,
            qr: Some(QrImage {
                width: 8,
                pixels: vec![255; 8 * 8 * 4].into(),
            }),
            seconds_left: 30,
        };
        live_presentation::publish(ui, &pairing);
        let card = element(ui, "SessionView::card");
        let status = element(ui, "SessionView::status");
        let qr = element(ui, "SessionView::qr-image");
        let action = element(ui, "SessionView::action");
        for dark in [true, false] {
            ui.global::<crate::Theme>().set_dark(dark);
            for (width, height) in [(1200., 800.), (800., 600.), (620., 480.), (520., 480.)] {
                ui.window().set_size(slint::LogicalSize::new(width, height));
                live_presentation::publish(ui, &pairing);
                assert!((card.size().width - 460.).abs() < 1.);
                assert!(card.absolute_position().x >= 12.);
                assert!(card.absolute_position().x + card.size().width <= width - 12.);
                assert!((qr.size().width - qr.size().height).abs() < 1.);
                assert!(
                    status.absolute_position().y + status.size().height <= qr.absolute_position().y
                );
                assert!(qr.absolute_position().y + qr.size().height < action.absolute_position().y);
                assert!(
                    action.absolute_position().y + action.size().height
                        < card.absolute_position().y + card.size().height - 28.
                );
            }
        }
        // Element queries omit clipped controls. At the minimum window size,
        // the user must be able to scroll the action into the viewport.
        element(ui, "SessionView::scroll").scroll(0., -1000.);
        assert!(action.absolute_position().y + action.size().height <= 480.);
        element(ui, "SessionView::action").invoke_accessible_default_action();
        assert_eq!(*requested.borrow_and_update(), Request::Disconnect);
        assert_eq!(ui.get_connection_state(), 7, "Cancel responds before I/O");
        assert_eq!(ui.get_pairing_qr().size().width, 0);
        live_presentation::publish(ui, &Snapshot::new(Phase::Connected));
        assert_eq!(
            ui.get_pairing_qr().size().width,
            0,
            "connected view must clear the credential image"
        );
        live_presentation::publish(ui, &Snapshot::new(Phase::Failed(Failure::Keyring)));
        element(ui, "SessionView::action").invoke_accessible_default_action();
        assert_eq!(*requested.borrow_and_update(), Request::Connect);
        live_presentation::publish(ui, &Snapshot::new(Phase::Failed(Failure::Shutdown)));
        element(ui, "SessionView::action").invoke_accessible_default_action();
        assert!(
            !requested.has_changed().unwrap(),
            "unsafe retries must be disabled"
        );
        ui.invoke_relink_account();
        assert!(
            !requested.has_changed().unwrap(),
            "shutdown failure cannot reset credentials"
        );
        live_presentation::publish(ui, &Snapshot::new(Phase::Failed(Failure::LoggedOut)));
        element(ui, "SessionView::action").invoke_accessible_default_action();
        assert_eq!(ui.get_account_dialog(), 1);
        assert!(
            !requested.has_changed().unwrap(),
            "opening confirmation does not relink"
        );
        element(ui, "AccountDialog::confirm").invoke_accessible_default_action();
        assert_eq!(*requested.borrow_and_update(), Request::Relink);
        assert_eq!(ui.get_account_dialog(), 0);
        live_presentation::publish(ui, &Snapshot::new(Phase::Connected));
        ui.set_account_dialog(2);
        element(ui, "AccountDialog::confirm").invoke_accessible_default_action();
        assert_eq!(*requested.borrow_and_update(), Request::Logout);
        assert_eq!(ui.get_connection_state(), 7);
        ui.global::<crate::Theme>().set_dark(true);
        ui.set_demo_mode(true);
    }

    fn check_live_message_handoff() -> AppWindow {
        use crate::{archive::View, live_presentation, messaging};
        let ui = AppWindow::new().unwrap();
        ui.set_demo_mode(false);
        let (tx, mut commands) = mpsc::channel(16);
        live_presentation::bind_messages(&ui, tx);
        let state = fixtures::demo();
        let mut conversation = state.conversation();
        conversation.messages = conversation.messages[..1].into();
        Arc::make_mut(&mut conversation.messages)[0].outgoing = true;
        Arc::make_mut(&mut conversation.messages)[0].time =
            "14 Oct 12:00 · Unconfirmed · check phone".into();
        let snapshot = messaging::Snapshot {
            search: Default::default(),
            jump: Default::default(),
            history: None,
            phone: Default::default(),
            chat_busy: false,
            operation: None,
            destinations: Arc::from([]),
            revision: 0,
            reply: None,
            notice: String::new(),
            view: Some(std::sync::Arc::new(View {
                unread_chats: 0,
                archived_count: 0,
                archived_view: false,
                chats: state.sidebar().into(),
                conversation: Some(conversation),
                can_send: true,
                history_failed: false,
                phone_more: false,
            })),
        };
        live_presentation::publish(
            &ui,
            &crate::connection::Snapshot::new(crate::connection::Phase::Connected),
        );
        live_presentation::publish_messages(&ui, &snapshot);
        let layout_revision = ui.get_view_revision();
        live_presentation::publish_messages(&ui, &snapshot);
        assert_eq!(
            ui.get_view_revision(),
            layout_revision,
            "unchanged snapshots must not schedule timeline layout/scroll work"
        );
        ui.set_show_conversation(true);
        ui.show().unwrap();
        for (width, height) in [(1200., 800.), (800., 600.), (620., 480.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            check_element_positions(&ui, true);
            let bubble = element(&ui, "MessageBubble::bubble");
            let stamp = element(&ui, "MessageBubble::stamp");
            assert_inside(&stamp, &bubble);
        }
        ui.set_draft("Submitted text".into());
        ui.invoke_draft_edited(ui.get_draft());
        assert!(matches!(
            commands.try_recv().unwrap(),
            messaging::Command::Draft(1, _, 1)
        ));
        ui.invoke_send_message(1, ui.get_draft());
        let request = match commands.try_recv().unwrap() {
            messaging::Command::Send {
                chat,
                text,
                request,
                draft_version,
            } => {
                assert_eq!(chat, 1);
                assert_eq!(text, "Submitted text");
                assert_eq!(draft_version, 1);
                request
            }
            _ => panic!("expected captured send"),
        };
        assert!(ui.get_send_pending());
        assert_eq!(ui.get_draft(), "Submitted text");
        ui.invoke_send_message(1, ui.get_draft());
        assert!(
            commands.try_recv().is_err(),
            "a double click must not queue twice"
        );
        ui.set_draft("Newer text".into());
        ui.invoke_draft_edited(ui.get_draft());
        commands.try_recv().unwrap();
        let reply = messaging::SendReply {
            request,
            chat: 1,
            text: "Submitted text".into(),
            saved: true,
        };
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                search: Default::default(),
                jump: Default::default(),
                operation: None,
                destinations: Arc::from([]),
                reply: Some(reply),
                ..snapshot
            },
        );
        assert_eq!(ui.get_draft(), "Newer text");
        assert!(!ui.get_send_pending());
        ui.invoke_select_chat(2);
        assert!(matches!(
            commands.try_recv().unwrap(),
            messaging::Command::Select {
                id: 2,
                previous: 1,
                draft_version: 2,
                ..
            }
        ));
        ui.invoke_send_message(1, "should not send during selection".into());
        assert!(commands.try_recv().is_err());
        live_presentation::publish_messages(
            &ui,
            &messaging::Snapshot {
                search: Default::default(),
                jump: Default::default(),
                operation: None,
                destinations: Arc::from([]),
                revision: ui.get_live_revision(),
                view: None,
                history: None,
                phone: Default::default(),
                chat_busy: false,
                reply: None,
                notice: "Synthetic archive write failure".into(),
            },
        );
        assert_eq!(ui.get_live_revision(), ui.get_live_shown_revision());
        assert_eq!(ui.get_draft(), "Newer text");
        live_presentation::publish(
            &ui,
            &crate::connection::Snapshot::new(crate::connection::Phase::Reconnecting),
        );
        assert!(ui.get_live_chat_view(), "cached chats stay visible offline");
        for (width, height) in [(1200., 800.), (620., 480.), (520., 480.)] {
            ui.window().set_size(slint::LogicalSize::new(width, height));
            let banner = element(&ui, "AppWindow::connection-banner");
            let notice = element(&ui, "AppWindow::notice-bar");
            let content = element(&ui, "AppWindow::conversation-content");
            assert!(banner.size().height > 0.);
            assert!(
                notice.absolute_position().y
                    >= banner.absolute_position().y + banner.size().height - 0.5
            );
            assert!(
                content.absolute_position().y
                    >= notice.absolute_position().y + notice.size().height - 0.5
            );
            check_element_positions(&ui, true);
        }
        ui.set_new_chat_open(true);
        ui.invoke_new_chat("not a number".into());
        assert!(ui.get_new_chat_open());
        assert!(!ui.get_new_chat_error().is_empty());
        assert!(commands.try_recv().is_err());
        ui.set_new_chat_open(false);
        ui
    }

    #[test]
    fn real_ui_callbacks_drive_the_background_service() {
        i_slint_backend_testing::init_integration_test_with_system_time();
        let runtime = Runtime::new().unwrap();
        let live_ui = check_live_message_handoff();
        send_tests::check_ui();
        media_tests::check_ui();
        super::chat_management_tests::check_ui();
        crate::presentation_avatars::check_ui();
        crate::presentation_preferences::check_ui(&runtime);
        crate::presentation_storage::check_ui();
        crate::presentation_content::check_ui();
        super::notice_tests::check_ui();
        super::call_tests::check_ui();
        crate::presentation_photos::check_ui();
        crate::presentation_audio::check_ui();
        crate::presentation_calling::check_ui();
        crate::presentation_calling::workflow_tests::check_ui_flow(&runtime);
        crate::presentation_video::check_ui();
        crate::presentation_hover_tests::check_ui();
        crate::presentation_hover_tests::check_action_flows();
        crate::presentation_hover_tests::check_forward_chooser();
        crate::presentation_menu_tests::check_ui();
        crate::presentation_message_controls::tests::check_ui();
        crate::presentation_emoji_tests::check_ui();
        crate::presentation_text::check_ui();
        crate::presentation_downloads::check_ui();
        crate::presentation_attachments::check_ui();
        crate::presentation_clipboard::tests::check_ui();
        crate::presentation_voice::check_ui();
        crate::presentation_read_tests::check_ui();
        crate::presentation_navigation::check_ui();
        crate::presentation_presence::tests::check_ui();
        check_wallpaper_coverage();
        let ui = AppWindow::new().unwrap();
        check_live_pairing_view(&ui);
        check_composer_layout(&ui);
        check_sidebar_resize(&ui);
        timeline::tests::check_native_layout(&ui);
        timeline::history_tests::check_ui();
        // Deliberately hold the UI event loop while updates arrive. A resize or
        // minimize must not leave a backlog of callbacks applying the same view.
        let calls = Arc::new(AtomicUsize::new(0));
        let latest = Arc::new(AtomicUsize::new(0));
        let (updates, snapshots) = tokio::sync::watch::channel(0usize);
        let pump = runtime.spawn(crate::live_presentation::present_latest(
            ui.as_weak(),
            snapshots,
            {
                let calls = calls.clone();
                let latest = latest.clone();
                move |_, value| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    latest.store(*value, Ordering::SeqCst);
                }
            },
        ));
        runtime.block_on(async {
            for value in 1..=100 {
                updates.send_replace(value);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let task = attach(
            &ui,
            &runtime,
            tokio::sync::watch::channel(crate::preferences::Preferences::default()).1,
        );
        check_message_positions(&ui);
        let original_preview = ui.get_chats().row_data(0).unwrap().preview;
        ui.set_draft("A draft with 👩🏽‍💻\nand a second line".into());
        assert_eq!(
            ui.get_chats().row_data(0).unwrap().preview,
            original_preview
        );
        ui.invoke_select_chat(2);
        let phase = Rc::new(std::cell::Cell::new(0));
        let weak = ui.as_weak();
        let timer = slint::Timer::default();
        let tick = phase.clone();
        let check_calls = calls.clone();
        let check_latest = latest.clone();
        let later_update = updates.clone();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(10),
            move || {
                let ui = weak.upgrade().unwrap();
                match tick.get() {
                    0 if ui.get_selected_id() == 2
                        && check_latest.load(Ordering::SeqCst) == 100 =>
                    {
                        assert_eq!(
                            check_calls.load(Ordering::SeqCst),
                            1,
                            "a busy UI must receive one callback with the newest snapshot"
                        );
                        // Once that callback runs, the pump must accept another update.
                        later_update.send_replace(101);
                        assert_eq!(
                            ui.get_chats().row_data(0).unwrap().preview,
                            "Draft: A draft with 👩🏽‍💻 and a second line"
                        );
                        ui.invoke_select_chat(1);
                        tick.set(1);
                    }
                    1 if ui.get_selected_id() == 1 => {
                        assert_eq!(ui.get_draft(), "A draft with 👩🏽‍💻\nand a second line");
                        assert_eq!(
                            ui.get_chats().row_data(0).unwrap().preview,
                            original_preview
                        );
                        ui.invoke_send_message(1, ui.get_draft());
                        ui.set_draft("Keep this newer draft".into());
                        tick.set(2);
                    }
                    2 if ui.get_messages().row_count() == 7 => {
                        assert_eq!(ui.get_draft(), "Keep this newer draft");
                        assert!(ui.get_messages().row_data(6).unwrap().local);
                        ui.invoke_search_chats("Rust".into());
                        tick.set(3);
                    }
                    3 if ui.get_chats().row_count() == 1
                        && check_latest.load(Ordering::SeqCst) == 101 =>
                    {
                        assert_eq!(ui.get_chats().row_data(0).unwrap().title, "Rust café");
                        tick.set(4);
                        slint::quit_event_loop().unwrap();
                    }
                    _ => {}
                }
            },
        );
        slint::Timer::single_shot(Duration::from_secs(5), || {
            slint::quit_event_loop().unwrap();
        });
        ui.run().unwrap();
        assert_eq!(
            phase.get(),
            4,
            "UI workflow timed out: selected={}, latest={}, calls={}",
            ui.get_selected_id(),
            latest.load(Ordering::SeqCst),
            calls.load(Ordering::SeqCst)
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the queue must coalesce and then rearm"
        );
        assert_eq!(
            latest.load(Ordering::SeqCst),
            101,
            "the final update must reach the UI"
        );
        timer.stop();
        drop(timer);
        drop(updates);
        drop(live_ui);
        drop(ui);
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), pump)
                .await
                .unwrap()
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap();
        });
    }
}
