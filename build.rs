fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/platform/macos.m");
        cc_macos();
    }
    slint_build::compile_with_config(
        "ui/app.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent".into())
            .with_debug_info(cfg!(debug_assertions)),
    )
    .expect("Slint UI must compile");
}

#[cfg(target_os = "macos")]
fn cc_macos() {
    cc::Build::new()
        .file("src/platform/macos.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=11.0")
        .compile("heya_macos");
    println!("cargo:rustc-link-arg=-mmacosx-version-min=11.0");
    for framework in [
        "AppKit",
        "CoreGraphics",
        "ImageIO",
        "Foundation",
        "UniformTypeIdentifiers",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

#[cfg(not(target_os = "macos"))]
fn cc_macos() {
    panic!("Build macOS on a macOS host with Apple's SDK installed");
}
