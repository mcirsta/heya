//! Picker coverage through the same font and shaper used by Slint on Linux.
use i_slint_core::textlayout::sharedparley::{fontique, parley};
use std::{collections::HashSet, sync::OnceLock};

static EMOJI_SUPPORT: OnceLock<HashSet<&'static str>> = OnceLock::new();

pub fn prepare_emoji_support() {
    EMOJI_SUPPORT.get_or_init(|| {
        let mut values = Vec::new();
        for emoji in emojis::iter() {
            if let Some(tones) = emoji.skin_tones() {
                values.extend(tones.map(|e| e.as_str()));
            } else {
                values.push(emoji.as_str());
            }
        }
        supported_emojis(values)
    });
}

pub fn supports_emoji(emoji: &str) -> bool {
    prepare_emoji_support();
    EMOJI_SUPPORT.get().unwrap().contains(emoji)
}

fn supported_emojis(values: impl IntoIterator<Item = &'static str>) -> HashSet<&'static str> {
    let mut fonts = parley::FontContext::new();
    let mut query = fonts.collection.query(&mut fonts.source_cache);
    query.set_families([fontique::QueryFamily::Named("Noto Color Emoji")]);
    let mut emoji_font = None;
    query.matches_with(|font| {
        emoji_font = Some(font.blob.id());
        fontique::QueryStatus::Stop
    });
    drop(query);
    let Some(emoji_font) = emoji_font else {
        return HashSet::new();
    };
    let mut context = parley::LayoutContext::<()>::new();
    let mut width = |text: &str| {
        let mut builder = context.ranged_builder(&mut fonts, text, 1., false);
        builder.push_default(parley::StyleProperty::FontSize(24.));
        builder.push_default(parley::StyleProperty::FontFamily(
            parley::FontFamily::Single(parley::style::FontFamilyName::named("Noto Color Emoji")),
        ));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        let mut count = 0;
        for item in layout.lines().flat_map(|line| line.items()) {
            if let parley::PositionedLayoutItem::GlyphRun(glyphs) = item {
                if glyphs.run().font().data.id() != emoji_font {
                    return None;
                }
                for glyph in glyphs.glyphs() {
                    if glyph.id == 0 {
                        return None;
                    }
                    count += 1;
                }
            }
        }
        (count > 0 && layout.len() == 1).then_some(layout.width())
    };
    let Some(cell_width) = width("😀").filter(|w| *w > 0.) else {
        return HashSet::new();
    };
    values
        .into_iter()
        // Joined glyphs may overlap in one cell; requiring one glyph would
        // incorrectly discard some supported skin-tone sequences.
        .filter(|text| width(text).is_some_and(|w| w > 0. && w <= cell_width * 1.5))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn retains_complete_sequences_and_rejects_missing_or_split_glyphs() {
        let supported = ["😀", "👩🏽‍💻", "🫱🏻‍🫲🏼", "🇷🇴", "1️⃣", "👨‍👩‍👧‍👦"];
        let rejected = ["", "\u{10ffff}", "😀😀", "😀\n😀", "Latin"];
        let choices = super::supported_emojis(supported.into_iter().chain(rejected));
        for emoji in supported {
            assert!(choices.contains(emoji), "Missing installed emoji: {emoji}");
            assert!(
                super::supports_emoji(emoji),
                "Missing cached emoji: {emoji}"
            );
        }
        for text in rejected {
            assert!(!choices.contains(text), "Invalid picker choice: {text:?}");
        }
    }
}
