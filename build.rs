fn main() {
    // Ensure rebuilds when translation files change.
    println!("cargo:rerun-if-changed=lang");

    // Re-run when anything that changes the build identity changes, so
    // `build=<sha>-dirty` and `built=<time>` in the startup log always describe
    // the binary that is actually running.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    for git_file in [".git/HEAD", ".git/index"] {
        if std::path::Path::new(git_file).exists() {
            println!("cargo:rerun-if-changed={git_file}");
        }
    }
    embed_build_identity();

    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/LOGO_notext.ico");
        res.set("FileDescription", "Clippi");
        res.set("ProductName", "Clippi");
        res.set("OriginalFilename", "clippi.exe");
        res.compile().expect("Failed to compile Windows resources");
    }

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=Vision");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        // Compile the native ObjC OCR helper
        cc::Build::new()
            .file("src/platform/ocr_helper.m")
            .flag("-fobjc-arc")
            .compile("ocr_helper");
    }
}

/// Embed the source tree identity and compile time into the binary.
///
/// `startup: clippi … build=… built=…` is what lets a user report and its log
/// be tied to an exact binary: the old log only carried the package version
/// (0.5.3), so a locally rebuilt debug binary and a release build were
/// indistinguishable. Best-effort: without git the fields fall back to
/// `unknown`, and the build never fails because of it.
fn embed_build_identity() {
    let mut identity = "unknown".to_string();
    if let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !sha.is_empty() {
                let dirty = std::process::Command::new("git")
                    .args(["status", "--porcelain"])
                    .stderr(std::process::Stdio::null())
                    .output()
                    .map(|out| !out.stdout.is_empty())
                    .unwrap_or(false);
                identity = if dirty { format!("{sha}-dirty") } else { sha };
            }
        }
    }

    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);

    println!("cargo:rustc-env=CLIPPI_BUILD_ID={identity}");
    println!("cargo:rustc-env=CLIPPI_BUILD_EPOCH={epoch}");
}
