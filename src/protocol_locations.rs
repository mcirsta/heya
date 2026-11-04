//! Location decoding stays in the protocol adapter; no map requests at ingest.
use crate::{locations::Location, message_content::Content};
use whatsapp_rust::prelude::wa;

pub fn extract(message: &wa::Message) -> Option<(Option<String>, Content, Vec<u8>)> {
    let fixed = message.location_message.as_option()?;
    if fixed.is_live == Some(true) {
        return Some((
            Some(crate::protocol_system::LIVE.into()),
            Content::default(),
            Vec::new(),
        ));
    }
    let location = fixed
        .degrees_latitude
        .zip(fixed.degrees_longitude)
        .and_then(|(lat, lon)| Location::new(lat, lon));
    let Some(mut location) = location else {
        return Some((
            Some("[Location unavailable]".into()),
            Content::default(),
            Vec::new(),
        ));
    };
    location.name = fixed
        .name
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(256)
        .collect();
    location.address = fixed
        .address
        .as_deref()
        .unwrap_or("")
        .chars()
        .take(1024)
        .collect();
    location.website = fixed
        .url
        .as_deref()
        .filter(|s| s.len() <= 8192)
        .and_then(crate::message_content::web_url)
        .unwrap_or_default();
    let text = location.transcript();
    Some((
        Some(text),
        Content {
            body: Some(String::new()),
            location: Some(location),
            ..Default::default()
        },
        Vec::new(),
    ))
}
