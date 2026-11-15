# ReplayViewer — Product & Engineering Roadmap

> **Mission:** Replace slow, bloated screen capture utilities (like Windows Xbox Game Bar) with a lightning-fast, cross-platform FOSS desktop app built on **Tauri v2, Rust, SQLite, React, TypeScript, Tailwind CSS v4, and daisyUI v5**.
>
> **Git Policy Reminder:** Keep all work uncommitted/unpublished until Phase 1 MVP and verification tests pass.

---

## Target Performance Benchmarks (SLAs)

To stand out in technical interviews and real-world usage, every phase is measured against strict performance budgets:

| Metric | Target Budget | How It Is Achieved |
| :--- | :--- | :--- |
| **Idle Background RAM** | `< 50 MB` | System tray daemon + hidden main gallery WebView when closed |
| **Overlay Activation (`Alt + S`)** | `< 16 ms` | Pre-warmed hidden `overlay` window + `< 3ms` uncompressed BMP freeze over `http://capture.localhost/current` |
| **Gallery Scroll & Sort (5,000+ items)** | `60 FPS` locked / `< 1 ms` sort | `@tanstack/react-virtual` windowed grid + SQLite B-Tree indexes (`created_at`, `size_bytes`, `last_copied_at`) |
| **Search Query Latency (Text / OCR)** | `< 2 ms` | SQLite WAL mode + `FTS5` inverted full-text index (`captures_fts`) |
| **Semantic AI Visual Search (10k items)** | `< 8 ms` / `0 MB` idle RAM | Opt-in quantized `CLIP-INT8` 512-d embeddings (`1 KB`/img) + Rust SIMD cosine similarity + 60s lazy unload |
| **Visual Clipboard Memory (`Alt + V`)** | `< 2 MB` for 20 images | Disk-backed `.webp` cache + `200px` preview thumbnails + `blake3` deduplication |
| **20-Min Replay Save (`Alt + C`)** | `< 500 ms` | Hardware-encoded 2-second ring segments stitched via FFmpeg `-f concat -c copy` |

---

## Phase 1: MVP — Screenshot, Annotation & Virtualized Gallery (`v1.0` — COMPLETED)

**Status:** Complete & Verified  
**Deliverable:** A rock-solid daily-driver screenshot, annotation, and batch-organizable gallery app for Windows, macOS, and Linux.

