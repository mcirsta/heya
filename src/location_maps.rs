//! Static OpenStreetMap previews, fetched only for demanded location cards.
//! No account identifiers, place names or precise coordinates leave this module:
//! the provider receives ordinary zoom/x/y tile requests with Heya's user agent.
use crate::{archive::Archive, locations::Location};
use image::RgbaImage;
use rusqlite::{OptionalExtension, params};
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const ZOOM: u32 = 16;
pub const WIDTH: u32 = 480;
pub const HEIGHT: u32 = 268;
const MAX_TILE: usize = 256 * 1024;
static DOWNLOAD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
pub async fn fetch(archive: Archive, location: Location) -> Option<Vec<u8>> {
    let permit = DOWNLOAD.acquire().await.ok()?;
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let stopped = cancel.0.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let deadline = Instant::now() + Duration::from_secs(12);
        let agent = ureq::Agent::config_builder().https_only(true).max_redirects(0)
            .timeout_global(Some(Duration::from_secs(3))).build().new_agent();
        let pixels = render(&location, |x,y| {
            if stopped.load(Ordering::Relaxed) || Instant::now() >= deadline { return None; }
            // Keep tiles for 30 days (OSM requires at least seven), including
            // when scrolling between messages that share the same neighbourhood.
            let cached = archive.blocking(|db| Ok(db.query_row(
                "SELECT bytes FROM location_map_tiles WHERE zoom=?1 AND x=?2 AND y=?3 AND fetched>unixepoch()-2592000",
                params![ZOOM,x,y], |r|r.get::<_,Vec<u8>>(0)).optional()?)).ok().flatten();
            if let Some(bytes) = cached { return tile(&bytes); }
            let mut response = agent.get(&format!("https://tile.openstreetmap.org/{ZOOM}/{x}/{y}.png"))
                .header("User-Agent",concat!("Heya/",env!("CARGO_PKG_VERSION")," (+https://github.com/mcirsta/heya)"))
                .call().ok()?;
            if !response.status().is_success() { return None; }
            let mut bytes = Vec::new();
            response.body_mut().as_reader().take((MAX_TILE+1) as u64).read_to_end(&mut bytes).ok()?;
            let pixels = tile(&bytes)?;
            archive.blocking(|db| {
                db.execute("DELETE FROM location_map_tiles WHERE fetched<=unixepoch()-2592000", [])?;
                db.execute("INSERT OR REPLACE INTO location_map_tiles(zoom,x,y,bytes,fetched) VALUES(?1,?2,?3,?4,unixepoch())",params![ZOOM,x,y,bytes])?;
                Ok(())
            }).ok()?;
            Some(pixels)
        })?;
        crate::document_previews::encode(crate::photos::Pixels { width:WIDTH, height:HEIGHT, rgba:pixels.into_raw() })
    }).await.ok().flatten()
}
fn tile(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() > MAX_TILE {
        return None;
    }
    let image = crate::message_thumbnails::decode(bytes)?;
    if image.width != 256 || image.height != 256 {
        return None;
    }
    RgbaImage::from_raw(256, 256, image.rgba)
}

/// Compose at one fixed resolution. Resizing the window never fetches or decodes
/// another map. At the date line x wraps; y clamps at the Mercator map edges.
fn render(
    location: &Location,
    mut load: impl FnMut(u32, u32) -> Option<RgbaImage>,
) -> Option<RgbaImage> {
    let (lat, lon) = location.coordinates()?;
    let world = (256_u64 << ZOOM) as f64;
    let latitude = lat.clamp(-85.05112878, 85.05112878).to_radians();
    let center_x = (lon + 180.) / 360. * world;
    let center_y =
        ((1. - latitude.tan().asinh() / std::f64::consts::PI) / 2. * world).clamp(0., world - 1.);
    let left = center_x.round() as i64 - i64::from(WIDTH / 2);
    let top = center_y.round() as i64 - i64::from(HEIGHT / 2);
    let count = 1_i64 << ZOOM;
    let mut map = RgbaImage::from_pixel(WIDTH, HEIGHT, image::Rgba([230, 234, 235, 255]));
    for ty in top.div_euclid(256)..=(top + i64::from(HEIGHT) - 1).div_euclid(256) {
        if !(0..count).contains(&ty) {
            continue;
        }
        for tx in left.div_euclid(256)..=(left + i64::from(WIDTH) - 1).div_euclid(256) {
            let tile = load(tx.rem_euclid(count) as u32, ty as u32)?;
            image::imageops::overlay(&mut map, &tile, tx * 256 - left, ty * 256 - top);
        }
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_is_bounded_and_handles_edges_without_network() {
        for (lat, lon) in [(0., 0.), (90., 180.), (-90., -180.), (45., 179.9999)] {
            let mut count = 0;
            let map = render(&Location::new(lat, lon).unwrap(), |x, y| {
                assert!(x < 1 << ZOOM && y < 1 << ZOOM);
                count += 1;
                Some(RgbaImage::from_pixel(
                    256,
                    256,
                    image::Rgba([40, 80, 120, 255]),
                ))
            })
            .unwrap();
            assert_eq!(map.dimensions(), (WIDTH, HEIGHT));
            assert!(count > 0 && count <= 9);
        }
        assert!(render(&Location::new(0., 0.).unwrap(), |_, _| None).is_none());
        assert!(Location::new(f64::NAN, 0.).is_none());
        assert!(Location::new(0., 181.).is_none());
    }
}
