fn main() {
    slint_build::compile("ui/app.slint").unwrap();

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/LOGO.ico");
        res.set("FileDescription", "Clippi - Clipboard Manager");
        res.set("ProductName", "Clippi");
        res.set("OriginalFilename", "clippi.exe");
        res.compile().expect("Failed to compile Windows resources");
    }
}
