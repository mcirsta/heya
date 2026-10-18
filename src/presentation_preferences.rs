//! The small settings view and its adapter; persistence runs on a worker.
use crate::{
    AppWindow, Theme, appearance,
    preferences::{self, Preferences, THEMES},
    ui::{SettingRow, SettingSection, ThemeColors},
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use tokio::sync::watch;

pub fn prepare(demo: bool) -> (Preferences, Option<PathBuf>, &'static str) {
    if demo {
        return (
            Preferences::default(),
            None,
            "Demo preferences last for this run only.",
        );
    }
    let Some(path) = preferences::path() else {
        return (
            Preferences::default(),
            None,
            "Settings are temporary: the configuration directory is unavailable.",
        );
    };
    match preferences::load(&path) {
        Ok(preferences) => (preferences, Some(path), ""),
        Err(_) => (
            Preferences::default(),
            None,
            "Your settings file could not be read. It has been left untouched; changes in this window are temporary.",
        ),
    }
}

pub fn attach(
    ui: &AppWindow,
    runtime: &tokio::runtime::Runtime,
    settings: Preferences,
    path: Option<PathBuf>,
    status: &str,
) -> (
    Option<tokio::task::JoinHandle<()>>,
    tokio::task::JoinHandle<()>,
    watch::Receiver<Preferences>,
) {
    let settings = Rc::new(RefCell::new(settings));
    apply(ui, &settings.borrow());
    refresh(ui, &settings.borrow());
    ui.set_settings_status(status.into());
    let (tx, mut rx) = watch::channel(settings.borrow().clone());
    let current = tx.subscribe();
    let (operations, operation_task) = crate::presentation_settings_io::attach(ui, runtime);
    {
        let tx = tx.clone();
        let settings = settings.clone();
        let weak = ui.as_weak();
        ui.on_download_folder_ready(move |folder| {
            let path = PathBuf::from(folder.as_str());
            if !path.is_absolute() {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let mut p = settings.borrow_mut();
            p.download_directory = path;
            refresh(&ui, &p);
            tx.send_replace(p.clone());
        });
    }
    {
        let settings = settings.clone();
        let weak = ui.as_weak();
        ui.on_audio_devices_ready(move || {
            if let Some(ui) = weak.upgrade() {
                refresh_audio(&ui, &settings.borrow());
            }
        });
    }
    let saver = path.map(|path| {
        let weak = ui.as_weak();
        runtime.spawn(async move {
            while rx.changed().await.is_ok() {
                let settings = rx.borrow_and_update().clone();
                let path = path.clone();
                let failed = !matches!(
                    crate::local_worker::run(move || preferences::save(&path, &settings)).await,
                    Ok(Ok(()))
                );
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    ui.set_settings_status(
                        if failed {
                            "Settings could not be saved. Your changes apply to this window only."
                        } else {
                            ""
                        }
                        .into(),
                    );
                });
            }
        })
    });
    {
        let settings = settings.clone();
        let weak = ui.as_weak();
        let operations = operations.clone();
        ui.on_preference_changed(move |key, value| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if key == "connection" {
                if matches!(ui.get_connection_state(), 1..=6) {
                    ui.invoke_disconnect_account();
                } else if ui.get_connection_state() != 7 && !ui.get_connection_blocked() {
                    ui.invoke_connect_account();
                    ui.set_settings_open(false);
                }
                return;
            }
            if key == "shortcuts" {
                ui.set_settings_query("shortcuts".into());
                refresh(&ui, &settings.borrow());
                return;
            }
            if key == "unlink" || key == "relink" {
                if !ui.get_demo_mode() && key == "unlink" && ui.get_connection_state() == 5 {
                    ui.set_account_dialog(2);
                } else if !ui.get_demo_mode() && key == "relink" && ui.get_connection_can_relink() {
                    ui.set_account_dialog(1);
                }
                return;
            }
            if [
                "download_folder",
                "default_download_folder",
                "refresh_audio",
            ]
            .contains(&key.as_str())
            {
                if ui.get_settings_io_busy() {
                    return;
                }
                let command = match key.as_str() {
                    "download_folder" => crate::presentation_settings_io::Command::Folder {
                        parent: crate::presentation_downloads::parent(&ui),
                        current: settings.borrow().download_directory.clone(),
                    },
                    "default_download_folder" => {
                        crate::presentation_settings_io::Command::DefaultFolder
                    }
                    _ => crate::presentation_settings_io::Command::Devices {
                        demo: ui.get_demo_mode(),
                    },
                };
                if operations.try_send(command).is_ok() {
                    ui.set_settings_io_busy(true);
                    refresh(&ui, &settings.borrow());
                }
                return;
            }
            let mut settings = settings.borrow_mut();
            let device = if key == "audio_input" || key == "audio_output" {
                audio_choices(
                    &ui,
                    if key == "audio_input" {
                        &settings.audio.input
                    } else {
                        &settings.audio.output
                    },
                    key == "audio_input",
                )
                .get(value as usize)
                .cloned()
            } else {
                None
            };
            let changed = if let Some(device) = device {
                if key == "audio_input" {
                    settings.audio.input = device;
                } else {
                    settings.audio.output = device;
                }
                true
            } else {
                settings.change(&key, value)
            };
            if changed {
                apply(&ui, &settings);
                if key != "input_volume" && key != "output_volume" {
                    refresh(&ui, &settings);
                }
                tx.send_replace(settings.clone());
            }
        });
    }
    {
        let settings = settings.clone();
        let weak = ui.as_weak();
        ui.on_search_settings(move |_| {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui, &settings.borrow());
            }
        });
    }
    let weak = ui.as_weak();
    ui.on_request_audio_devices(move || {
        if let Some(ui) = weak.upgrade()
            && !ui.get_audio_devices_loaded()
            && !ui.get_settings_io_busy()
            && crate::audio::SUPPORTED
            && operations
                .try_send(crate::presentation_settings_io::Command::Devices {
                    demo: ui.get_demo_mode(),
                })
                .is_ok()
        {
            ui.set_settings_io_busy(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_refresh_settings(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &settings.borrow());
        }
    });
    (saver, operation_task, current)
}

