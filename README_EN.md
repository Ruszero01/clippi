<div align="center">
  <img src="assets/LOGO_notext.png" width="96" alt="Clippi logo">

  # Clippi

  **Simple · Lightweight · Open Source** native clipboard manager<br>
  Rust + GPUI · Windows / macOS

  <p>
    <a href="https://clippi.rains-ailurus.cn/"><img src="https://img.shields.io/badge/Website-Visit-2EA44F?style=for-the-badge" alt="Website"></a>
    <a href="https://clippi.rains-ailurus.cn/docs"><img src="https://img.shields.io/badge/Docs-User%20Guide-0969DA?style=for-the-badge" alt="Docs"></a>
    <a href="https://github.com/Ruszero01/clippi/releases"><img src="https://img.shields.io/github/downloads/Ruszero01/clippi/total?style=for-the-badge&amp;label=Download&amp;color=F97316&amp;logo=github&amp;logoColor=white" alt="Download the latest release and view total downloads"></a>
  </p>

  <p>
    <img src="https://img.shields.io/badge/Rust-2021-000000?style=flat&amp;logo=rust&amp;logoColor=white" alt="Rust 2021">
    <img src="https://img.shields.io/badge/GPUI-0.2.2-237a55?style=flat" alt="GPUI 0.2.2">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-007EC6?style=flat" alt="Windows / macOS">
  </p>

  <a href="README.md">中文</a> · <strong>English</strong>
</div>

![Clippi interface](docs/images/UI.png)

## Features

- **Capture & organize**: save text, rich text, images, files, and more. Search, tag, favorite, annotate, and manage entries in batches.
- **Quick paste**: summon Clippi with a global hotkey; the quick window preserves your typing focus. Hold Alt to see and use switching shortcuts.
- **Content recognition**: extract text with image OCR and recognize QR codes.
- **Cross-device sync**: sync text entries and tags through OneDrive, iCloud folders, or WebDAV, with multiple backends supported.
- **Everyday tools**: an entry editor, emoji and symbol panel, and file transfer station.
- **Appearance & privacy**: light/dark themes, custom fonts and accent colors, masked previews, and local backup and restore.

## Download & use

Download from the [website](https://clippi.rains-ailurus.cn/) or [GitHub Releases](https://github.com/Ruszero01/clippi/releases). Press `Alt+V` to open the main window by default. See the [user guide](https://clippi.rains-ailurus.cn/docs) for details.

- **macOS**: the app is unsigned. Allow it in **System Settings → Privacy & Security** after installation or updates, and grant **Accessibility** permission for quick paste.
- **Windows**: to paste into elevated applications, Clippi must also run as administrator. Change administrator mode in **Settings → Main → Startup**, then restart Clippi.

## Run from source

Install [Rust](https://www.rust-lang.org/tools/install) and your platform's build tools, then run:

```bash
git clone https://github.com/Ruszero01/clippi.git
cd clippi
cargo run --release
```

## Contributor wall

Thanks to everyone who contributes!

<a href="https://github.com/Ruszero01/clippi/graphs/contributors">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/contributors-dark.svg">
    <img src="docs/images/contributors.svg" alt="Clippi contributors" width="840">
  </picture>
</a>

<p align="center">
  <a href="https://github.com/Ruszero01/clippi/issues"><img src="https://img.shields.io/badge/Contribute-237a55?style=flat&amp;logo=github&amp;logoColor=white" alt="Contribute and report issues" height="24"></a>
  <a href="RELEASES.md"><img src="https://img.shields.io/badge/Changelog-237a55?style=flat&amp;logo=readthedocs&amp;logoColor=white" alt="Changelog" height="24"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/MIT%20License-237a55?style=flat&amp;logo=opensourceinitiative&amp;logoColor=white" alt="MIT License" height="24"></a>
</p>
