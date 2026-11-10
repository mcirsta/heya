//! Location decoding stays in the protocol adapter; no map requests at ingest.
use crate::{locations::Location, message_content::Content};
use whatsapp_rust::prelude::wa;

pub fn extract(message: &wa::Message) -> Option<(Option<String>, Content, Vec<u8>)> {
    if let Some(live) = message.live_location_message.as_option() {
        return Some(live_card(
            live.degrees_latitude,
            live.degrees_longitude,
            live.caption.as_deref(),
        ));
    }
    let fixed = message.location_message.as_option()?;
    if fixed.is_live == Some(true) {
        return Some(live_card(
            fixed.degrees_latitude,
            fixed.degrees_longitude,
            fixed.comment.as_deref(),
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

fn live_card(
    lat: Option<f64>,
    lon: Option<f64>,
    caption: Option<&str>,
) -> (Option<String>, Content, Vec<u8>) {
    // Invalid/missing coordinates still have a recognizable unavailable card,
    // but cannot request a map. A location update is not a tracking subscription.
    let mut location = lat
        .zip(lon)
        .and_then(|(a, b)| Location::new(a, b))
        .unwrap_or_default();
    location.live = true;
    let caption = caption.unwrap_or_default().to_owned();
    let text = if caption.is_empty() {
        location.transcript()
    } else {
        format!("{}\n{caption}", location.transcript())
    };
    (
        Some(text),
        Content {
            location: Some(location),
            body: Some(caption),
            ..Default::default()
        },
        Vec::new(),
    )
}
