//! Received location cards. Coordinate strings retain the wire precision and
//! keep the archive's equality-based snapshots free of non-finite floats.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    /// A static live-location preview; linked devices cannot follow the sender.
    #[serde(default)]
    pub live: bool,
    pub latitude: String,
    pub longitude: String,
    pub name: String,
    pub address: String,
    pub website: String,
}
impl Location {
    pub fn new(latitude: f64, longitude: f64) -> Option<Self> {
        valid(latitude, longitude).then(|| Self {
            latitude: latitude.to_string(),
            longitude: longitude.to_string(),
            ..Default::default()
        })
    }
    pub fn coordinates(&self) -> Option<(f64, f64)> {
        let (lat, lon) = (self.latitude.parse().ok()?, self.longitude.parse().ok()?);
        valid(lat, lon).then_some((lat, lon))
    }
    pub fn coordinate_text(&self) -> String {
        format!("{}, {}", self.latitude, self.longitude)
    }
    pub fn map_url(&self) -> String {
        let Some((lat, lon)) = self.coordinates() else {
            return String::new();
        };
        format!("https://maps.google.com/maps?q={lat},{lon}&z=17")
    }
    pub fn summary(&self) -> &str {
        if self.live {
            "Live location"
        } else if !self.name.is_empty() {
            &self.name
        } else if !self.address.is_empty() {
            &self.address
        } else {
            "Location"
        }
    }
    pub fn transcript(&self) -> String {
        if self.live {
            return "Live location".into();
        }
        let mut parts = vec!["Location".to_owned()];
        parts.extend(
            [&self.name, &self.address]
                .into_iter()
                .filter(|s| !s.is_empty())
                .cloned(),
        );
        parts.push(self.map_url());
        parts.join("\n")
    }
}
fn valid(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
}
