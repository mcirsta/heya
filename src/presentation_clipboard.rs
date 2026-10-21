//! Window-level clipboard work; the timeline keeps no clipboard state or workers.
#[cfg(test)]
#[path = "presentation_clipboard_tests.rs"]
pub(crate) mod tests;
use crate::{
    AppWindow,
    archive::Archive,
    attachments::{self, Command as Upload, Destination, Error},
    clipboard::{self, Content},
    downloads::Control,
};
use slint::{ComponentHandle, Model};
use std::path::PathBuf;
use tokio::sync::{mpsc, watch};

#[derive(Clone)]
struct Target {
    chat: i32,
    title: String,
}
struct Paste {
    target: Target,
    draft: String,
    anchor: i32,
    cursor: i32,
    generation: i32,
}
enum Command {
    Paste(Paste),
    Drop(Target, Vec<PathBuf>),
    Copy(i32, i32),
}
fn target(ui: &AppWindow) -> Target {
    Target {
        chat: ui.get_selected_id(),
        title: ui.get_chat_title().into(),
    }
}
fn send(ui: &AppWindow, tx: &mpsc::Sender<Command>, command: Command) {
    if ui.get_clipboard_busy() {
        ui.set_notice("The clipboard is busy. Please try again.".into());
        return;
    }
    if tx.try_send(command).is_ok() {
        ui.set_clipboard_busy(true);
    } else {
        ui.set_notice("Clipboard actions are unavailable. Please try again.".into());
    }
}
fn bind(ui: &AppWindow, tx: mpsc::Sender<Command>) {
    ui.set_clipboard_enabled(true);
    let weak = ui.as_weak();
    let paste = tx.clone();
    ui.on_paste_composer(move |anchor, cursor| {
        let Some(ui) = weak.upgrade() else { return };
        send(
            &ui,
            &paste,
            Command::Paste(Paste {
                target: target(&ui),
                draft: ui.get_draft().into(),
                anchor,
                cursor,
                generation: ui.get_paste_generation(),
            }),
        );
    });
    // Slint/winit delivers one completed native transfer, including all paths.
    // These paths are already in memory; file I/O stays in the import worker.
    ui.on_accepts_drop(|data| data.has_file_paths());
    let weak = ui.as_weak();
    let drops = tx.clone();
    ui.on_drop_files(move |data| {
        let Some(ui) = weak.upgrade() else { return };
        if !ui.invoke_can_import() || ui.get_clipboard_busy() {
            return;
        }
        if let Ok(paths) = data.file_paths() {
            let paths = paths
                .take(attachments::MAX_FILES + 1)
                .map(PathBuf::from)
                .collect();
            send(&ui, &drops, Command::Drop(target(&ui), paths));
        }
    });
    let weak = ui.as_weak();
    ui.on_copy_photo(move |id| {
        let Some(ui) = weak.upgrade() else { return };
        let viewing = ui.get_photo_viewer_id() == id && ui.get_photo_viewer_ready();
        if !viewing
            && !ui.get_timeline_rows().iter().any(|r| {
                r.message.id == id
                    && r.message.actionable
                    && r.message.has_photo
                    && r.message.photo_state == 2
            })
        {
            return;
        }
        send(&ui, &tx, Command::Copy(ui.get_selected_id(), id));
    });
}
async fn destination(
    archive: Option<Archive>,
    target: Target,
    demo: bool,
) -> Result<Destination, Error> {
    if demo {
        return Ok(Destination {
            chat: target.chat,
            title: target.title,
            reply: None,
            owner: String::new(),
        });
    }
    let mut destination = archive
        .ok_or(Error::Archive)?
        .attachment_destination(target.chat)
        .await
        .map_err(|_| Error::Archive)?;
    destination.title = target.title;
    Ok(destination)
}
pub fn attach(
    ui: &AppWindow,
    archives: watch::Receiver<Option<Archive>>,
    uploads: mpsc::Sender<Upload>,
    demo: bool,
) -> impl std::future::Future<Output = ()> + Send + use<> {
    let (tx, mut rx) = mpsc::channel(1);
    bind(ui, tx);
    let weak = ui.as_weak();
    async move {
        // Retain the current native offer; clipboard serving is owned by the
        // platform library, not by a parked Heya preparation thread.
        let mut _clipboard_owner = None;
        while let Some(command) = rx.recv().await {
            let archive = archives.borrow().clone();
            let control = Control::default();
            let result: Result<Option<PasteResult>, Error> = async {
                match command {
                    Command::Paste(paste) => {
                        // Capture the quote and recipient before reading an external owner.
                        let destination = destination(archive, paste.target.clone(), demo).await;
                        let content =
                            crate::attachment_worker::blocking(control, clipboard::read).await?;
                        match content {
                            Content::Text(text) => Ok(Some(PasteResult::Text(paste, text))),
                            Content::Attachments(input) => {
                                Ok(Some(PasteResult::Import(destination?, input)))
                            }
                        }
                    }
                    Command::Drop(target, paths) => {
                        let destination = destination(archive, target, demo).await?;
                        Ok(Some(PasteResult::Import(
                            destination,
                            clipboard::paths_input(paths)?,
                        )))
                    }
                    Command::Copy(chat, id) => {
                        let original = if demo {
                            None
                        } else {
                            Some(
                                archive
                                    .ok_or(Error::Archive)?
                                    .clipboard_photo(chat, id)
                                    .await
                                    .map_err(|_| Error::Archive)?
                                    .ok_or(Error::PhotoUnavailable)?,
                            )
                        };
                        _clipboard_owner = Some(
                            crate::attachment_worker::blocking(control, move || {
                                let pixels = match original {
                                    Some(bytes) => crate::photos::decode(&bytes, u32::MAX)
                                        .ok_or(Error::PhotoUnavailable)?,
                                    None => crate::fixtures::photo_pixels(0, 1024),
                                };
                                clipboard::copy(pixels)
                            })
                            .await?,
                        );
                        Ok(None)
                    }
                }
            }
            .await;
            let uploads = uploads.clone();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_clipboard_busy(false);
                match result {
                    Ok(Some(PasteResult::Text(paste,text))) => insert_text(&ui,paste,&text),
                    Ok(Some(PasteResult::Import(destination,input))) => {
                        // Navigation may have changed; review keeps the captured recipient.
                        // Don't overwrite another selection or a voice recording.
                        if ui.get_attachment_phase()!=0 || ui.get_voice_phase()!=0 {
                            ui.set_notice("Finish the current attachment or recording, then paste or drop again.".into());
                        } else {
                            let title=destination.title.clone();
                            if uploads.try_send(Upload::Import{destination,input}).is_ok() {
                                ui.set_attachment_title(title.into());
                                ui.set_attachment_phase(2); ui.set_attachment_review_open(true);
                            } else {
                                ui.set_notice("Attachment selection is unavailable. Please try again.".into());
                            }
                        }
                    }
                    Ok(None) => ui.set_notice("Image copied".into()),
                    Err(error) => ui.set_notice(error.label().into()),
                }
            });
        }
    }
}
enum PasteResult {
    Text(Paste, String),
    Import(Destination, attachments::Input),
}
fn insert_text(ui: &AppWindow, paste: Paste, text: &str) {
    if ui.get_selected_id() != paste.target.chat
        || ui.get_paste_generation() != paste.generation
        || ui.get_draft().as_str() != paste.draft
        || !ui.get_composer_focused()
        || ui.get_composer_anchor() != paste.anchor
        || ui.get_composer_cursor() != paste.cursor
    {
        ui.set_notice("The editor changed while reading the clipboard. Please paste again.".into());
        return;
    }
    let (a, b) = (
        paste.anchor.min(paste.cursor) as usize,
        paste.anchor.max(paste.cursor) as usize,
    );
    if b > paste.draft.len()
        || !paste.draft.is_char_boundary(a)
        || !paste.draft.is_char_boundary(b)
        || paste.draft.len() - (b - a) + text.len() > 64 * 1024
    {
        ui.set_notice("The pasted text exceeds the message size limit.".into());
        return;
    }
    // Slint has no public insert-text operation. A composition commit uses
    // its native selection/undo implementation and cannot invoke Enter-to-send.
    use i_slint_core::input::{InternalKeyEvent, KeyEvent, KeyEventType};
    let mut key_event = KeyEvent::default();
    key_event.text = text.into();
    ui.window()
        .dispatch_event(slint::platform::WindowEvent::internal(InternalKeyEvent {
            key_event,
            event_type: KeyEventType::CommitComposition,
            ..Default::default()
        }));
}
