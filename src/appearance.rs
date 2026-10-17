//! Visual palette data from the pinned reference; no widget or protocol types.
//! Provenance: THIRD_PARTY_NOTICES.md and ui/themes/*.json.
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    pub colors: BTreeMap<String, u32>,
}

fn rgb(color: u32) -> [f64; 3] {
    [
        (color >> 16 & 255) as f64,
        (color >> 8 & 255) as f64,
        (color & 255) as f64,
    ]
}
fn mix(a: u32, b: u32, amount: f64) -> u32 {
    let a = rgb(a);
    let b = rgb(b);
    (0..3).fold(0, |color, i| {
        (color << 8) | (a[i] + (b[i] - a[i]) * amount).round() as u32
    })
}
fn luminance(color: u32) -> f64 {
    let rgb = rgb(color).map(|v| {
        let v = v / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}
fn contrast(a: u32, b: u32) -> f64 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn readable(foreground: u32, background: u32, text: u32) -> u32 {
    (0..=20)
        .map(|step| mix(foreground, text, step as f64 / 20.))
        .find(|color| contrast(*color, background) >= 4.5)
        .unwrap_or(text)
}

fn parse(json: &str) -> Palette {
    #[derive(serde::Deserialize)]
    struct Document {
        base: String,
        colors: BTreeMap<String, String>,
    }
    let document: Document = serde_json::from_str(json).expect("bundled palette JSON");
    let mut colors: BTreeMap<String, u32> = document
        .colors
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                u32::from_str_radix(value.trim_start_matches('#'), 16).expect("bundled RGB color"),
            )
        })
        .collect();
    let dark = document.base == "dark";
    // The reference derives its chat roles from these shared interface palettes.
    colors.insert("chat".into(), colors["window"]);
    colors.insert(
        "bubble_in".into(),
        mix(colors["surface"], colors["text"], 0.05),
    );
    colors.insert(
        "bubble_out".into(),
        mix(colors["surface"], colors["accent"], 0.24),
    );
    colors.insert(
        "date".into(),
        if dark {
            mix(colors["panel"], colors["bubble_in"], 0.5)
        } else {
            colors["panel"]
        },
    );
    colors.insert(
        "outgoing_secondary".into(),
        readable(colors["secondary"], colors["bubble_out"], colors["text"]),
    );
    let [r, g, b] = rgb(colors["chat"]);
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let (ink, step): (f64, f64) = if luma - 30. > 255. - luma {
        (30., 23.2)
    } else {
        (255., 38.)
    };
    let alpha = (step / (luma - ink).abs().max(1.)).clamp(0.1, 0.4);
    colors.insert(
        "wallpaper_ink".into(),
        (((alpha * 255.).round() as u32) << 24) | ((ink as u32) * 0x010101),
    );
    Palette { dark, colors }
}

pub fn palettes() -> &'static [Palette] {
    static PALETTES: OnceLock<Vec<Palette>> = OnceLock::new();
    PALETTES.get_or_init(|| {
        [
            include_str!("../ui/themes/Catppuccin Latte.json"),
            include_str!("../ui/themes/Catppuccin.json"),
            include_str!("../ui/themes/Nord.json"),
            include_str!("../ui/themes/Ristretto.json"),
            include_str!("../ui/themes/Rose Pine Dawn.json"),
            include_str!("../ui/themes/Rose Pine Moon.json"),
            include_str!("../ui/themes/Rose Pine.json"),
            include_str!("../ui/themes/Tokyo Night.json"),
        ]
        .into_iter()
        .map(parse)
        .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_reference_palettes_have_readable_bubbles_and_bounded_doodles() {
        assert_eq!(palettes().len(), crate::preferences::THEMES.len() - 3);
        for palette in palettes() {
            let c = &palette.colors;
            for background in ["bubble_in", "bubble_out"] {
                assert!(contrast(c["text"], c[background]) >= 4.5);
            }
            assert!(contrast(c["outgoing_secondary"], c["bubble_out"]) >= 4.5);
            assert!((25..=102).contains(&(c["wallpaper_ink"] >> 24)));
        }
        assert_eq!(palettes()[2].colors["panel"], 0x333945);
        assert!(!palettes()[0].dark);
    }
}
