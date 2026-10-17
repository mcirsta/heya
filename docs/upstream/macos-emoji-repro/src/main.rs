//! Offscreen comparison; no Slint, Parley, account data or desktop window.
use skia_safe::{Color, Data, EncodedImageFormat, Font, FontMgr, FontStyle, Paint, Point, Rect};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        let ttc = "/System/Library/Fonts/Apple Color Emoji.ttc";
        if std::path::Path::new(ttc).exists() {
            ttc.into()
        } else {
            "/System/Library/Fonts/Apple Color Emoji.ttf".into()
        }
    });
    let output = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("macos-emoji-repro.png"));
    let mgr = FontMgr::new();
    let native = mgr
        .match_family_style("Apple Color Emoji", FontStyle::normal())
        .unwrap();
    let bytes = std::fs::read(path)?;
    let from_data = mgr.new_from_data(Data::new_copy(&bytes), Some(0)).unwrap();
    let mut surface = skia_safe::surfaces::raster_n32_premul((720, 260)).unwrap();
    surface.canvas().clear(Color::WHITE);
    let mut ink = Paint::default();
    ink.set_color(Color::BLACK);
    let mut guide = Paint::default();
    guide.set_color(Color::from_rgb(210, 210, 210));
    println!(
        "Left: native family; right: same font loaded from bytes. Gray lines mark identical baselines."
    );
    for (row, size) in [14.5, 34.8, 69.6].into_iter().enumerate() {
        let baseline = 40. + row as f32 * 90.;
        surface
            .canvas()
            .draw_line((20., baseline), (700., baseline), &guide);
        for (column, (name, face)) in [("native", &native), ("from-data", &from_data)]
            .into_iter()
            .enumerate()
        {
            let mut font = Font::from_typeface(face.clone(), size);
            font.set_subpixel(true);
            let ids = font.str_to_glyphs_vec("😟");
            let mut bounds = vec![Rect::default(); ids.len()];
            let mut widths = vec![0.; ids.len()];
            font.get_bounds(&ids, &mut bounds, None);
            font.get_widths(&ids, &mut widths);
            println!(
                "{name} size={size} family={} glyphs={ids:?} widths={widths:?} bounds={bounds:?}",
                face.family_name()
            );
            let positions = [Point::new(40. + column as f32 * 340., baseline)];
            surface.canvas().draw_glyphs_at(
                &ids,
                skia_safe::canvas::GlyphPositions::Points(&positions),
                Point::default(),
                &font,
                &ink,
            );
        }
    }
    let png = surface
        .image_snapshot()
        .encode(None, EncodedImageFormat::PNG, 100)
        .unwrap();
    std::fs::write(&output, png.as_bytes())?;
    println!("Saved {}", output.display());
    Ok(())
}
