//! Accept direct chat links from the OS or command line.
use crate::AppWindow;
use slint::{ComponentHandle, winit_031::WinitWindowAccessor};
use url::Url;

#[cfg(target_os = "linux")]
#[path = "whatsapp_link_linux.rs"]
pub mod linux;
#[cfg(target_os = "macos")]
#[path = "whatsapp_link_macos.rs"]
pub mod macos;

#[derive(Debug, PartialEq, Eq)]
struct ChatLink {
    phone: String,
    text: String,
}

fn parse(raw: &str) -> Option<ChatLink> {
    if raw.len() > 64 * 1024 || raw.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut phone = match (url.scheme(), url.host_str(), url.path()) {
        ("whatsapp", Some("send"), "" | "/")
        | ("https", Some("api.whatsapp.com"), "/send" | "/send/") => None,
        ("https", Some("wa.me"), path) => Some(digits(path.strip_prefix('/')?)?),
        _ => return None,
    };
    let mut text = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            // A literal '+' is decoded as a space in query parameters.
            "phone" if phone.is_none() => {
                phone = Some(digits(value.strip_prefix(' ').unwrap_or(&value))?)
            }
            "text" if text.is_none() => text = Some(value.into_owned()),
            "phone" | "text" => return None,
            _ => {}
        }
    }
    let text = text
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return None;
    }
    Some(ChatLink {
        phone: phone?,
        text,
    })
}

fn digits(value: &str) -> Option<String> {
    if !value
        .strip_prefix('+')
        .unwrap_or(value)
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    crate::domain::phone_number(value)
}

/// Returns whether a direct chat preview was loaded.
pub fn receive(ui: &AppWindow, raw: Option<String>, focus: bool) -> bool {
    if focus {
        ui.window().with_winit_window(|window| {
            window.set_minimized(false);
            window.focus_window();
            #[cfg(target_os = "linux")]
            if !window.has_focus() {
                use slint::winit_031::winit::window::UserAttentionType;
                window.request_user_attention(Some(UserAttentionType::Informational));
            }
        });
    }
    if let Some(raw) = raw {
        if let Some(link) = parse(&raw) {
            ui.set_linked_chat_preview(crate::domain::single_line_summary(&link.text).into());
            ui.set_linked_chat_text(link.text.into());
            ui.set_linked_chat_error("".into());
            ui.set_linked_chat_phone(format!("+{}", link.phone).into());
            return true;
        } else {
            ui.invoke_show_error(
                "This WhatsApp link does not contain a supported direct chat.".into(),
            );
        }
    }
    false
}

#[cfg(test)]
pub(crate) fn check_ui() {
    use crate::messaging::Command;
    use i_slint_backend_testing::ElementHandle;

    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(false);
    ui.show().unwrap();
    let (tx, mut commands) = tokio::sync::mpsc::channel(1);
    crate::live_presentation::bind_messages(&ui, tx);
    ui.set_selected_id(12);
    ui.set_draft("The current draft".into());
    receive(
        &ui,
        Some("whatsapp://send?phone=15550101010&text=Hello%0AWorld".into()),
        false,
    );
    let open = || ElementHandle::find_by_accessible_label(&ui, "Open chat").next();
    assert!(
        open().is_none(),
        "link must wait until the account is ready"
    );
    assert!(commands.try_recv().is_err());
    ui.set_connection_state(5);
    ui.set_archive_ready(true);
    ui.set_live_chat_view(true);
    ui.set_live_shown_revision(ui.get_live_revision());
    open()
        .expect("link visible after connecting")
        .invoke_accessible_default_action();
    assert!(matches!(
        commands.try_recv().unwrap(),
        Command::NewChat { phone, prefill, previous: 12, draft, .. }
            if phone == "+15550101010" && prefill == "Hello\nWorld" && draft == "The current draft"
    ));
    assert!(ui.get_linked_chat_phone().is_empty());
    assert!(
        commands.try_recv().is_err(),
        "opening a link must not send a message"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_chat_links_and_decodes_drafts() {
        for raw in [
            "whatsapp://send?phone=%2B15550101010&text=Caf%C3%A9%0D%0AHello%21&source=web",
            "https://api.whatsapp.com/send?phone=15550101010&text=Caf%C3%A9%0AHello%21",
            "https://wa.me/15550101010?text=Caf%C3%A9%0AHello%21",
        ] {
            assert_eq!(
                parse(raw),
                Some(ChatLink {
                    phone: "15550101010".into(),
                    text: "Café\nHello!".into()
                })
            );
        }
        assert_eq!(parse("whatsapp://send?phone=15550101010").unwrap().text, "");
    }

    #[test]
    fn rejects_ambiguous_non_direct_and_unrelated_links() {
        for raw in [
            "whatsapp://send?text=Hello",
            "whatsapp://send?phone=123",
            "whatsapp://call?phone=15550101010",
            "whatsapp://send?phone=15550101010&phone=15550101011",
            "whatsapp://send?phone=15550%09101010",
            "whatsapp://send?phone=15550101010&text=%00",
            "https://chat.whatsapp.com/invite",
            "https://wa.me@other.invalid/15550101010",
            "https://wa.me/message/123",
            "https://wa.me/15550101010?phone=15550101011",
            "http://wa.me/15550101010",
        ] {
            assert!(parse(raw).is_none(), "{raw}");
        }
    }
}
