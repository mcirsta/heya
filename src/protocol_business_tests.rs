use super::*;
use wa::message::{InteractiveMessage, TemplateMessage, interactive_message, template_message};
use whatsapp_rust::waproto::{buffa::MessageField, codec};

fn button(name: &str, json: String) -> interactive_message::native_flow_message::NativeFlowButton {
    let mut button = interactive_message::native_flow_message::NativeFlowButton::default();
    button.name = Some(name.into());
    button.button_params_json = Some(json);
    button
}
fn template_message(template: TemplateMessage) -> wa::Message {
    let mut message = wa::Message::default();
    message.template_message = MessageField::some(template);
    message
}
pub(crate) fn template(body: &str) -> wa::Message {
    let mut flow = interactive_message::NativeFlowMessage::default();
    flow.buttons = vec![button(
        "cta_url",
        serde_json::json!({
            "display_text": "Track package",
            "url": "https://example.invalid/tracking",
            "id": "internal-not-for-display"
        })
        .to_string(),
    )];
    let mut interactive = InteractiveMessage::default();
    interactive.body.get_or_insert_default().text = Some(body.into());
    interactive.interactive_message = Some(
        interactive_message::InteractiveMessage::NativeFlowMessage(Box::new(flow)),
    );
    let mut template = TemplateMessage::default();
    template.format = Some(template_message::Format::InteractiveMessageTemplate(
        Box::new(interactive),
    ));
    template_message(template)
}

#[test]
fn native_flow_template_preserves_full_text_and_copyable_link() {
    let body = format!(
        "Synthetic delivery 👩🏽‍💻\n{}\nEnd",
        "Long Unicode 文字 🌻\n".repeat(700)
    );
    let message = template(&body);
    // Exercise the real protobuf round trip, not only struct construction.
    let decoded = codec::message_decode(&codec::message_to_vec(&message)).unwrap();
    assert_eq!(
        text(&decoded).unwrap(),
        format!("{body}\n\nTrack package: https://example.invalid/tracking")
    );
    let mut wrapper = wa::message::FutureProofMessage::default();
    wrapper.message = MessageField::some(decoded.clone());
    let mut wrapped = wa::Message::default();
    wrapped.ephemeral_message = MessageField::some(wrapper.clone());
    assert_eq!(text(&wrapped), text(&decoded));
    let mut once = wa::Message::default();
    once.view_once_message = MessageField::some(wrapper);
    assert!(text(&once).is_none());
}

#[test]
fn malformed_and_unknown_actions_do_not_hide_body_or_leak_internal_payloads() {
    let mut interactive = InteractiveMessage::default();
    interactive.header.get_or_insert_default().title = Some("Title".into());
    interactive.header.get_or_insert_default().subtitle = Some("Subtitle".into());
    interactive.body.get_or_insert_default().text = Some("Keep this body".into());
    interactive.footer.get_or_insert_default().text = Some("Footer".into());
    let mut flow = interactive_message::NativeFlowMessage::default();
    flow.buttons = vec![
        button("cta_url", "{malformed".into()),
        button("cta_url", "x".repeat(64 * 1024 + 1)),
        button(
            "unknown",
            r#"{"display_text":"Choose","url":"internal-not-for-display","id":"opaque"}"#.into(),
        ),
        button(
            "cta_copy",
            r#"{"display_text":"Code","copy_code":"SYNTHETIC-42"}"#.into(),
        ),
        button(
            "cta_call",
            r#"{"display_text":"Call","phone_number":"+15550000001"}"#.into(),
        ),
        button(
            "quick_reply",
            r#"{"display_text":"Confirm","id":"opaque"}"#.into(),
        ),
        button(
            "cta_url",
            r#"{"display_text":"  ","title":"Details","url":"https://example.invalid/details"}"#
                .into(),
        ),
    ];
    interactive.interactive_message = Some(
        interactive_message::InteractiveMessage::NativeFlowMessage(Box::new(flow)),
    );
    let mut message = wa::Message::default();
    message.interactive_message = MessageField::some(interactive);
    let output = text(&message).unwrap();
    assert!(output.starts_with("Title\n\nSubtitle\n\nKeep this body\n\nFooter"));
    assert!(output.contains("Code: SYNTHETIC-42"));
    assert!(output.contains("Call: +15550000001"));
    assert!(output.contains("Confirm · open on your phone"));
    assert!(output.contains("Details: https://example.invalid/details"));
    assert_eq!(output.matches("[Action · open on your phone]").count(), 2);
    assert!(!output.contains("internal-not-for-display"));
    assert!(!output.contains("opaque"));
    let (_, content) = parse(&message).unwrap();
    assert_eq!(
        content.body.as_deref(),
        Some("Title\n\nSubtitle\n\nKeep this body\n\nFooter")
    );
    assert_eq!(
        content.actions[3].target,
        message_content::Target::Copy("SYNTHETIC-42".into())
    );
    for index in [0, 1, 2, 4, 5] {
        assert_eq!(
            content.actions[index].target,
            message_content::Target::Unavailable
        );
    }
    assert_eq!(
        content.actions[6].target,
        message_content::Target::Url("https://example.invalid/details".into())
    );
}

#[test]
fn hydrated_template_variants_preserve_sections_and_respect_linked_device_mask() {
    use wa::hydrated_template_button::{HydratedButton, HydratedURLButton};
    let mut hydrated = template_message::HydratedFourRowTemplate::default();
    hydrated.title = Some(
        template_message::hydrated_four_row_template::Title::HydratedTitleText("Title".into()),
    );
    hydrated.hydrated_content_text = Some("Body\nwith newlines".into());
    hydrated.hydrated_footer_text = Some("Footer".into());
    let mut url = HydratedURLButton::default();
    url.display_text = Some("Details".into());
    url.url = Some("https://example.invalid".into());
    let mut button = wa::HydratedTemplateButton::default();
    button.hydrated_button = Some(HydratedButton::UrlButton(Box::new(url)));
    hydrated.hydrated_buttons = vec![button];
    let mut explicit = TemplateMessage::default();
    explicit.hydrated_template = MessageField::some(hydrated.clone());
    let mut oneof = TemplateMessage::default();
    oneof.format = Some(template_message::Format::HydratedFourRowTemplate(Box::new(
        hydrated.clone(),
    )));
    let expected = "Title\n\nBody\nwith newlines\n\nFooter\n\nDetails: https://example.invalid";
    assert_eq!(text(&template_message(explicit.clone())).unwrap(), expected);
    assert_eq!(text(&template_message(oneof)).unwrap(), expected);
    let mut hsm = wa::Message::default();
    hsm.highly_structured_message
        .get_or_insert_default()
        .hydrated_hsm = MessageField::some(explicit);
    assert_eq!(text(&hsm).unwrap(), expected);
    hydrated.mask_linked_devices = Some(true);
    let mut masked = TemplateMessage::default();
    masked.hydrated_template = MessageField::some(hydrated);
    assert_eq!(text(&template_message(masked)).unwrap(), PHONE);
    assert_eq!(
        text(&template_message(TemplateMessage::default())).unwrap(),
        PHONE
    );
    assert!(text(&wa::Message::default()).is_none());
}
