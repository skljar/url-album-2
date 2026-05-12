# URL Album 2

Portable desktop bookmark manager. Modern rewrite of the classic Win32 URL Album application.

Built with **Tauri 2** · **Rust** · **Vanilla JS** · **SQLite**

---

## Features

- **Tree navigation** — folder hierarchy with accordion expand, inline rename, drag-free reorder
- **Screenshot previews** — headless Edge/Chrome captures stored in `Data/` next to the exe
- **Link checker** — concurrent HTTP checks with pause/resume, sortable/resizable results table
- **Duplicate finder** — two-panel utility, delete selected or keep-one-per-group
- **Import** — ua.dat (Windows-1251), Netscape HTML, TXT (URL per line), JSON sync, browser bookmarks (Chrome/Edge/Opera/Firefox via places.sqlite)
- **Export** — Netscape HTML, plain TXT, JSON sync file
- **Toolbar** — compact Win32-style, fully customizable via drag-and-drop dialog
- **Settings** — theme, accordion tree, confirm-on-delete, duplicate guard, proxy, screenshot size
- **Browser manager** — Open With list, portable browser support, auto-detect installed browsers

## Portable architecture

Everything lives next to `url-album.exe`:

```
url-album.exe
album.db          ← SQLite database (created on first run)
album.db-shm
album.db-wal
Data/             ← screenshot PNGs
browsers.json     ← browser list
toolbar.json      ← toolbar layout
settings.json     ← app settings
```

Move the folder anywhere — the app just works.

## Building

Requires: [Rust](https://rustup.rs) · [Node.js](https://nodejs.org) · [Tauri CLI v2](https://tauri.app)

```bash
# Development
cargo tauri dev

# Release build
cd src-tauri
cargo build --release
# → src-tauri/target/release/url-album.exe
```

## Stack

| Layer | Technology |
|-------|-----------|
| Shell | Tauri 2 (WebView2) |
| Backend | Rust — rusqlite (bundled SQLite), rfd 0.15, reqwest 0.12 |
| Frontend | Vanilla JS, no framework |
| Dialogs | rfd AsyncFileDialog with parent window (no dialogs behind main window) |

## Keyboard shortcuts

| Key | Action |
|-----|--------|
| `Ctrl+F` | Search |
| `Ctrl+N` | New folder (root level) |
| `Ctrl+Shift+N` | New link |
| `F2` | Rename selected folder |
| `F4` | Properties |
| `Del` | Delete |
| `Enter` | Open link in browser |
| `Esc` | Close dialog / clear search |
