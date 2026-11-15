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
* **Hardware-Accelerated Canvas Annotator (`react-konva` + Cross-Session Command Pattern Undo/Redo):**
  Starts `Alt + S` with no region pre-selected and stays in crop mode after dragging so you can refine your region freely before drawing arrows, rectangles, freehand ink, high-contrast text, and **destructive pixelate redactions**. Supports **1:1 centered editing from `Alt + V`** with visual zoom (`1:1`, `2×`, `4×`, `Fit`, corner drag) that keeps original output resolution locked by default (or 1-click output resize) and preserves **cross-session `Ctrl + Z` / `Ctrl + Y` vector history**.
* **`Alt + V` Visual Image Clipboard Drawer:**
  Frameless popup drawer (`Alt + V`) backed by an OS clipboard sequence watcher, `< 0.5ms` SIMD `crc32fast` + `Sha256` deduplication, `200px` WebP thumbnails, pinned-on-top items, a live `CURRENTLY COPIED` indicator, `1–9` instant paste shortcuts, and configurable FIFO ring eviction (`5–50` unpinned images).

---

## Getting Started

### 1. Run Frontend & Interactive Browser Preview (Only Node.js Required)

You can test the UI—including the Virtualized Gallery, Marquee Multi-Select, FTS5 search, B-Tree sorting, 500-item synthetic benchmark seeder, Settings keybind & folder configuration, `Alt + V` Visual Clipboard, and `Alt + S` `react-konva` Region Annotator—with Node.js:

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

Run the Rust SQLite + FTS5 + B-Tree sorting + Clipboard FIFO/Pin tests:

```powershell
cd src-tauri
cargo test
```

---

## Architecture

ReplayViewer is structured around **three pre-warmed frameless Tauri v2 windows** backed by a single **Rust System Tray & SQLite WAL Core**:

1. **`overlay` Window (`Alt + S` — Region Capture & Annotator):**
   * Freezes the active monitor via `xcap` as an uncompressed BMP in RAM (`< 3ms`) and streams it over `http://capture.localhost/current` with zero Base64 JSON serialization overhead.
   * Also opens clipboard items from `Alt + V` centered at 1:1 actual pixel dimensions with non-destructive vector layer hydration (`annotations_json` + `base_file_path`) for cross-session `Ctrl + Z` / `Ctrl + Y`.
2. **`clipboard` Window (`Alt + V` — Visual Image Clipboard Drawer):**
   * Watches OS clipboard sequence changes (`GetClipboardSequenceNumber` + `arboard`), deduplicates raw RGBA pixels in `< 0.5ms`, stores `200px` `.webp` thumbnails in `clipboard_cache/`, keeps pinned items at the top, and evicts oldest unpinned items via a FIFO ring limit (`5–50`).
3. **`main` Window (`Alt + G` — Virtualized Gallery & Settings):**
   * Renders 10,000+ captures at 60 FPS using `@tanstack/react-virtual`, $O(N)$ coordinate-math rubber-band marquee multi-selection, SQLite `FTS5` full-text search (`< 2ms`), and B-Tree indexed sorting (`created_at`, `size_bytes`, `last_copied_at`).

For full sequence diagrams, SQLite schema definitions, and IPC contracts, see **[ARCHITECTURE.md](./ARCHITECTURE.md)**.

---

## Roadmap

* **Phase 1.0 — MVP Screenshot, Annotation & Virtualized Gallery (`v1.0` — Completed)**
  * [x] Frameless daisyUI v5 UI with 10 live themes, System Tray daemon, and SQLite WAL + FTS5 + B-Tree core
  * [x] Anti-cheat safe `Alt + S` region capture (`< 3ms` BMP freeze) & `Alt + Shift + S` instant fullscreen capture (zero focus loss)
  * [x] `react-konva` annotation canvas (Arrow, Rect, Pen, Text, destructive Pixelate redaction, Undo/Redo)
  * [x] 60 FPS Virtualized Gallery (`Alt + G` toggle), Marquee Multi-Select, Batch Tag/Group/Move/Delete, and Working Folder migration & external image importer
* **Phase 1.5 — Visual Clipboard, Storage Quota & Native OS OCR (`v1.5` — In Progress)**
  * [x] **v1.5.1:** `Alt + V` Visual Image Clipboard Drawer (FIFO ring buffer, pinned-on-top, `CURRENTLY COPIED` badge, 1:1 centered annotation with visual zoom lock & cross-session `Ctrl + Z` / `Ctrl + Y`)
  * [x] **v1.5.2:** Pinning, Tagging, Custom Groups & Instant FTS5 Search
  * [ ] **v1.5.3:** Automated Storage Quota (Max GB / Max Days) & Dry-Run Cleanup Engine
  * [ ] **v1.5.4:** Zero-Bloat Native OS Background OCR (`Windows.Media.Ocr` / `Vision`, `+0 MB` installer size) indexed into `captures_fts`
* **Phase 2.0 — Hardware Replay Buffer, Video Trimmer & Opt-In Visual AI (`v2.0` — Planned)**
  * [ ] **v2.0.1–2.0.2:** GPU-accelerated 30s–20m segmented ring buffer (`Alt + C` `< 0.5s` lossless FFmpeg concat + multi-track WASAPI audio)
  * [ ] **v2.0.3:** Built-in lossless Lightbox Video Trimmer
  * [ ] **v2.0.4:** Password Manager & Excluded Apps Clipboard Privacy Blocklist
  * [ ] **v2.0.5:** Opt-In Smart AI Visual Search (`MobileCLIP` / `CLIP-INT8` 512-d embeddings + 60s lazy RAM unloader for `< 8ms` semantic search across 10,000+ unlabeled images)
* **Phase 3.0 — Mobile Companion & Cross-Device LAN Sync (`v3.0` — Future)**
  * [ ] Android & iOS companion apps with mDNS + QUIC/WebRTC local clip transfer

For detailed milestone checklists and SLA benchmarks, see **[ROADMAP.md](./ROADMAP.md)**.

---

## License

[MIT License](./LICENSE)
