fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).unwrap();

    // Ensure rebuilds when translation files change.
    println!("cargo:rerun-if-changed=lang");

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/LOGO.ico");
        res.set("FileDescription", "Clippi");
        res.set("ProductName", "Clippi");
        res.set("OriginalFilename", "clippi.exe");
        res.compile().expect("Failed to compile Windows resources");
    }

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=Vision");
        // Compile the native ObjC OCR helper
        cc::Build::new()
            .file("src/platform/ocr_helper.m")
            .flag("-fobjc-arc")
            .compile("ocr_helper");
    }
}
