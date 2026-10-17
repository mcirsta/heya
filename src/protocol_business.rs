//! Business-template extraction. Display metadata contains no executable protocol flows.
use crate::message_content::{self, Content};
use whatsapp_rust::prelude::{MessageExt, wa};

#[cfg(test)]
#[path = "protocol_business_tests.rs"]
pub(crate) mod tests;

const PHONE: &str = "[Business message · open on your phone]";

pub fn text(message: &wa::Message) -> Option<String> {
    parse(message).map(|(text, _)| text)
}

pub fn parse(message: &wa::Message) -> Option<(String, Content)> {
    // Keep the same privacy boundary for live ingestion and archive repair.
    if message.is_view_once() {
        return None;
    }
    let message = message.get_base_message();
    let mut parts = Parts::default();
    if let Some(template) = message.template_message.as_option() {
        template_text(template, &mut parts);
    } else if let Some(interactive) = message.interactive_message.as_option() {
        interactive_text(interactive, &mut parts);
    } else {
        let template = message.highly_structured_message.hydrated_hsm.as_option()?;
        template_text(template, &mut parts);
    }
    let text = if parts.transcript.is_empty() {
        PHONE.into()
    } else {
        parts.transcript.join("\n\n")
    };
    if parts.omitted {
        parts
            .body
            .push("[More actions · open on your phone]".into());
    }
    let body = if parts.body.is_empty() && parts.actions.is_empty() {
        PHONE.into()
    } else {
        parts.body.join("\n\n")
    };
    Some((
        text,
        Content {
            body: Some(body),
            actions: parts.actions,
            ..Default::default()
        },
    ))
}

#[derive(Default)]
struct Parts {
    transcript: Vec<String>,
    body: Vec<String>,
    actions: Vec<message_content::Action>,
    omitted: bool,
}
impl Parts {
    fn push(&mut self, text: Option<&str>) {
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            // Preserve line breaks and full content; only the sidebar summarizes.
            self.transcript.push(text.to_owned());
            self.body.push(text.to_owned());
        }
    }
    fn media(&mut self, label: &str, caption: Option<&str>) {
        self.push(Some(label));
        self.push(caption);
    }
    fn action(&mut self, label: Option<&str>, value: Option<&str>, kind: &str) {
        if self.actions.len() < message_content::MAX_ACTIONS {
            self.actions
                .push(message_content::action(label, value, kind));
        } else {
            self.omitted = true;
        }
        let label = label.filter(|text| !text.trim().is_empty());
        let value = value.filter(|text| !text.trim().is_empty());
        match (label, value) {
            (Some(label), Some(value)) if label != value => {
                self.transcript.push(format!("{label}: {value}"));
            }
            (_, Some(value)) => self.transcript.push(value.into()),
            (Some(label), None) => self
                .transcript
                .push(format!("{label} · open on your phone")),
            _ => self.transcript.push("[Action · open on your phone]".into()),
        }
    }
}

fn template_text(template: &wa::message::TemplateMessage, parts: &mut Parts) {
    use wa::message::template_message::Format;
    if let Some(hydrated) = template.hydrated_template.as_option() {
        hydrated_text(hydrated, parts);
    } else {
        match &template.format {
            Some(Format::HydratedFourRowTemplate(hydrated)) => hydrated_text(hydrated, parts),
            Some(Format::InteractiveMessageTemplate(interactive)) => {
                interactive_text(interactive, parts);
            }
            // Unhydrated templates require server-localized text. Never display
            // internal template identifiers or raw substitution parameters.
            _ => parts.push(Some(PHONE)),
        }
    }
}

