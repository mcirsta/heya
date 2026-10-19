//! Local presentation preferences. No account data or toolkit types belong here.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const THEMES: &[&str] = &[
    "Follow system",
    "Light",
    "Dark",
    "Catppuccin Latte",
    "Catppuccin",
    "Nord",
    "Ristretto",
    "Rose Pine Dawn",
    "Rose Pine Moon",
    "Rose Pine",
    "Tokyo Night",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub theme: String,
    pub font: String,
    pub enter_sends: bool,
    pub show_shortcut_hint: bool,
    pub show_wallpaper: bool,
    pub auto_download_photos: bool,
    pub send_receipts: bool,
    pub audio: crate::audio_devices::Settings,
    pub download_directory: PathBuf,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            theme: "Light".into(),
            font: if cfg!(target_os = "macos") {
                "System"
            } else {
                "Inter"
            }
            .into(),
            enter_sends: true,
            show_shortcut_hint: true,
            show_wallpaper: true,
            auto_download_photos: true,
            send_receipts: true,
            audio: Default::default(),
            download_directory: default_download_directory(),
        }
    }
}

impl Preferences {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1 {
            return Err("Unsupported settings version");
        }
        if !THEMES.contains(&self.theme.as_str()) {
            return Err("Unknown theme");
        }
        if !["System", "Inter"].contains(&self.font.as_str()) {
            return Err("Unknown font");
        }
        if !self.audio.valid() || !self.download_directory.is_absolute() {
            return Err("Invalid media settings");
        }
        Ok(())
    }

    /// Reject invalid and unfinished controls at the application boundary too.
    pub fn change(&mut self, key: &str, value: i32) -> bool {
        match key {
            "theme" => match usize::try_from(value).ok().and_then(|i| THEMES.get(i)) {
                Some(name) => self.theme = (*name).into(),
                None => return false,
            },
            "font" if (0..=1).contains(&value) => {
                self.font = if value == 0 { "System" } else { "Inter" }.into()
            }
            "enter_sends"
            | "show_shortcut_hint"
            | "show_wallpaper"
            | "auto_download_photos"
            | "send_receipts"
                if (0..=1).contains(&value) =>
            {
                let field = match key {
                    "send_receipts" => &mut self.send_receipts,
                    "auto_download_photos" => &mut self.auto_download_photos,
                    "enter_sends" => &mut self.enter_sends,
                    "show_shortcut_hint" => &mut self.show_shortcut_hint,
                    _ => &mut self.show_wallpaper,
                };
                *field = value == 1;
            }
            "input_volume" if (0..=100).contains(&value) => self.audio.input_volume = value as u32,
            "output_volume" if (0..=100).contains(&value) => {
                self.audio.output_volume = value as u32
            }
            _ => return false,
        }
        true
    }
}

pub fn default_download_directory() -> PathBuf {
    let base = directories::UserDirs::new()
        .map(|u| {
            u.download_dir()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| u.home_dir().join("Downloads"))
        })
        .unwrap_or_else(std::env::temp_dir);
    download_subdirectory(&base)
}
fn download_subdirectory(base: &Path) -> PathBuf {
    if base
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("Heya"))
    {
        base.to_path_buf()
    } else {
        base.join("Heya")
    }
}

pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("org", "heya", "Heya")
        .map(|dirs| dirs.config_dir().join("settings.json"))
}

pub fn load(path: &Path) -> anyhow::Result<Preferences> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Preferences::default());
        }
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(65_537).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 65_536, "Settings file is too large");
    let preferences: Preferences = serde_json::from_slice(&bytes)?;
    preferences.validate().map_err(anyhow::Error::msg)?;
    Ok(preferences)
}

/// Replacement is atomic; a failed save leaves the previous file intact.
/// Called on a worker, never by a Slint event handler.
pub fn save(path: &Path, preferences: &Preferences) -> anyhow::Result<()> {
    preferences.validate().map_err(anyhow::Error::msg)?;
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No settings directory"))?;
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut file, preferences)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_roundtrip_and_reject_unsupported_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(load(&path).unwrap().theme, "Light");
        let mut settings = Preferences::default();
        assert_eq!(
            download_subdirectory(Path::new("/tmp/Downloads")),
            Path::new("/tmp/Downloads/Heya")
        );
        assert_eq!(
            download_subdirectory(Path::new("/tmp/Downloads/Heya")),
            Path::new("/tmp/Downloads/Heya")
        );
        settings.download_directory = dir.path().join("Chosen downloads");
        settings.audio.input = crate::audio_devices::Selection {
            id: "synthetic-input".into(),
            name: "Synthetic microphone".into(),
        };
        assert!(settings.change("input_volume", 65));
        assert!(settings.change("output_volume", 35));
        assert!(!settings.change("input_volume", 101));
        for index in 0..THEMES.len() {
            assert!(settings.change("theme", index as i32));
            save(&path, &settings).unwrap();
            assert_eq!(load(&path).unwrap(), settings);
        }
        assert!(!settings.change("theme", -1));
        assert!(!settings.change("theme", THEMES.len() as i32));
        assert!(!settings.change("send_read_receipts", 1));
        assert!(settings.change("send_receipts", 0));
        save(&path, &settings).unwrap();
        assert!(!load(&path).unwrap().send_receipts);
        assert!(!settings.change("send_receipts", 2));
        assert!(settings.change("send_receipts", 1));
        assert!(!settings.change("enter_sends", 2));
        assert!(settings.change("auto_download_photos", 0));
        save(&path, &settings).unwrap();
        assert!(!load(&path).unwrap().auto_download_photos);
        assert!(!settings.change("auto_download_photos", 2));
        let previous = std::fs::read(&path).unwrap();
        settings.version = 2;
        assert!(save(&path, &settings).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), previous);
        for invalid in ["{broken", "{\"version\":2}", "{\"theme\":\"missing\"}"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
        }
    }
}
