//! Standalone raster experiment: change hinting and surface coverage independently.
//! Build against Heya's pinned skia-safe; see docs/text-rendering.md.
use skia_safe::{self as sk, Color, FontHinting, ImageInfo, SurfaceProps};
fn main() {
    let directory = std::env::args_os()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("output directory");
    std::fs::create_dir_all(&directory).unwrap();
    let manager = sk::FontMgr::new();
    let face = manager
        .new_from_data(
            sk::Data::new_copy(&std::fs::read("/usr/share/fonts/inter/Inter.ttc").unwrap()),
            Some(0),
        )
        .unwrap();
    let default = SurfaceProps::default();
    println!(
        "Defaults: {default:?}; {:?}",
        sk::Font::new(face.clone(), 14.5)
    );
    for (name, props) in [
        ("default", default),
        (
            "linear",
            SurfaceProps::new_with_text_properties(Default::default(), Default::default(), 0., 1.),
        ),
        (
            "contrast0",
            SurfaceProps::new_with_text_properties(
                Default::default(),
                Default::default(),
                0.,
                default.text_gamma(),
            ),
        ),
        (
            "gamma1",
            SurfaceProps::new_with_text_properties(
                Default::default(),
                Default::default(),
                default.text_contrast(),
                1.,
            ),
        ),
    ] {
        for (hint, hinting) in [
            ("none", FontHinting::None),
            ("slight", FontHinting::Slight),
            ("normal", FontHinting::Normal),
            ("full", FontHinting::Full),
        ] {
            let info = ImageInfo::new_n32_premul((840, 150), sk::ColorSpace::new_srgb());
            let mut surface = sk::surfaces::raster(&info, None, Some(&props)).unwrap();
            surface.canvas().clear(Color::WHITE);
            let mut paint = sk::Paint::default();
            paint.set_color(Color::from_rgb(17, 27, 33));
            for (size, y) in [(14.5, 40.), (21.75, 95.)] {
                let mut font = sk::Font::new(face.clone(), size);
                font.set_subpixel(true);
                font.set_hinting(hinting);
                surface.canvas().draw_str(
                    "A little room for better conversations. Salut, buna dimineata!",
                    (20., y),
                    &font,
                    &paint,
                );
            }
            let data = surface
                .image_snapshot()
                .encode(None, sk::EncodedImageFormat::PNG, None)
                .unwrap();
            std::fs::write(
                directory.join(format!("skia-{name}-{hint}.png")),
                data.as_bytes(),
            )
            .unwrap();
        }
    }
}
