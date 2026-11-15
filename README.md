# ReplayViewer

A fast, lightweight, cross-platform screenshot, annotation, visual clipboard, and hardware-accelerated replay buffer desktop application built with **Tauri v2, Rust, SQLite (WAL + FTS5), React 19, TypeScript, Tailwind CSS v4, and daisyUI v5**.

Engineered to replace bloated, laggy utilities like Windows Xbox Game Bar while maintaining **< 50 MB idle RAM** and **60 FPS scrolling & sorting across 10,000+ captures**.

---

## Key Engineering Highlights

* **Zero-Base64 Uncompressed BMP Capture Streaming (`< 3ms` Freeze):**
  Instead of serializing 33 MB 4K screenshots as Base64 strings over JSON IPC (or spending 250ms compressing PNGs in debug builds), Rust freezes the active monitor buffer via `xcap` as an uncompressed BMP in `< 3ms` *before* showing the pre-warmed `overlay` window (`< 16ms` wakeup) and streams raw bytes via `http://capture.localhost/current` (Windows) / `capture://localhost/current` (macOS/Linux).
* **Frameless daisyUI v5 Interface + 10 Live Themes:**
  Custom frameless desktop window (`decorations: false`) with an integrated daisyUI v5 draggable titlebar, window controls (Minimize, Maximize/Restore, Close to Tray), and live theme switching (`night`, `dark`, `dim`, `dracula`, `synthwave`, `cyberpunk`, `sunset`, `business`, `nord`, `light`).
* **100% Anti-Cheat Safe & Exclusive Fullscreen Friendly (`v1.0`):**
  Uses OS-sanctioned desktop compositor APIs (zero DLL/process injection, safe for Vanguard, Easy Anti-Cheat, and BattlEye). Includes **`Alt + Shift + S` (Instant Fullscreen Capture)** to capture and copy to the clipboard in `< 10ms` without spawning an overlay window or Alt-Tabbing Exclusive Fullscreen games.
* **SQLite WAL Mode + FTS5 Search + B-Tree Indexed Sorting:**
  Embedded `rusqlite` database running with `PRAGMA journal_mode = WAL`, an `FTS5` inverted index (`captures_fts`) for `< 5ms` search across tags, groups, source apps, and OCR text, and B-Tree indexes (`created_at`, `size_bytes`, `last_copied_at`) for `< 1ms` forward/reverse sorting (`DESC ↓` / `ASC ↑`).
* **Virtualized 60 FPS Grid, Marquee Multi-Select & Batch Actions (`@tanstack/react-virtual`):**
  Windowed grid with a live **1–8 Column Card Resizer**, **rubber-band marquee drag-to-select** (even starting directly on image cards), and batch operations to **Group**, **Tag**, **Delete**, or **Move to an External Folder**.
* **Hardware-Accelerated Canvas Annotator (`react-konva` + Command Pattern Undo/Redo):**
  Starts `Alt + S` with no region pre-selected and stays in crop mode after dragging so you can refine your region freely before drawing arrows, rectangles, freehand ink, high-contrast text, and **destructive pixelate redactions**.

---

## Architecture & Roadmap Docs

* **[ARCHITECTURE.md](./ARCHITECTURE.md)** — Multi-window lifecycle, sequence diagrams, SQLite schema, B-Tree index strategy, Rust trait abstractions, and Desktop/Mobile (Android & iOS) sandbox design.
* **[ROADMAP.md](./ROADMAP.md)** — Phased milestones from `v1.0` MVP (Completed) to `v1.5` (`Alt + V` Visual Image Clipboard & Storage Quota Engine) and `v2.0` (`Alt + C` Segmented Hardware Replay Buffer).

---

## Getting Started

### 1. Run Frontend & Interactive Browser Preview (Only Node.js Required)

You can test the UI—including the Virtualized Gallery, Marquee Multi-Select, FTS5 search, B-Tree sorting, 500-item synthetic benchmark seeder, Settings keybind & folder configuration, and `Alt + S` `react-konva` Region Annotator—with Node.js:

```powershell
npm.cmd install
npm.cmd run dev
```

Run the unit test suite (`vitest`) and production TypeScript + Vite build:

```powershell
npm.cmd test
npm.cmd run build
```

### 2. Run Native Desktop App (`Tauri v2 + Rust`)

To compile and run the native Windows/macOS/Linux desktop app with system tray, global OS hotkeys (`Alt + S`, `Alt + Shift + S`, `Alt + G`, `Alt + V`, `Alt + C`), and `xcap` monitor capture:

```powershell
npm.cmd run tauri dev
```

Run the Rust SQLite + FTS5 + B-Tree sorting + group & orphan reconciliation tests:

```powershell
cd src-tauri
cargo test
```

---

## License

[MIT License](./LICENSE)