fn hydrated_text(
    template: &wa::message::template_message::HydratedFourRowTemplate,
    parts: &mut Parts,
) {
    use wa::{
        hydrated_template_button::HydratedButton,
        message::template_message::hydrated_four_row_template::Title,
    };
    if template.mask_linked_devices == Some(true) {
        parts.push(Some(PHONE));
        return;
    }
    match &template.title {
        Some(Title::HydratedTitleText(title)) => parts.push(Some(title)),
        Some(Title::ImageMessage(image)) => parts.media("[Photo]", image.caption.as_deref()),
        Some(Title::VideoMessage(video)) => parts.media("[Video]", video.caption.as_deref()),
        Some(Title::DocumentMessage(document)) => {
            parts.media("[Document]", document.file_name.as_deref());
            parts.push(document.caption.as_deref());
        }
        Some(Title::LocationMessage(_)) => parts.push(Some("[Location · open on your phone]")),
        _ => {}
    }
    parts.push(template.hydrated_content_text.as_deref());
    parts.push(template.hydrated_footer_text.as_deref());
    for button in &template.hydrated_buttons {
        match &button.hydrated_button {
            Some(HydratedButton::UrlButton(button)) => {
                parts.action(button.display_text.as_deref(), button.url.as_deref(), "url");
            }
            Some(HydratedButton::CallButton(button)) => {
                parts.action(
                    button.display_text.as_deref(),
                    button.phone_number.as_deref(),
                    "phone",
                );
            }
            Some(HydratedButton::QuickReplyButton(button)) => {
                parts.action(button.display_text.as_deref(), None, "unavailable");
            }
            _ => parts.action(None, None, "unavailable"),
        }
    }
}

fn interactive_text(message: &wa::message::InteractiveMessage, parts: &mut Parts) {
    use wa::message::interactive_message::{InteractiveMessage, header::Media};
    if let Some(header) = message.header.as_option() {
        parts.push(header.title.as_deref());
        parts.push(header.subtitle.as_deref());
        match &header.media {
            Some(Media::ImageMessage(image)) => parts.media("[Photo]", image.caption.as_deref()),
            Some(Media::VideoMessage(video)) => parts.media("[Video]", video.caption.as_deref()),
            Some(Media::DocumentMessage(document)) => {
                parts.media("[Document]", document.file_name.as_deref());
                parts.push(document.caption.as_deref());
            }
            Some(_) => parts.push(Some("[Attachment · open on your phone]")),
            None => {}
        }
    }
    parts.push(message.body.text.as_deref());
    parts.push(message.footer.text.as_deref());
    match &message.interactive_message {
        Some(InteractiveMessage::NativeFlowMessage(flow)) => {
            for button in &flow.buttons {
                native_button(button, parts);
            }
        }
        Some(InteractiveMessage::CarouselMessage(_)) => {
            parts.push(Some("[Cards · open on your phone]"));
        }
        Some(_) => parts.push(Some("[Business actions · open on your phone]")),
        None => {}
    }
}

fn native_button(
    button: &wa::message::interactive_message::native_flow_message::NativeFlowButton,
    parts: &mut Parts,
) {
    // Only decode known display fields. IDs, tracking payloads and opaque flow
    // data are not user-facing content. Malformed actions cannot hide the body.
    #[derive(serde::Deserialize)]
    struct Display {
        display_text: Option<String>,
        title: Option<String>,
        url: Option<String>,
        phone_number: Option<String>,
        copy_code: Option<String>,
    }
    let display = button
        .button_params_json
        .as_deref()
        .filter(|json| json.len() <= 64 * 1024)
        .and_then(|json| serde_json::from_str::<Display>(json).ok());
    let Some(display) = display else {
        parts.action(None, None, "unavailable");
        return;
    };
    let value = match button.name.as_deref() {
        Some("cta_url") => display.url.as_deref(),
        Some("cta_call") => display.phone_number.as_deref(),
        Some("cta_copy") => display.copy_code.as_deref(),
        _ => None,
    };
    let label = display
        .display_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .or(display.title.as_deref());
    let kind = match button.name.as_deref() {
        Some("cta_url") => "url",
        Some("cta_copy") => "copy",
        _ => "unavailable",
    };
    parts.action(label, value, kind);
}
