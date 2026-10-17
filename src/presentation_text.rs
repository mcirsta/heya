//! Shared display rules; the original message and selection offsets stay intact.
use unicode_segmentation::UnicodeSegmentation;

/// Match the reference's 2.4× body size for one to three complete emoji.
/// Graphemes keep flags, skin tones and ZWJ sequences together. Whitespace is
/// allowed, but text-presentation selectors and ordinary symbols stay text.
pub fn large_emoji(text: &str) -> bool {
    let mut count = 0;
    for cluster in text
        .graphemes(true)
        .filter(|g| !g.chars().all(char::is_whitespace))
    {
        if cluster.contains('\u{fe0e}') || emojis::get(cluster).is_none() {
            return false;
        }
        count += 1;
        if count > 3 {
            return false;
        }
    }
    count != 0
}

// Called by the single shared Slint test thread.
#[cfg(test)]
pub(crate) fn check_ui() {
    use crate::{AppWindow, Theme, fixtures, presentation};
    use i_slint_backend_testing::ElementHandle;
    use slint::{ComponentHandle, Model};
    let ui = AppWindow::new().unwrap();
    ui.set_demo_mode(true);
    ui.set_show_conversation(true);
    let mut demo = fixtures::demo();
    demo.select(13);
    let view = demo.conversation();
    for dark in [false, true] {
        ui.global::<Theme>().set_dark(dark);
        for width in [1200., 520., 800., 1200.] {
            ui.window().set_size(slint::LogicalSize::new(width, 1000.));
            presentation::conversation(&ui, &view, true);
            ui.show().unwrap();
            ui.invoke_layout_timeline();
            let rows = ui.get_messages();
            for (i, message) in view.messages.iter().enumerate() {
                let row = rows.row_data(i).unwrap();
                assert_eq!(
                    row.text, message.text,
                    "display must retain copy/selection text"
                );
                assert_eq!(row.large_emoji, matches!(i, 1 | 2));
            }
            let mut heights = Vec::new();
            for item in ElementHandle::find_by_element_type_name(&ui, "TimelineItem")
                .filter(|e| e.id().as_deref() != Some("AppWindow::measure-row"))
            {
                let bubble = item
                    .query_descendants()
                    .match_id("MessageBubble::bubble")
                    .find_first()
                    .unwrap();
                let body = item
                    .query_descendants()
                    .match_id("MessageBubble::body")
                    .find_first()
                    .unwrap();
                let stamp = item
                    .query_descendants()
                    .match_id("MessageBubble::stamp")
                    .find_first()
                    .unwrap();
                for element in [&body, &stamp] {
                    let p = element.absolute_position();
                    let b = bubble.absolute_position();
                    assert!(p.x >= b.x && p.y >= b.y);
                    assert!(p.x + element.size().width <= b.x + bubble.size().width + 0.5);
                    assert!(p.y + element.size().height <= b.y + bubble.size().height + 0.5);
                }
                heights.push(body.size().height);
            }
            assert_eq!(heights.len(), 6);
            assert!(
                heights[1] > heights[0] * 2.,
                "emoji-only body must use 2.4× text"
            );
            assert!(heights[5] < heights[1], "four emoji keep the ordinary size");
            assert!(ui.get_timeline_rows().row_count() <= 6);
        }
    }
    ui.hide().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real shaper/font routing, without a GUI or a renderer. These fonts are
    /// development prerequisites on Linux; other platforms use native fonts.
    #[cfg(target_os = "linux")]
    #[test]
    fn emoji_fallback_does_not_replace_numbers_or_text_presentation() {
        use i_slint_core::textlayout::sharedparley::{fontique, parley};
        let mut fonts = parley::FontContext::new();
        let mut query = fonts.collection.query(&mut fonts.source_cache);
        query.set_families([fontique::QueryFamily::Named("Noto Color Emoji")]);
        let mut emoji_font = None;
        query.matches_with(|font| {
            emoji_font = Some(font.blob.id());
            fontique::QueryStatus::Stop
        });
        let emoji_font = emoji_font.expect("Noto Color Emoji is a development prerequisite");
        drop(query);
        let mut context = parley::LayoutContext::<()>::new();
        for (text, expect_color) in [
            ("Latin 0123456789 # *", false),
            ("❤\u{fe0e}", false),
            ("😟", true),
            ("❤️", true),
            ("👩🏽‍💻", true),
            ("🇷🇴", true),
            ("#️⃣", true),
            ("1️⃣", true),
        ] {
            let mut builder = context.ranged_builder(&mut fonts, text, 1., false);
            builder.push_default(parley::StyleProperty::FontFamily(
                parley::FontFamily::Single(parley::style::FontFamilyName::named("Inter")),
            ));
            let mut layout = builder.build(text);
            layout.break_all_lines(None);
            let mut runs = 0;
            for line in layout.lines() {
                for item in line.items() {
                    if let parley::PositionedLayoutItem::GlyphRun(glyphs) = item {
                        assert_eq!(
                            glyphs.run().font().data.id() == emoji_font,
                            expect_color,
                            "{text:?}"
                        );
                        assert!(
                            glyphs.glyphs().all(|glyph| glyph.id != 0),
                            "missing glyph in {text:?}"
                        );
                        runs += 1;
                    }
                }
            }
            assert!(runs > 0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_emoji_baseline_preserves_ordinary_text_and_horizontal_spacing() {
        use i_slint_core::textlayout::sharedparley::{fontique, parley};
        let mut fonts = parley::FontContext::new();
        let mut query = fonts.collection.query(&mut fonts.source_cache);
        query.set_families([fontique::QueryFamily::Named("Apple Color Emoji")]);
        let mut emoji_data = None;
        query.matches_with(|font| {
            emoji_data = Some(font.blob.clone());
            fontique::QueryStatus::Stop
        });
        let emoji_data = emoji_data.expect("macOS must provide Apple Color Emoji");
        drop(query);
        let mut context = parley::LayoutContext::<()>::new();
        let mut emoji_runs = 0;
        for size in [14.5, 34.8, 69.6] {
            for text in [
                "Latin 012 # *",
                "😟",
                "Hello 😟 world ❤️",
                "👩🏽‍💻🇷🇴1️⃣",
                "😟😟😟😟",
            ] {
                let mut builder = context.ranged_builder(&mut fonts, text, 1., false);
                builder.push_default(parley::StyleProperty::FontSize(size));
                builder.push_default(parley::StyleProperty::FontFamily(
                    parley::FontFamily::Single(parley::style::FontFamilyName::Generic(
                        fontique::GenericFamily::SystemUi,
                    )),
                ));
                let mut layout = builder.build(text);
                layout.break_all_lines(None);
                for item in layout.lines().flat_map(|line| line.items()) {
                    let parley::PositionedLayoutItem::GlyphRun(glyphs) = item else {
                        continue;
                    };
                    let emoji = glyphs.run().font().data.id() == emoji_data.id();
                    let mut x = glyphs.offset();
                    for (raw, positioned) in glyphs.glyphs().zip(glyphs.positioned_glyphs()) {
                        assert_ne!(raw.id, 0, "missing glyph in {text:?}");
                        assert!((positioned.x - raw.x - x).abs() < 0.001);
                        x += raw.advance;
                        let below_baseline = positioned.y - raw.y - glyphs.baseline();
                        if emoji {
                            assert!(
                                below_baseline > 0.
                                    && below_baseline < glyphs.run().metrics().descent
                            );
                        } else {
                            assert_eq!(below_baseline, 0., "ordinary text must not move");
                        }
                    }
                    emoji_runs += usize::from(emoji);
                }
            }
        }
        assert!(emoji_runs > 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_emoji_keeps_the_text_baseline_grid_across_wrapped_lines() {
        use i_slint_core::textlayout::sharedparley::{fontique, parley};
        let mut fonts = parley::FontContext::new();
        let mut context = parley::LayoutContext::<()>::new();
        let mut shape = |text: &str, size, scale, ratio, width| {
            let mut builder = context.ranged_builder(&mut fonts, text, scale, false);
            builder.push_default(parley::StyleProperty::FontSize(size));
            builder.push_default(parley::StyleProperty::FontFamily(
                parley::FontFamily::Single(parley::style::FontFamilyName::Generic(
                    fontique::GenericFamily::SystemUi,
                )),
            ));
            if let Some(ratio) = ratio {
                // Match Slint: all fallback runs use the requested text font's
                // natural line-height ratio, including emoji-only lines.
                builder.push_default(parley::StyleProperty::LineHeight(
                    parley::style::LineHeight::FontSizeRelative(ratio),
                ));
            }
            let mut layout = builder.build(text);
            layout.break_all_lines(width);
            layout
        };
        for size in [14.5, 34.8] {
            for scale in [1., 2.] {
                let reference = shape("First line", size, scale, None, None);
                let first_line = reference.lines().next().unwrap();
                let metrics = first_line.metrics();
                let ratio = metrics.line_height / (size * scale);
                let wrap_width = metrics.advance + size * scale * 0.4;
                for (text, width) in [
                    ("First line\n😅", None),
                    ("First line\n😅 followed by text", None),
                    ("First line 😅\nSecond line", None),
                    ("First line\n😅\nFollowing line", None),
                    ("😅\n😅", None),
                    ("First line 😅 followed by text", Some(wrap_width)),
                ] {
                    let layout = shape(text, size, scale, Some(ratio), width);
                    assert!(layout.len() > 1, "fixture must span multiple lines");
                    for (index, line) in layout.lines().enumerate() {
                        let expected = metrics.baseline + index as f32 * metrics.line_height;
                        assert!(
                            (line.metrics().baseline - expected).abs() < 0.06,
                            "emoji changed baseline at line {index}, size {size}, scale {scale}, text {text:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn complete_emoji_sequences_get_the_reference_size() {
        for text in ["😟", "😂 🙏", "👩🏽‍💻🇷🇴#️⃣", "  👨‍👩‍👧‍👦\n❤️ ", "👍🏽"]
        {
            assert!(large_emoji(text), "{text:?}");
        }
        for text in [
            "",
            " \n",
            "Hi 😟",
            "😟!",
            "😟😟😟😟",
            "123",
            "#",
            "e\u{301}",
            "❤\u{fe0e}",
        ] {
            assert!(!large_emoji(text), "{text:?}");
        }
    }
}
