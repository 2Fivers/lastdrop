fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/flyout.slint", config).expect("slint compile");

    // Embed an application manifest (per-monitor v2 DPI awareness, Windows 10/11 supportedOS).
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
}
