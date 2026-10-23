fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/platform/macos.m");
        println!("cargo:rerun-if-changed=src/platform/macos_audio.m");
        println!("cargo:rerun-if-changed=src/platform/macos_call_audio.m");
        println!("cargo:rerun-if-changed=src/platform/macos_call_video.m");
        println!("cargo:rerun-if-changed=src/platform/macos_video.m");
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
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    let minimum = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "11.0".into());
    cc::Build::new()
        .file("src/platform/macos.m")
        .file("src/platform/macos_audio.m")
        .file("src/platform/macos_call_audio.m")
        .file("src/platform/macos_call_video.m")
        .file("src/platform/macos_video.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag(format!("-mmacosx-version-min={minimum}"))
        .compile("heya_macos");
    println!("cargo:rustc-link-arg=-mmacosx-version-min={minimum}");
    for framework in [
        "AppKit",
        "CoreGraphics",
        "CoreText",
        "ImageIO",
        "Foundation",
        "UniformTypeIdentifiers",
        "AVFoundation",
        "VideoToolbox",
        "AudioToolbox",
        "AudioUnit",
        "CoreAudio",
        "CoreVideo",
        "CoreMedia",
        "CoreImage",
        "QuartzCore",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

#[cfg(not(target_os = "macos"))]
fn cc_macos() {
    panic!("Build macOS on a macOS host with Apple's SDK installed");
}
