//! Portable GStreamer device snapshots and per-operation audio configuration.
//! Device discovery runs only in finite helpers, never in a chat row or UI loop.
use crate::voice_recorder::Error;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Selection {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub input: Selection,
    pub output: Selection,
    pub input_volume: u32,
    pub output_volume: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            input: Selection::default(),
            output: Selection::default(),
            input_volume: 100,
            output_volume: 100,
        }
    }
}
impl Settings {
    pub fn valid(&self) -> bool {
        self.input_volume <= 100
            && self.output_volume <= 100
            && [&self.input, &self.output]
                .iter()
                .all(|d| d.id.len() <= 128 && d.name.len() <= 1024)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub input: bool,
}

#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
fn devices(input: bool) -> Result<Vec<gstreamer::Device>, Error> {
    use gstreamer::{self as gst, prelude::*};
    gst::init().map_err(|_| Error::Codec)?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(
        Some(if input { "Audio/Source" } else { "Audio/Sink" }),
        Some(&gst::Caps::new_empty_simple("audio/x-raw")),
    );
    Ok(monitor
        .devices()
        .into_iter()
        .filter(|d| !input || d.properties().is_none_or(|p| usable_input(&p)))
        .take(64)
        .collect())
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
fn info(device: &gstreamer::Device, input: bool) -> Device {
    use gstreamer::prelude::*;
    use sha2::{Digest, Sha256};
    let props = device.properties();
    let stable = props.as_ref().and_then(|p| {
        ["device.path", "device.name", "object.path", "node.name"]
            .iter()
            .find_map(|k| p.get::<String>(*k).ok())
    });
    let name: String = device
        .display_name()
        .chars()
        .filter(|c| !c.is_control())
        .take(240)
        .collect();
    let id = format!(
        "{:x}",
        Sha256::digest(format!(
            "{}:{}",
            if input { "input" } else { "output" },
            stable.as_deref().unwrap_or(&name)
        ))
    );
    Device { id, name, input }
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
pub fn snapshot() -> Result<Vec<Device>, Error> {
    let mut result = Vec::new();
    for input in [true, false] {
        result.extend(devices(input)?.iter().map(|d| info(d, input)));
    }
    Ok(result)
}
#[cfg(not(all(feature = "media-gstreamer", target_os = "linux")))]
pub fn snapshot() -> Result<Vec<Device>, Error> {
    Err(Error::Disabled)
}

#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
pub fn element(input: bool, id: &str) -> Result<gstreamer::Element, Error> {
    use gstreamer::prelude::*;
    let mut devices = devices(input)?;
    devices.sort_by_key(|d| {
        !d.properties()
            .is_some_and(|p| p.get::<bool>("is-default").unwrap_or(false))
    });
    let error = if input {
        Error::NoInput
    } else {
        Error::Playback
    };
    select_device(devices, input, id)?
        .create_element(None)
        .map_err(|_| error)
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
fn select_device(
    devices: Vec<gstreamer::Device>,
    input: bool,
    id: &str,
) -> Result<gstreamer::Device, Error> {
    devices
        .into_iter()
        .find(|d| id.is_empty() || info(d, input).id == id)
        .ok_or(if input {
            Error::NoInput
        } else {
            Error::Playback
        })
}
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
pub(crate) fn usable_input(p: &gstreamer::StructureRef) -> bool {
    !["device.class", "media.class"].iter().any(|key| {
        p.get::<String>(*key)
            .is_ok_and(|v| v.eq_ignore_ascii_case("monitor") || v.starts_with("Audio/Sink"))
    }) && !["device.name", "node.name"]
        .iter()
        .any(|key| p.get::<String>(*key).is_ok_and(|v| v.ends_with(".monitor")))
}

#[cfg(all(test, feature = "media-gstreamer", target_os = "linux"))]
pub fn select_empty_input_for_test() -> Result<(), Error> {
    // Selection is checked before creating any source or pipeline.
    select_device(Vec::new(), true, "").map(|_| ())
}