### Milestone 1.1: Foundation, Frameless daisyUI v5 UI, Tray Daemon & SQLite Core
- [x] Scaffold Tauri v2 + React 19 + TypeScript + Vite + **Tailwind CSS v4 + daisyUI v5** workspace.
- [x] **Frameless Custom daisyUI Titlebar:** Replace the native Windows titlebar (`decorations: false`) with an integrated daisyUI draggable titlebar (`Minimize`, `Maximize/Restore`, `Close to Tray`) and a live **10-Theme Switcher** (`night`, `dark`, `dim`, `dracula`, `synthwave`, `cyberpunk`, `sunset`, `business`, `nord`, `light`).
- [x] Configure system tray menu (*Capture Region*, *Instant Fullscreen Capture*, *Open Gallery*, *Quit*) and intercept window close events so the app minimizes silently to tray.
- [x] Initialize embedded `rusqlite` database (`AppData/Local/com.replayviewer.app/replay_viewer_v2.db`) with `PRAGMA journal_mode = WAL`, non-destructive column migrations, and user-configurable media storage folders (`Pictures/ReplayViewer/captures` by default).
- [x] Register configurable global shortcuts (`Alt + S` for region capture, `Alt + Shift + S` for instant fullscreen capture, `Alt + G` for gallery, `Alt + V` for visual clipboard, and `Alt + C` for replay clip save—avoiding NVIDIA's `Alt + R` performance overlay).

### Milestone 1.2: Anti-Cheat Safe Capture, Fullscreen Mode & Persistent Region Selector (`Alt + S`)
- [x] Implement `ScreenCaptureProvider` strictly using OS compositor APIs (`xcap` / Desktop Duplication / `ScreenCaptureKit` / `PipeWire`) with **zero process/DLL injection** so captures are 100% safe with Vanguard, Easy Anti-Cheat, and BattlEye.
- [x] Add **Instant Fullscreen Capture (`Alt + Shift + S`)** that grabs raw RGBA monitor pixels, copies to clipboard, and encodes WebP + thumbnail without double PNG encoding and *without* spawning an overlay window—preventing Exclusive Fullscreen games from minimizing/Alt-Tabbing mid-match.
- [x] Stream frozen monitor frames as **uncompressed BMP in RAM (`< 3ms`)** via `http://capture.localhost/current` (Windows WebView2) / `capture://localhost/current` (macOS/Linux) with zero Base64 JSON serialization delay.
- [x] Build the frameless fullscreen `overlay` window (`Alt + S`) starting with **no region pre-selected (`cropRect: null`)** and **remaining in capture/crop mode after selecting an area** so users can freely re-drag their selection until choosing an annotation tool or saving.
- [x] Normalize high-DPI coordinates (`window.devicePixelRatio` vs. monitor `scale_factor`) so crops on 125% / 150% / 200% displays are pixel-accurate.

### Milestone 1.3: Quick Annotation Canvas (`react-konva`)
- [x] Mount a `react-konva` stage over the frozen screen with a daisyUI v5 floating contextual toolbar.
- [x] Implement core annotation tools:
  - **Arrow** & **Rectangle** (with color picker & stroke width)
  - **Freehand Pen / Highlighter**
  - **Text Box** (inline prompt with high-contrast stroke)
  - **Pixelate / Redact Region** (destructively flattened into the exported bitmap to redact sensitive information like API keys or emails)
- [x] Implement a **Command Pattern Undo/Redo stack** (`Ctrl + Z` / `Ctrl + Y`) in Zustand.
- [x] Wire up instant export: pressing `Enter` / `Ctrl + C` or clicking **Save & Copy** hides the overlay immediately, copies the image to the OS clipboard (`tauri-plugin-clipboard-manager`), saves `.webp`/`.png` to disk, generates a `320px` `.webp` thumbnail in Rust, and records metadata in SQLite.

### Milestone 1.4: Virtualized Gallery (`Alt + G`), Marquee Multi-Select, B-Tree Sorting & Settings
- [x] Build the responsive gallery grid powered by `@tanstack/react-virtual` and a live **Card Columns Resizer (`Auto` or `1–8` columns)**.
- [x] **Rubber-Band Marquee Drag-to-Select & Batch Action Bar:** Left-click hold and drag anywhere in the gallery (even starting on an image card) to multi-highlight cards across virtualized rows, revealing a batch toolbar to **Group Selected**, **Add Tags**, **Move to New Folder** (relocating files on disk and removing them from the working gallery), or **Delete Selected**.
- [x] **Zero-Lag B-Tree Indexed Sorting & Rich Filters:** Instant `DESC ↓` / `ASC ↑` sorting by **Date Captured**, **File Size**, or **Copied Time**, plus filters for **All**, **Pinned**, **Recently Copied to Clipboard**, **Media Type** (`Screenshots` / `Replay Clips`), **Groups**, **Date Range** (`24h` / `7d` / `30d`), and **Instant FTS5 Search**.
- [x] **Orphaned DB Row Reconciliation:** Automatically prunes SQLite rows whose underlying files were manually deleted or moved in Windows Explorer / Finder.
- [x] **Configurable Working Storage Directory & External Media Importer:** Browse with a native OS Folder Picker dialog (`pick_folder_dialog`) or paste a custom path in Settings. Changing folders prompts the user to either **Move Existing Captures to New Folder** (preserving all tags/pins/groups) or **Switch Active Folder View** (scoping the Gallery to the active folder while remembering old folder metadata), and automatically checks for pre-existing images (*"Found X existing image(s) — Import into Gallery?"*) with original file timestamps preserved.

---

## Phase 1.5: Visual Image Clipboard (`Alt + V`) & Smart Storage (`v1.5` — UP NEXT)

**Goal:** Solve Windows `Win + V` image reliability problems and automate storage quota cleanup.

### Milestone 1.5.1: The `Alt + V` Visual Image Clipboard (Windows `Win + V` for Images)
- [x] Implement a background OS clipboard image watcher in Rust (`arboard` + `clipboard-win` sequence counter) that detects images copied from *any* application (browsers, Discord, design tools, or ReplayViewer itself) with near-zero idle CPU usage and Windows privacy marker compliance (`ExcludeClipboardContentFromMonitorProcessing`).
- [x] Compute a sub-millisecond pixel digest (`crc32fast` SIMD + `Sha256`, `< 0.5ms`) on raw RGBA pixel bytes to deduplicate repeated copies—bumping existing images to the top of the history list instead of duplicating storage.
- [x] Store full clipboard images in `clipboard_cache/` with `200px` `.webp` thumbnails, keep **Pinned images anchored at the top** (`ORDER BY is_pinned DESC, copied_at DESC`), and enforce a user-configurable **FIFO ring buffer limit** (`5`, `10`, `20`, or `50` unpinned images) in SQLite.
- [x] Build the frameless daisyUI v5 `Alt + V` quick-paste popup window (`clipboard`):
  - **Persistent Always-on-Top Toggle:** Pin/unpin the `Alt + V` drawer window from the top (`clipboard_always_on_top`), remembered across openings.
  - **`CURRENTLY COPIED` Indicator:** Live badge showing which clipboard item is currently sitting in the active OS clipboard.
  - **Click / `Enter` / `1–9`:** Pushes the selected historical image back into the active OS clipboard and closes the drawer.
  - **1:1 Centered Annotation + Cross-Session `Ctrl + Z` / `Ctrl + Y` History (`A`):** Opens any clipboard image centered at its actual 1:1 pixel dimensions with scale buttons (`1:1`, `2×`, `4×`, `Fit Screen`) and 4 draggable corner handles, saving both the flattened bitmap and non-destructive vector layers (`annotations_json` + `base_file_path`) so re-editing preserves full `Ctrl + Z` / `Ctrl + Y` history while keeping `Pixelate` redactions destructive.
  - **Pin & Promote:** Pin favorite clipboard images to the top so FIFO eviction skips them, or click *"Save to Gallery"* to archive them permanently.

### Milestone 1.5.2: Pinning, Tagging, Groups & Instant FTS5 Search (Completed Early in v1.0!)
- [x] One-click **Pinning** (`is_pinned`), custom **Groups/Collections** (`group_name`), and **Tags** (`#bug`, `#valorant`, etc.) with batch editing.
- [x] SQLite **FTS5 (`captures_fts`)** sub-5ms search across tags, groups, source application names, and OCR text.
- [x] Date-range, Recently Copied (`last_copied_at`), Group, and Media-Type (`Screenshots` / `Videos` / `Pinned`) filters.

### Milestone 1.5.3: Automated Storage Quota & Cleanup Engine
- [ ] Build a background storage policy runner in Rust supporting two user-configurable rules:
  1. **Max Folder Quota** (e.g., *"Cap library at 15 GB; auto-delete oldest unpinned captures when exceeded"*).
  2. **Max Retention Age** (e.g., *"Auto-delete unpinned captures older than 30 days"*).
- [ ] Provide a visual storage breakdown bar in Settings with a **"Preview Cleanup (Dry Run)"** button so users never lose files unexpectedly.

### Milestone 1.5.4: Zero-Bloat Native OS Background OCR Indexing
- [ ] **Native OS Neural OCR (`0 MB` Installer & Idle RAM Bloat):** Call built-in OS OCR APIs directly from Rust (`Windows.Media.Ocr` via WinRT on Windows, `VNRecognizeTextRequest` on macOS, with optional on-demand `ocrs` fallback on Linux) on a background thread (`~50–120 ms` per capture) *after* the screenshot is already copied to the clipboard.
- [ ] **Instant `< 2 ms` FTS5 Text Search:** Populate `captures.ocr_text` and sync with the `captures_fts` virtual table so searching *"error 500"*, *"invoice"*, or a player username finds matching screenshots across 10,000+ items without re-scanning images at query time.

---

## Phase 2.0: Hardware-Accelerated Replay Buffer, Video Trimmer & Smart Visual AI (`v2.0`)

**Goal:** Deliver a zero-lag 30-second to 20-minute gaming & desktop replay buffer (`Alt + C` — Save Clip, avoiding NVIDIA's `Alt + R` performance overlay) plus opt-in semantic visual search.

### Milestone 2.0.1: GPU Capture, On-Demand LGPL FFmpeg & Segmented Ring Buffer Engine
- [ ] **MIT-Safe FFmpeg Resolver & One-Click Downloader:** Keep the base installer tiny (~5–7 MB) by detecting `ffmpeg` on the system `PATH`, offering a one-click *"Download Replay Engine (Minimal LGPL Build)"* button in Settings (`AppData/Local/ReplayViewer/bin/`), and supporting custom user-supplied Full GPL (`libx264`) binary paths.
- [ ] Implement `ReplayBufferProvider` using native capture APIs (`windows-capture` / `Windows.Graphics.Capture` on Windows, `ScreenCaptureKit` on macOS, `PipeWire` on Linux) paired with the resolved FFmpeg binary.
- [ ] Auto-detect available hardware encoders (**NVIDIA NVENC**, **AMD AMF**, **Intel QuickSync**, **Apple VideoToolbox**, **Windows MediaFoundation `h264_mf`**) with software fallback.
- [ ] Configure FFmpeg's `segment` muxer to write rolling 2-second `.ts` / fragmented `.mp4` chunks into a dedicated ring directory, automatically pruning segments older than the user's configured window (**30s, 2m, 5m, or 20m**).

### Milestone 2.0.2: Multi-Track Audio & Instant `Alt + C` Save
- [ ] Capture desktop/game loopback audio (WASAPI on Windows) + optional microphone input track.
- [ ] Implement the `Alt + C` save handler: run `ffmpeg -f concat -safe 0 -i segments.txt -c copy output.mp4` to stitch the active ring buffer in **< 0.5 seconds** with zero re-encoding, extract a video thumbnail frame, and insert into SQLite.

### Milestone 2.0.3: Built-In Lossless Video Trimmer
- [ ] Add an interactive video player with a dual-handle timeline range slider inside the Gallery Lightbox.
- [ ] Allow users to trim a 20-minute replay down to a 15-second highlight (using fast keyframe stream copy or hardware trim) and optionally delete the original multi-gigabyte file in one click.

### Milestone 2.0.4: Privacy, Password Manager Protection & True Redaction Security
- [ ] **Password Manager & Clipboard Privacy Compliance:** Respect OS clipboard privacy flags (`ExcludeClipboardContentFromMonitorProcessing` and `CanIncludeInClipboardHistory`), add a *"Pause Clipboard History"* System Tray toggle, and implement an **Excluded Apps Blocklist** (e.g., 1Password, Bitwarden, KeePass, banking/incognito windows) so sensitive copies are never recorded in `Alt + V`.
- [x] **True Redaction Security (Destructive Flatten Before OCR/Disk):** Pixelation boxes drawn over sensitive info in `react-konva` are destructively flattened into a single rasterized bitmap before writing to disk.

### Milestone 2.0.5: Opt-In Smart AI Visual Search (`MobileCLIP` / `CLIP-INT8` Embeddings)
- [ ] **Zero-Bloat On-Demand Model Downloader:** Keep the base installer at ~5–6 MB by offering an opt-in *"Enable Smart AI Visual Search (~95 MB Download)"* in Settings (`AppData/Local/com.replayviewer.app/models/`).
- [ ] **One-Time Background Thumbnail Embedding (`~40–80 ms`):** Encode each capture's `320px` thumbnail into a compact 512-dimensional quantized vector (`512 bytes–1 KB` per image; only ~10 MB for 10,000 images) stored in SQLite (`capture_embeddings`).
- [ ] **60-Second Lazy RAM Unloader (`0 MB` Idle Bloat):** Automatically drop the ONNX session from memory after 60 seconds of inactivity so background tray RAM stays `< 50 MB`.
- [ ] **Sub-8ms Semantic Search Across 10,000+ Unlabeled Images:** Embed natural-language queries (e.g., `"cat"`, `"sunset"`, `"skill tree"`, `"red car"`) in ~3 ms and rank 10,000 image vectors in ~2–4 ms using Rust SIMD cosine similarity.

---

## Phase 3.0 (Post-2027): Mobile Expansion (Android & iOS)

- [ ] **Milestone 3.1 (Android):** Opt-in "Gaming Mode" `MediaProjection` 30s–2m rolling replay buffer, Quick Settings Tile, and Floating Assist Bubble.
- [ ] **Milestone 3.2 (iOS):** Action Button / Back-Tap Apple Shortcut, Share Sheet Annotator Extension, and `PHPhotoLibrary` Camera Roll Screenshot De-Clutterer.
- [ ] **Milestone 3.3 (Cross-Device LAN Sync):** Zero-cloud mDNS + QUIC/WebRTC local Wi-Fi transfer between PC and phone.
