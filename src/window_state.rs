//! Small fallback for platforms without automatic window restoration. No account data.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Saved {
    pub version: u32,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    pub fullscreen: bool,
    /// Win32 workspace coordinates, used only with Get/SetWindowPlacement.
    pub windows_normal: Option<[i32; 4]>,
}
impl Default for Saved {
    fn default() -> Self {
        Self {
            version: 1,
            width: 1200.,
            height: 800.,
            maximized: false,
            fullscreen: false,
            windows_normal: None,
        }
    }
}
impl Saved {
    pub fn valid(&self) -> bool {
        self.version == 1
            && size_valid(self.width, self.height)
            && self.windows_normal.is_none_or(|[l, t, r, b]| {
                (1..=131072).contains(&(i64::from(r) - i64::from(l)))
                    && (1..=131072).contains(&(i64::from(b) - i64::from(t)))
            })
    }
    pub fn observe(
        &mut self,
        width: f64,
        height: f64,
        maximized: bool,
        fullscreen: bool,
        minimized: bool,
    ) {
        if minimized || width <= 0. || height <= 0. {
            return;
        }
        if !fullscreen {
            self.maximized = maximized;
        }
        self.fullscreen = fullscreen;
        if !maximized && !fullscreen && size_valid(width, height) {
            self.width = width;
            self.height = height;
        }
    }
}
fn size_valid(w: f64, h: f64) -> bool {
    w.is_finite() && h.is_finite() && (520. ..=8192.).contains(&w) && (480. ..=8192.).contains(&h)
}

pub fn load(path: &Path) -> anyhow::Result<Saved> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Saved::default()),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 4096, "Window state is too large");
    let saved: Saved = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(saved.valid(), "Invalid window state");
    Ok(saved)
}
pub fn save(path: &Path, saved: &Saved) -> anyhow::Result<()> {
    anyhow::ensure!(saved.valid(), "Invalid window state");
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No state directory"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, saved)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

/// Fit restored logical dimensions to the available display after DPI/monitor
/// changes. Placement remains with the desktop on the portable path.
pub fn fit(saved: &Saved, screen: Option<([u32; 2], f64)>) -> (f64, f64) {
    let Some((size, scale)) = screen else {
        return (saved.width, saved.height);
    };
    let scale = if scale.is_finite() && scale > 0. {
        scale
    } else {
        1.
    };
    (
        saved
            .width
            .min((f64::from(size[0]) / scale - 24.).max(520.)),
        saved
            .height
            .min((f64::from(size[1]) / scale - 64.).max(480.)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_bounds_survive_maximize_fullscreen_and_minimize() {
        let mut s = Saved::default();
        s.observe(930., 670., false, false, false);
        let normal = s.clone();
        s.observe(1920., 1080., true, false, false);
        assert!(s.maximized);
        assert_eq!((s.width, s.height), (normal.width, normal.height));
        s.observe(1920., 1080., false, true, false);
        assert!(s.fullscreen && s.maximized);
        let before = s.clone();
        s.observe(0., 0., false, false, true);
        assert_eq!(s, before);
        s.observe(930., 670., false, false, false);
        assert_eq!(s, normal);
    }
    #[test]
    fn bounded_atomic_roundtrip_rejects_corruption_without_clobbering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("window-state.json");
        let mut s = load(&path).unwrap();
        s.observe(950., 700., true, false, false);
        s.windows_normal = Some([-900, 40, 50, 740]);
        save(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        let bytes = std::fs::read(&path).unwrap();
        s.width = f64::NAN;
        assert!(save(&path, &s).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        for bad in [
            b"not JSON".to_vec(),
            vec![b' '; 4097],
            br#"{"version":99}"#.to_vec(),
            br#"{"width":0}"#.to_vec(),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(load(&path).is_err());
        }
    }
    #[test]
    fn screen_and_dpi_changes_keep_normal_dimensions_usable() {
        let s = Saved {
            width: 2000.,
            height: 1400.,
            ..Default::default()
        };
        let (w, h) = fit(&s, Some(([1920, 1080], 1.5)));
        assert!(w <= 1280. && h <= 720.);
        assert_eq!(fit(&s, None), (s.width, s.height));
        assert_eq!(fit(&s, Some(([320, 240], 1.))), (520., 480.));
    }
}