fn apply(ui: &AppWindow, preferences: &Preferences) {
    let theme = ui.global::<Theme>();
    theme.set_macos(cfg!(target_os = "macos"));
    theme.set_emoji_font(
        if cfg!(target_os = "windows") {
            "Segoe UI Emoji"
        } else if cfg!(target_os = "macos") {
            "Apple Color Emoji"
        } else {
            "Noto Color Emoji"
        }
        .into(),
    );
    let index = THEMES
        .iter()
        .position(|name| *name == preferences.theme)
        .unwrap_or(1);
    theme.set_follow_system(index == 0);
    theme.set_custom(index >= 3);
    theme.set_dark(index == 2);
    if let Some(palette) = index
        .checked_sub(3)
        .and_then(|i| appearance::palettes().get(i))
    {
        let c = |key: &str| slint::Color::from_argb_encoded(0xff00_0000 | palette.colors[key]);
        theme.set_dark(palette.dark);
        theme.set_colors(ThemeColors {
            window: c("window"),
            panel: c("panel"),
            canvas: c("chat"),
            surface: c("surface"),
            hover: c("surface_hover"),
            selected: c("surface_active"),
            outline: c("outline"),
            text: c("text"),
            secondary: c("secondary"),
            dim: c("dim"),
            accent: c("accent"),
            accent_hover: c("accent_hover"),
            on_accent: c("on_accent"),
            incoming: c("bubble_in"),
            outgoing: c("bubble_out"),
            outgoing_secondary: c("outgoing_secondary"),
            date: c("date"),
            wallpaper_ink: slint::Color::from_argb_encoded(palette.colors["wallpaper_ink"]),
        });
    }
    theme.set_font_family(
        if preferences.font == "System" {
            ""
        } else {
            "Inter"
        }
        .into(),
    );
    theme.set_show_wallpaper(preferences.show_wallpaper);
    ui.set_auto_download_photos(preferences.auto_download_photos);
    ui.set_enter_sends(preferences.enter_sends);
    ui.set_show_shortcut_hint(preferences.show_shortcut_hint);
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(rows))
}
fn row(key: &str, title: &str, description: &str, kind: i32) -> SettingRow {
    SettingRow {
        key: key.into(),
        title: title.into(),
        description: description.into(),
        kind,
        ..Default::default()
    }
}
fn pending(title: &str, description: &str) -> SettingRow {
    row("", title, description, 2)
}
fn info(title: &str, description: &str) -> SettingRow {
    row("", title, description, 3)
}
fn toggle(key: &str, title: &str, description: &str, value: bool) -> SettingRow {
    SettingRow {
        value: i32::from(value),
        ..row(key, title, description, 0)
    }
}
fn choice(
    key: &str,
    title: &str,
    description: &str,
    options: &[&str],
    selected: &str,
) -> SettingRow {
    SettingRow {
        value: options
            .iter()
            .position(|name| *name == selected)
            .unwrap_or(0) as i32,
        options: model(
            options
                .iter()
                .map(|name| SharedString::from(*name))
                .collect(),
        ),
        ..row(key, title, description, 1)
    }
}
fn audio_choices(
    ui: &AppWindow,
    selected: &crate::audio_devices::Selection,
    input: bool,
) -> Vec<crate::audio_devices::Selection> {
    use slint::Model;
    let mut values = vec![crate::audio_devices::Selection {
        id: String::new(),
        name: "System default".into(),
    }];
    values.extend(
        ui.get_audio_devices()
            .iter()
            .filter(|d| d.input == input)
            .map(|d| crate::audio_devices::Selection {
                id: d.id.to_string(),
                name: d.name.to_string(),
            }),
    );
    if !selected.id.is_empty() && !values.iter().any(|d| d.id == selected.id) {
        values.push(crate::audio_devices::Selection {
            id: selected.id.clone(),
            name: format!("{} (unavailable)", selected.name),
        });
    }
    values
}
fn audio_choice(
    ui: &AppWindow,
    key: &str,
    title: &str,
    p: &Preferences,
    input: bool,
) -> SettingRow {
    let selected = if input {
        &p.audio.input
    } else {
        &p.audio.output
    };
    let choices = audio_choices(ui, selected, input);
    let names = choices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>();
    let index = choices
        .iter()
        .position(|d| d.id == selected.id)
        .unwrap_or(0);
    choice(
        key,
        title,
        if input && ui.get_audio_devices_loaded() && choices.len() == 1 {
            "No microphones found. Connect one and refresh devices."
        } else {
            "Applies the next time audio starts."
        },
        &names,
        names[index],
    )
}
/// Discovery updates the two device rows in place. It must not destroy a theme
/// popup or an unrelated slider while the user is interacting with settings.
fn refresh_audio(ui: &AppWindow, p: &Preferences) {
    for section in ui
        .get_settings_sections()
        .iter()
        .filter(|s| s.title == "Audio")
    {
        if let Some(rows) = section.rows.as_any().downcast_ref::<VecModel<SettingRow>>() {
            for i in 0..rows.row_count() {
                if let Some(row) = rows.row_data(i) {
                    let replacement = match row.key.as_str() {
                        "audio_input" => {
                            Some(audio_choice(ui, "audio_input", "Microphone", p, true))
                        }
                        "audio_output" => {
                            Some(audio_choice(ui, "audio_output", "Speakers", p, false))
                        }
                        _ => None,
                    };
                    if let Some(replacement) = replacement {
                        rows.set_row_data(i, replacement);
                    }
                }
            }
        }
    }
}
fn refresh(ui: &AppWindow, p: &Preferences) {
    let connection = match ui.get_connection_state() {
        1 => "Opening encrypted archive…",
        2 => "Connecting…",
        3 => "Scan the QR code to link this computer.",
        4 => "Pairing…",
        5 => "Connected",
        6 => "Reconnecting…",
        7 => "Disconnecting…",
        9 => "Connection needs attention",
        _ => "Offline",
    };
    let sections = vec![
        (
            "Appearance",
            vec![
                choice("theme", "Theme", "", THEMES, &p.theme),
                choice(
                    "font",
                    "Font",
                    "System uses the toolkit's default interface font. Inter uses the installed Inter family.",
                    &["System", "Inter"],
                    &p.font,
                ),
                toggle(
                    "show_wallpaper",
                    "Add doodles",
                    "Show the default doodles behind conversations.",
                    p.show_wallpaper,
                ),
                pending(
                    "Wallpaper",
                    "Choose a wallpaper colour or image. The theme's chat colour is used for now.",
                ),
                pending("Zoom", "Scale the whole interface."),
                pending(
                    "Language",
                    "Translate the interface. English is currently available.",
                ),
                pending(
                    "Custom themes",
                    "Load additional palettes from the themes folder.",
                ),
            ],
        ),
        (
            "Chats",
            vec![
                toggle(
                    "enter_sends",
                    "Enter sends",
                    if cfg!(target_os = "macos") {
                        "When off, ⌘Enter sends. Shift+Enter always adds a line."
                    } else {
                        "When off, Ctrl+Enter sends. Shift+Enter always adds a line."
                    },
                    p.enter_sends,
                ),
                toggle(
                    "auto_download_photos",
                    "Download photos automatically",
                    "Download received photos when they come into view. Other attachments are not yet supported.",
                    p.auto_download_photos,
                ),
                pending(
                    "Pause other media while recording or playing",
                    "Pause and resume media in other apps.",
                ),
                pending(
                    "Keep chats archived",
                    "Control whether new messages bring archived chats back to the list.",
                ),
                pending(
                    "Locked chats code",
                    "Hide selected chats behind a local code.",
                ),
            ],
        ),
        (
            "Audio",
            if crate::audio::SUPPORTED {
                vec![
                    audio_choice(ui, "audio_input", "Microphone", p, true),
                    SettingRow {
                        value: p.audio.input_volume as i32,
                        ..row(
                            "input_volume",
                            "Recording volume",
                            "Level applied to new voice recordings.",
                            5,
                        )
                    },
                    audio_choice(ui, "audio_output", "Speakers", p, false),
                    SettingRow {
                        value: p.audio.output_volume as i32,
                        ..row(
                            "output_volume",
                            "Playback volume",
                            "Level applied when playback starts.",
                            5,
                        )
                    },
                    SettingRow {
                        action: "Refresh devices".into(),
                        ..row(
                            "refresh_audio",
                            "Audio devices",
                            "Refresh after connecting or removing an audio device.",
                            4,
                        )
                    },
                ]
            } else {
                vec![info(
                    "Audio disabled",
                    if cfg!(target_os = "macos") {
                        "Playback and recording are disabled in this build."
                    } else {
                        "This build was compiled without GStreamer audio support."
                    },
                )]
            },
        ),
        (
            "Notifications",
            vec![
                pending(
                    "Desktop notifications",
                    "Notify for chats you are not looking at; muted chats stay quiet.",
                ),
                pending("Message sound", "Choose a sound for new messages."),
                pending(
                    "Play sounds for group messages",
                    "Choose whether every message or only mentions and replies makes a sound.",
                ),
                pending(
                    "Mention sound",
                    "Choose a sound for mentions and replies to you.",
                ),
            ],
        ),
        (
            "Privacy",
            vec![
                pending(
                    "Send read receipts",
                    "Control when this client sends read receipts.",
                ),
                pending(
                    "Show when you are typing",
                    "Share typing state while composing.",
                ),
                pending("App lock", "Require a password to open Heya."),
                pending(
                    "Lock after",
                    "Choose how long Heya can remain idle before locking.",
                ),
                pending("Last seen", "Manage who can see your last seen time."),
                pending("Online", "Manage who can see when you are online."),
                pending("Profile photo", "Manage who can see your profile photo."),
                pending("About", "Manage who can see your About text."),
                pending("Groups", "Manage who can add you to groups."),
                pending(
                    "Read receipts",
                    "Manage the read-receipt setting for your whole account.",
                ),
            ],
        ),
        (
            "System",
            vec![
                pending(
                    "Keep running when the window closes",
                    "Continue in the tray; closing the window currently quits Heya.",
                ),
                pending("Start at login", "Start Heya in the tray when you sign in."),
                pending(
                    "Proxy",
                    "Configure a proxy for the connection and media downloads.",
                ),
            ],
        ),
        (
            "Account",
            vec![
                if ui.get_demo_mode()
                    || ui.get_connection_state() == 7
                    || ui.get_connection_blocked()
                {
                    info(
                        "Connection",
                        if ui.get_demo_mode() {
                            "Offline demo — no account is opened."
                        } else {
                            connection
                        },
                    )
                } else {
                    SettingRow {
                        action: if matches!(ui.get_connection_state(), 1..=6) {
                            "Disconnect"
                        } else {
                            "Connect"
                        }
                        .into(),
                        ..row("connection", "Connection", connection, 4)
                    }
                },
                pending("Your name", "Edit your WhatsApp profile name."),
                pending("About", "Edit your WhatsApp profile text."),
                pending(
                    "Change profile picture",
                    "Choose your WhatsApp profile picture.",
                ),
                if ui.get_connection_can_relink() && !ui.get_demo_mode() {
                    SettingRow {
                        action: "Link again".into(),
                        ..row(
                            "relink",
                            "Link this account again",
                            "Keep your saved chats and link the same WhatsApp account.",
                            4,
                        )
                    }
                } else if ui.get_connection_state() == 5 && !ui.get_demo_mode() {
                    SettingRow {
                        action: "Unlink".into(),
                        ..row(
                            "unlink",
                            "Unlink this computer",
                            "Remove this linked device from WhatsApp. Your encrypted chats and drafts stay here.",
                            4,
                        )
                    }
                } else {
                    info(
                        "Unlink this computer",
                        "Connect first, or remove Heya from Linked devices on your phone.",
                    )
                },
            ],
        ),
        (
            "Files",
            vec![
                info(
                    "Message archive",
                    "The session and chat archive are encrypted on this device; their key is kept in the OS keyring.",
                ),
                SettingRow {
                    action: "Choose…".into(),
                    ..row(
                        "download_folder",
                        "Downloads folder",
                        &p.download_directory.to_string_lossy(),
                        4,
                    )
                },
                SettingRow {
                    action: "Use default".into(),
                    ..row(
                        "default_download_folder",
                        "Default download folder",
                        "System Downloads folder, with a Heya subfolder.",
                        4,
                    )
                },
            ],
        ),
        (
            "About",
            vec![
                info(
                    "Heya 0.1.0",
                    "A native WhatsApp client built with Rust, Slint and whatsapp-rust. Visual reference: ZapFast.",
                ),
                toggle(
                    "show_shortcut_hint",
                    "Show keyboard shortcut hint",
                    "Show the hint below the message composer.",
                    p.show_shortcut_hint,
                ),
                info(
                    "Keyboard shortcuts",
                    if cfg!(target_os = "macos") {
                        "⌘, — settings · ⌘B — chat list · ⌘N — new chat · Esc — back. ⌘Enter always sends; Shift+Enter adds a line."
                    } else {
                        "Ctrl+, — settings · Ctrl+B — chat list · Ctrl+N — new chat · Esc — back. Ctrl+Enter always sends; Shift+Enter adds a line."
                    },
                ),
            ],
        ),
    ];
    let query = ui.get_settings_query().trim().to_lowercase();
    let sections = sections
        .into_iter()
        .filter_map(|(title, rows)| {
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|row| {
                    title.to_lowercase().contains(&query)
                        || format!("{} {}", row.title, row.description)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            (!rows.is_empty()).then(|| SettingSection {
                title: title.into(),
                rows: model(rows),
            })
        })
        .collect();
    ui.set_settings_sections(model(sections));
}

#[cfg(test)]
pub(crate) fn check_ui(runtime: &tokio::runtime::Runtime) {
    use i_slint_backend_testing::ElementHandle;
    use slint::{
        Model,
        platform::{Key, WindowEvent},
    };
    fn element(ui: &AppWindow, id: &str) -> ElementHandle {
        ElementHandle::find_by_element_id(ui, id)
            .next()
            .unwrap_or_else(|| panic!("missing {id}"))
    }
    fn inside(child: &ElementHandle, parent: &ElementHandle) {
        let c = child.absolute_position();
        let p = parent.absolute_position();
        assert!(
            c.x >= p.x - 0.5
                && c.y >= p.y - 0.5
                && c.x + child.size().width <= p.x + parent.size().width + 0.5
                && c.y + child.size().height <= p.y + parent.size().height + 0.5,
            "{:?} {:?}/{:?} outside {:?} {:?}/{:?}",
            child.type_name(),
            c,
            child.size(),
            parent.type_name(),
            p,
            parent.size()
        );
    }
    fn key(ui: &AppWindow, key: impl Into<SharedString>) {
        let text = key.into();
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }
    let ui = AppWindow::new().unwrap();
    assert!(
        !ui.global::<Theme>().get_is_dark(),
        "the declaration defaults to light too"
    );
    let (saver, operations, current) = attach(
        &ui,
        runtime,
        Preferences::default(),
        None,
        "Demo preferences last for this run only.",
    );
    assert!(saver.is_none());
    ui.set_audio_devices(model(vec![
        crate::ui::AudioDeviceRow {
            id: "synthetic-input".into(),
            name: "Synthetic microphone".into(),
            input: true,
        },
        crate::ui::AudioDeviceRow {
            id: "synthetic-output".into(),
            name: "Synthetic speakers".into(),
            input: false,
        },
    ]));
    ui.invoke_audio_devices_ready();
    ui.invoke_preference_changed("audio_input".into(), 1);
    ui.invoke_preference_changed("audio_output".into(), 1);
    ui.invoke_preference_changed("input_volume".into(), 65);
    ui.invoke_preference_changed("output_volume".into(), 35);
    assert_eq!(current.borrow().audio.input.id, "synthetic-input");
    assert_eq!(current.borrow().audio.output.id, "synthetic-output");
    assert_eq!(current.borrow().audio.input_volume, 65);
    assert_eq!(current.borrow().audio.output_volume, 35);
    let download = std::env::temp_dir().join("Heya synthetic downloads");
    ui.invoke_download_folder_ready(download.to_string_lossy().as_ref().into());
    assert_eq!(current.borrow().download_directory, download);
    ui.show().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let settings = ElementHandle::find_by_accessible_label(
        &ui,
        if cfg!(target_os = "macos") {
            "Settings (⌘,)"
        } else {
            "Settings (Ctrl+,)"
        },
    )
    .next()
    .unwrap();
    settings.invoke_accessible_default_action();
    assert!(ui.get_settings_open());
    for index in 0..THEMES.len() {
        ui.invoke_preference_changed("theme".into(), index as i32);
        assert_eq!(ui.global::<Theme>().get_follow_system(), index == 0);
        if index >= 3 {
            let theme = ui.global::<Theme>();
            assert_eq!(
                theme.get_panel().as_argb_encoded() & 0xffffff,
                appearance::palettes()[index - 3].colors["panel"]
            );
        }
    }
    ui.invoke_preference_changed("theme".into(), 1);
    assert_eq!(
        ui.global::<Theme>().get_panel(),
        slint::Color::from_rgb_u8(255, 255, 255)
    );
    // The real popup must accept keyboard navigation and commit its choice.
    ElementHandle::find_by_accessible_label(&ui, "Theme: Light")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    key(&ui, Key::DownArrow);
    key(&ui, Key::Return);
    assert!(ui.global::<Theme>().get_is_dark());
    ui.invoke_preference_changed("theme".into(), 1);
    let rows_before = ui.get_settings_sections().row_count();
    assert_eq!(rows_before, 9);
    for (width, height) in [
        (1200., 800.),
        (800., 600.),
        (620., 480.),
        (520., 480.),
        (1920., 1080.),
    ] {
        ui.window().set_size(slint::LogicalSize::new(width, height));
        let column = element(&ui, "SettingsPage::column");
        let page = ElementHandle::find_by_element_type_name(&ui, "SettingsPage")
            .next()
            .unwrap();
        assert!(column.size().width <= 640.5);
        assert!(column.absolute_position().x >= page.absolute_position().x + 31.5);
        assert!(
            column.absolute_position().x + column.size().width
                <= page.absolute_position().x + page.size().width - 31.5
        );
        for row in ElementHandle::find_by_element_type_name(&ui, "SettingsRow") {
            let labels = row
                .query_descendants()
                .match_id("SettingsRow::labels")
                .find_first()
                .unwrap();
            inside(&labels, &row);
            let Some(control) = row
                .query_descendants()
                .match_id("SettingsRow::control")
                .find_first()
            else {
                continue;
            };
            // Information-only rows have no control children.
            if control.query_descendants().find_first().is_some() {
                inside(&control, &row);
                let l = labels.absolute_position();
                let c = control.absolute_position();
                assert!(
                    c.x >= l.x + labels.size().width - 0.5
                        || c.y >= l.y + labels.size().height - 0.5,
                    "setting text and control overlap"
                );
            }
        }
    }
    ui.set_settings_query("READ RECEIPTS".into());
    ui.invoke_search_settings(ui.get_settings_query());
    assert_eq!(ui.get_settings_sections().row_count(), 1);
    let privacy = ui.get_settings_sections().row_data(0).unwrap();
    assert_eq!(privacy.title, "Privacy");
    assert_eq!(privacy.rows.row_count(), 2);
    assert!(privacy.rows.iter().all(|row| row.kind == 2));
    ui.set_settings_query("not-a-setting".into());
    ui.invoke_search_settings(ui.get_settings_query());
    assert_eq!(ui.get_settings_sections().row_count(), 0);
    ui.set_settings_open(false);
    ui.set_settings_query("".into());
    ui.invoke_search_settings(ui.get_settings_query());
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    let header = element(&ui, "AppWindow::sidebar-header");
    let actions = element(&ui, "AppWindow::header-actions");
    inside(&actions, &header);
    assert!((header.size().height - 60.).abs() < 0.5);
    assert!((actions.size().width - 106.).abs() < 0.5);
    assert!((element(&ui, "AppWindow::chat-list").absolute_position().y - 148.).abs() < 0.5);
    ElementHandle::find_by_accessible_label(
        &ui,
        if cfg!(target_os = "macos") {
            "Collapse the chat list (⌘B)"
        } else {
            "Collapse the chat list (Ctrl+B)"
        },
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(ui.get_sidebar_hidden());
    assert!(
        (element(&ui, "AppWindow::conversation")
            .absolute_position()
            .x
            - 60.)
            .abs()
            < 0.5
    );
    ui.set_chats(model(
        (0..500)
            .map(|i| crate::ui::ChatRow {
                id: i + 1,
                title: format!("Rail chat {i}").into(),
                initials: "RC".into(),
                unread: 999,
                ..Default::default()
            })
            .collect(),
    ));
    let rail = element(&ui, "AppWindow::compact-sidebar");
    assert!((rail.size().width - 60.).abs() < 0.5);
    assert!(
        (element(&ui, "AppWindow::compact-list")
            .absolute_position()
            .y
            - 60.)
            .abs()
            < 0.5
    );
    let cells: Vec<_> = ElementHandle::find_by_element_id(&ui, "CompactChat::avatar").collect();
    assert!(
        !cells.is_empty() && cells.len() < 30,
        "only visible rail cells are instantiated"
    );
    for avatar in cells {
        assert!((avatar.size().width - 36.).abs() < 0.5);
        let x = avatar.absolute_position().x;
        assert!(x >= 0. && x + avatar.size().width <= rail.size().width + 0.5);
    }
    for badge in ElementHandle::find_by_element_id(&ui, "CompactChat::badge") {
        let pos = badge.absolute_position();
        assert!(
            pos.x >= 0. && pos.x + badge.size().width <= 60.5,
            "99+ badge fits the rail"
        );
    }
    let selected = Rc::new(std::cell::Cell::new(0));
    let selected_by_callback = selected.clone();
    ui.on_select_chat(move |id| selected_by_callback.set(id));
    ElementHandle::find_by_accessible_label(&ui, "Rail chat 0, 999 unread")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert_eq!(selected.get(), 1);
    ui.set_settings_open(true);
    assert!(
        (element(&ui, "AppWindow::settings-page")
            .absolute_position()
            .x
            - 60.)
            .abs()
            < 0.5
    );
    ui.set_settings_open(false);
    ui.set_chats(model(vec![]));
    ElementHandle::find_by_accessible_label(
        &ui,
        if cfg!(target_os = "macos") {
            "Show the chat list (⌘B)"
        } else {
            "Show the chat list (Ctrl+B)"
        },
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(!ui.get_sidebar_hidden());
    ui.set_new_chat_open(true);
    let dialog = element(&ui, "NewChatDialog::dialog");
    assert!(dialog.size().width <= 420.5);
    key(&ui, Key::Escape);
    assert!(!ui.get_new_chat_open());
    let sent = Rc::new(std::cell::Cell::new(0));
    let count = sent.clone();
    ui.on_send_message(move |_, _| count.set(count.get() + 1));
    let editor = ElementHandle::find_by_accessible_label(&ui, "Message draft")
        .next()
        .unwrap();
    let point = editor.absolute_position();
    let position = slint::LogicalPosition::new(
        point.x + editor.size().width / 2.,
        point.y + editor.size().height / 2.,
    );
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: slint::platform::PointerEventButton::Left,
    });
    ui.set_draft("Synthetic draft".into());
    key(&ui, Key::Return);
    assert_eq!(sent.get(), 1);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Shift.into(),
    });
    key(&ui, Key::Return);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Shift.into(),
    });
    assert_eq!(sent.get(), 1, "Shift+Enter must not send");
    assert!(ui.get_draft().contains('\n'));
    ui.invoke_preference_changed("enter_sends".into(), 0);
    key(&ui, Key::Return);
    assert_eq!(sent.get(), 1);
    assert!(ui.get_draft().contains('\n'));
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    key(&ui, Key::Return);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    assert_eq!(sent.get(), 2);
    ui.invoke_preference_changed("font".into(), 0);
    assert_eq!(ui.get_text_layout_font(), "");
    ui.invoke_preference_changed("font".into(), 1);
    assert_eq!(ui.get_text_layout_font(), "Inter");
    ui.invoke_preference_changed("show_shortcut_hint".into(), 0);
    assert!(!ui.get_show_shortcut_hint());
    ui.invoke_preference_changed("show_wallpaper".into(), 0);
    assert!(!ui.global::<Theme>().get_show_wallpaper());
    // Opening a modal or settings must take focus away from the draft.
    ui.set_new_chat_open(true);
    let _ = element(&ui, "NewChatDialog::dialog");
    key(&ui, Key::Return);
    assert_eq!(sent.get(), 2, "a modal must not send the obscured draft");
    key(&ui, Key::Escape);
    ui.set_settings_open(true);
    let _ = element(&ui, "SettingsPage::column");
    // A late recording/upload completion must not steal the search focus.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(1));
    let draft_before = ui.get_draft();
    ui.set_settings_query("".into());
    ui.invoke_focus_composer();
    key(&ui, "z");
    assert_eq!(ui.get_draft(), draft_before);
    assert_eq!(ui.get_settings_query().as_str(), "z");
    key(&ui, Key::Return);
    assert_eq!(sent.get(), 2, "settings must not send the obscured draft");
    ui.hide().unwrap();
    drop(ui);
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(4), operations)
            .await
            .unwrap()
            .unwrap();
    });
    // Exercise the actual coalesced saver, including shutdown with a final update.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.json");
    let ui = AppWindow::new().unwrap();
    let saver = attach(&ui, runtime, Preferences::default(), Some(path.clone()), "")
        .0
        .unwrap();
    for index in 0..100 {
        ui.invoke_preference_changed("theme".into(), index % THEMES.len() as i32);
    }
    ui.invoke_preference_changed("theme".into(), 5);
    ui.invoke_preference_changed("enter_sends".into(), 0);
    drop(ui);
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(3), saver)
            .await
            .unwrap()
            .unwrap();
    });
    let saved = preferences::load(&path).unwrap();
    assert_eq!(saved.theme, "Nord");
    assert!(!saved.enter_sends);
    let (demo, path, _) = prepare(true);
    assert_eq!(demo, Preferences::default());
    assert!(path.is_none());
}
