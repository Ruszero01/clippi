<div align="center">
  <p>
    <img src="assets/LOGO_notext.png" width="120" alt="Clippi Logo">
  </p>

  # Clippi

  **Simple · Lightweight · Open Source** Native Clipboard Manager<br>
  Built with Rust + GPUI, available for Windows and macOS

  <p>
    <a href="https://clippi.rains-ailurus.cn/">🌐 Official Website</a> ·
    <a href="https://github.com/Ruszero01/clippi/releases">⬇️ Download</a> ·
    <a href="https://github.com/Ruszero01/clippi">GitHub</a>
  </p>

  <p>
    <a href="README.md">中文</a> · <a href="README_EN.md">English</a>
  </p>

  <p>
    <a href="https://github.com/Ruszero01/clippi/issues">Issues</a> ·
    <a href="https://github.com/Ruszero01/clippi/releases">Changelog</a>
  </p>

  <p>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/Rust-2021-%23000000?logo=rust" alt="Rust">
    <img src="https://img.shields.io/badge/GPUI-0.2-%23555555?logo=rust" alt="GPUI">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue" alt="Platform">
    <a href="https://deepwiki.com/Ruszero01/clippi"><img src="https://deepwiki.com/badge.svg" alt="Ask DeepWiki"></a>
  </p>
</div>

![UI](./docs/images/UI.png)

---

> Clippi is a lightweight, open-source clipboard history manager: it silently records everything you copy, then helps you search, filter, OCR images, detect QR codes, batch-process entries, and sync across devices — turning copied content into an organized, reusable, searchable asset.

---

## Why Clippi?

- **No WebView dependency**: a native Rust + GPUI interface with low idle memory usage and high performance
- **Multi-tag system**: add multiple tags to entries and use versatile tag filtering modes
- **Dual-window mode**: a focus-free quick-paste window never interrupts your current input
- **Multi-backend sync architecture**: OneDrive / iCloud / WebDAV and other sync methods
- **Privacy protection**: masked previews for sensitive content such as email addresses and phone numbers

## Features

### Clipboard Monitoring

![clipboard](./docs/images/clipboard.png)

- Multi-format content detection: plain text, rich text, files, images, links, paths, colors, phone numbers, email addresses
- Content hash deduplication: re-copying the same content updates the timestamp without creating duplicate entries
- Color normalization dedup: `#FF8000` ≡ `rgb(255,128,0)`, prevents duplicates
- Image OCR: keyword search and paste OCR-extracted text
- QR code detection: recognizes QR codes with one-click navigation
- Hotkey blacklist: disable global hotkeys in specified applications
- Plain text copy mode: discard rich formatting and keep plain text only

### Content Management

![content1](./docs/images/content.png)

- Double-click cards for quick paste
- Multi-type entry editing
- Favorites & notes: pin favorite snippets and add notes to entries
- Multi-select batch operations: batch paste (newline-separated), batch favorite, batch delete, batch tag
- Combined type filters: freely mix multiple filter rules
- Keyword search — matches both text content and tag names
- Tag filtering — switchable AND/OR logic across multiple tags
- Sorting: by creation time / by last used time
- Sensitive info preview masking: email shows first 2 chars + domain, phone shows first 3 + last 4 digits

### Tag System

![tags](./docs/images/tags.png)

- Create / Edit / Delete tags, 12 preset colors
- Tag association with clipboard entries (many-to-many)
- Side tag bar: pin filter tags to the left side of the window, with expand/collapse animation and pinning
- Tag filter panel + tag picker panel (both support tag CRUD)
- Single-item / batch tag assignment and removal
- Cross-device tag synchronization (with color conflict resolution)

### Window & Interaction

![hotkey](./docs/images/hotkey.png)

- Global hotkey to show/hide (default `Alt+V`, supports custom recording)
- Window pin-on-top mode
- Auto-hide on focus loss
- Multi-monitor support (cursor's monitor)
- Three popup positions: center / follow mouse / remember position
- Dark / light theme with automatic system dark mode detection

### Display Options

![display](./docs/images/display.png)

- Source app info display (clipboard source application name and icon)
- Card height modes: tall / medium / short / auto
- Show original content on hover (when notes exist)
- Plain text copy

### Cloud Sync

![sync](./docs/images/sync.png)

- Multi-backend architecture: supports multiple sync services simultaneously, each with independent toggle and interval
- Local folder backend: sync via OneDrive / iCloud folders
- WebDAV backend: supports WebDAV servers, ETag caching + Basic Auth
- Auto-detect OneDrive (Windows + macOS) and iCloud (macOS) preset paths
- Cross-device delete & unfavorite propagation (tombstone mechanism, 30-day window)
- Last-writer-wins (LWW) conflict resolution
- Semantic hash comparison, skip unchanged pushes (prevent sync loops)
- Automatic conflict file merging and cleanup
- Configurable sync interval (30s / 1min / 10min / 30min) + manual instant sync
- Favorites-only sync mode
- Async connection test

## Quick Start

### Download

- Get the latest version from the [Official Website](https://clippi.rains-ailurus.cn/) or [GitHub Releases](https://github.com/Ruszero01/clippi/releases)
- Installers / DMGs are available for Windows and macOS

### Build from Source

```bash
git clone https://github.com/Ruszero01/clippi.git
cd clippi
cargo build
cargo run
```

---

## macOS Users Notice

Clippi is not signed with an Apple Developer certificate (not enrolled in Apple Developer Program). On first launch or after each update, macOS Gatekeeper may block the app from running. Please follow the steps below:

### First Install / After Update

1. After downloading the `.dmg`, drag Clippi into the `Applications` folder
2. Double-click Clippi to open, then select **"Keep"** in the security dialog
3. Go to **System Settings → Privacy & Security**, scroll to the bottom and click **"Open Anyway"** (required once per update)

### Grant Accessibility Permission (Required for Quick Paste)

Clippi's quick paste feature requires Accessibility permission to simulate keystrokes:

1. Open **System Settings → Privacy & Security → Accessibility**
2. Find **Clippi** in the list and enable the toggle
3. If Clippi is not in the list, click the `+` button and add it manually from `/Applications/Clippi.app`

> Without Accessibility permission, quick paste (double-click card / Enter key paste) will not work, but you can still copy and paste manually via the right-click menu.

---

## Links

- [Official Website](https://clippi.rains-ailurus.cn/)
- [Official Docs](https://clippi.rains-ailurus.cn/docs.html)
- [GitHub Repository](https://github.com/Ruszero01/clippi)
- [GitHub Releases](https://github.com/Ruszero01/clippi/releases)
- [Issue Tracker](https://github.com/Ruszero01/clippi/issues)

## License

[MIT](LICENSE)
