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
| **Search Query Latency (Tags / OCR / BYOK Labels)** | `< 2 ms` | SQLite WAL mode + `FTS5` inverted full-text index (`captures_fts`) |
| **Vision Tagging Bloat (`v2.0.5` BYOK)** | `0 MB` installer & `0 MB` idle RAM | Scope-disciplined BYOK `VisionTaggingProvider` adapter (Gemini / OpenAI / AWS / Local Ollama) populating `FTS5` tags |
| **Visual Clipboard Memory (`Alt + V`)** | `< 2 MB` for 20 images | Disk-backed `.webp` cache + `200px` preview thumbnails + `blake3` deduplication |
| **20-Min Replay Save (`Alt + C`)** | `< 500 ms` | Hardware-encoded 2-second ring segments stitched via FFmpeg `-f concat -c copy` |

---

## Phase 1: MVP — Screenshot, Annotation & Virtualized Gallery (`v1.0` — COMPLETED)

**Status:** Complete & Verified  
**Deliverable:** A rock-solid daily-driver screenshot, annotation, and batch-organizable gallery app for Windows, macOS, and Linux.

### Milestone 1.1: Foundation, Frameless daisyUI v5 UI, Tray Daemon & SQLite Core
- [x] Scaffold Tauri v2 + React 19 + TypeScript + Vite + **Tailwind CSS v4 + daisyUI v5** workspace.
- [x] **Frameless Custom daisyUI Titlebar:** Replace the native Windows titlebar (`decorations: false`) with an integrated daisyUI draggable titlebar (`Minimize`, `Maximize/Restore`, `Close to Tray`) and a live **10-Theme Switcher** (`night`, `dark`, `dim`, `dracula`, `synthwave`, `cyberpunk`, `sunset`, `business`, `nord`, `light`).
- [x] Configure system tray menu (*Capture Region*, *Instant Fullscreen Capture*, *Open Gallery*, *Quit*), intercept window close events so the app minimizes silently to tray, and enforce **Single-Instance Startup (`single_instance.rs`)** so launching `replay-viewer.exe` a second time brings the existing running instance to the foreground instead of spawning a duplicate tray icon.
- [x] Initialize embedded `rusqlite` database (`AppData/Local/com.replayviewer.app/replay_viewer_v2.db`) with `PRAGMA journal_mode = WAL`, non-destructive column migrations, and a portable built-in `./captures` default storage directory next to `replay-viewer.exe` (with automatic fallback to `Pictures/ReplayViewer/captures` if installed in a read-only folder).
- [x] Register configurable global shortcuts (`Alt + S` for region capture, `Alt + Shift + S` for instant fullscreen capture, `Alt + G` for gallery, `Alt + V` for visual clipboard, and `Alt + C` for replay clip save—avoiding NVIDIA's `Alt + R` performance overlay) with **live OS re-registration**, **recording-mode OS shortcut suspension** (so pressing `Alt + S` while recording a keybind sets `Alt + S` instead of taking a screenshot), **duplicate keybind conflict auto-unbinding**, and a **meatballs (`⋯`) menu** per shortcut to **Reset to Default** or **Remove Keybind**.

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
- [x] Wire up 3-way instant export:
  - **`Copy` (`Ctrl + C` / `Cmd + C` — Default Primary Button):** Copies the cropped/annotated image to the OS clipboard and `Alt + V` ring *without* saving a file to the PC's Gallery folder.
  - **`Save`:** Saves `.webp`/`.png` + `320px` thumbnail to the Gallery folder and SQLite without overwriting the OS clipboard.
  - **`Copy & Save` (`Enter`):** Copies to the OS clipboard *and* saves to the Gallery folder on disk.

### Milestone 1.4: Virtualized Gallery (`Alt + G`), Marquee Multi-Select, B-Tree Sorting & Settings
- [x] Build the responsive gallery grid powered by `@tanstack/react-virtual` and a live **Card Columns Resizer (`Auto` or `1–8` columns)**.
- [x] **Rubber-Band Marquee Drag-to-Select & Batch Action Bar:** Left-click hold and drag anywhere in the gallery (even starting on an image card) to multi-highlight cards across virtualized rows, revealing a batch toolbar to **Group Selected**, **Add Tags**, **Move to New Folder** (relocating files on disk and removing them from the working gallery), or **Delete Selected**.
- [x] **Zero-Lag B-Tree Indexed Sorting & Rich Filters:** Instant `DESC ↓` / `ASC ↑` sorting by **Date Captured**, **File Size**, or **Copied Time**, plus filters for **All**, **Pinned**, **Recently Copied to Clipboard**, **Media Type** (`Screenshots` / `Replay Clips`), **Groups**, **Date Range** (`24h` / `7d` / `30d`), and **Instant FTS5 Search**.
- [x] **Orphaned DB Row Reconciliation:** Automatically prunes SQLite rows whose underlying files were manually deleted or moved in Windows Explorer / Finder.
- [x] **Configurable Working Storage Directory & External Media Importer:** Browse with a native OS Folder Picker dialog (`pick_folder_dialog`) or paste a custom path in Settings. Changing folders prompts the user to either **Move Existing Captures to New Folder** (preserving all tags/pins/groups) or **Switch Active Folder View** (scoping the Gallery to the active folder while remembering old folder metadata), and automatically checks for pre-existing images (*"Found X existing image(s) — Import into Gallery?"*) with original file timestamps preserved.

---

## Phase 1.5: Visual Image Clipboard (`Alt + V`), Smart Storage 3-Day Grace Buffer & Native OS OCR (`v1.5` — COMPLETED)

**Status:** Complete & Verified  
**Goal:** Solve Windows `Win + V` image reliability and lock-contention problems, automate storage quota cleanup with a 3-day safety buffer and 100% pinned immunity, and index image text using zero-bloat Native OS OCR.

### Milestone 1.5.1: The `Alt + V` Visual Image Clipboard (Windows `Win + V` for Images)
- [x] Implement a non-blocking background OS clipboard image watcher in Rust (`clipboard-win` sequence counter + `180ms` settle debounce + `< 1ms` `OpenClipboard` raw `memcpy` of `"PNG"` or `CF_DIBV5`/`CF_DIB`) that detects 12 MB+ images copied from *any* application (Discord, browsers, design tools, or ReplayViewer) with zero UI stutter and Windows privacy marker compliance (`ExcludeClipboardContentFromMonitorProcessing`).
- [x] Compute a sub-millisecond pixel digest (`crc32fast` SIMD + `Sha256`, `< 0.5ms`) on raw RGBA pixel bytes to deduplicate repeated copies—bumping existing images to the top of the history list instead of duplicating storage.
- [x] Store full clipboard images in `clipboard_cache/` with `200px` `.webp` thumbnails, keep **Pinned images anchored at the top** (`ORDER BY is_pinned DESC, copied_at DESC`), and enforce a user-configurable **FIFO ring buffer limit** (`5`, `10`, `20`, or `50` unpinned images) in SQLite.
- [x] Build the frameless daisyUI v5 `Alt + V` quick-paste popup window (`clipboard`):
  - **Persistent Always-on-Top Toggle:** Pin/unpin the `Alt + V` drawer window from the top (`clipboard_always_on_top`), remembered across openings.
  - **`CURRENTLY COPIED` Indicator:** Live badge showing which clipboard item is currently sitting in the active OS clipboard.
  - **Click / `Enter` / `1–9`:** Pushes the selected historical image back into the active OS clipboard and closes the drawer.
  - **1:1 Centered Annotation, Visual Zoom vs. Output Resize Lock, & Cross-Session `Ctrl + Z` / `Ctrl + Y` (`A`):** Opens any clipboard image centered at its actual 1:1 pixel dimensions with visual zoom controls (`1:1`, `2×`, `4×`, `Fit`, and 4 draggable corner handles) that preserve original output dimensions by default (`Output: Locked`), plus a 1-click `Resize Output` toggle pill when actual dimension scaling is desired. Saves both the flattened bitmap and non-destructive vector layers (`annotations_json` + `base_file_path`) so re-editing preserves full `Ctrl + Z` / `Ctrl + Y` history while keeping `Pixelate` redactions destructive.
  - **Pin & Promote:** Pin favorite clipboard images to the top so FIFO eviction skips them, or click *"Save to Gallery"* to archive them permanently.

### Milestone 1.5.2: Pinning, Tagging, Groups & Instant FTS5 Search (Completed Early in v1.0!)
- [x] One-click **Pinning** (`is_pinned`), custom **Groups/Collections** (`group_name`), and **Tags** (`#bug`, `#valorant`, etc.) with batch editing.
- [x] SQLite **FTS5 (`captures_fts`)** sub-5ms search across tags, groups, source application names, and OCR text.
- [x] Date-range, Recently Copied (`last_copied_at`), Group, and Media-Type (`Screenshots` / `Videos` / `Pinned`) filters.

### Milestone 1.5.3: Automated Storage Quota, Pinned Immunity & 3-Day "Queued Deletion" Grace Buffer
- [x] Build a background storage policy runner in Rust supporting two user-configurable rules:
  1. **Max Folder Quota (Default: `10 GB`):** Automatically queues the oldest unpinned captures when unpinned active storage exceeds the cap (`0 = Unlimited`).
  2. **Max Retention Age (Default: `0` = No Max Retention Age / Keep Forever):** Optional age rule (`7`, `14`, `30`, `60`, `90`, `180`, `365` days) for unpinned captures.
- [x] **100% Pinned Immunity (`is_pinned = 1`):** Pinned images and videos are never pruned or queued for deletion, and pinning any queued item immediately rescues it (`queued_delete_at = NULL`).
- [x] **3-Day "Queued Deletion" Grace Buffer (`queued_delete_at`):** Pruned unpinned captures are held for **3 days** before permanent deletion from disk. Dedicated **`Queued Deletion (X)`** Gallery filter tab + Settings breakdown show the live countdown & exact auto-deletion datetime (`"2d 23h left • Deletes Nov 18, 20:35"`) with 1-click **Restore**, **Pin & Restore**, and **Preview Cleanup (Dry Run)**.

### Milestone 1.5.4: Zero-Bloat Native OS Background OCR Indexing
- [x] **Native OS Neural OCR (`0 MB` Installer & Idle RAM Bloat):** Call built-in OS OCR APIs directly from Rust (`Windows.Media.Ocr` via WinRT `OcrEngine` + `SoftwareBitmap` BGRA8 on Windows, `VNRecognizeTextRequest` on macOS, and `ocrs`/`tesseract` fallback on Linux) on a background thread (`~50–120 ms` per capture) *after* the screenshot is already copied to the clipboard.
- [x] **Instant `< 2 ms` FTS5 Text Search & Lightbox OCR Inspector:** Populate `captures.ocr_text` + `ocr_status` (`'pending' | 'done' | 'failed'`), sync with the `captures_fts` virtual table, display copyable **Detected Text (Native OS OCR)** in the Lightbox, and provide a **"Scan OCR"** backfill button in Settings.

---

## Phase 2.0: Hardware-Accelerated Replay Buffer, Video Trimmer & Smart Visual AI (`v2.0`)

**Goal:** Deliver a zero-lag 30-second to 20-minute gaming & desktop replay buffer (`Alt + C` — Save Clip, avoiding NVIDIA's `Alt + R` performance overlay) plus opt-in semantic visual search.

### Milestone 2.0.1: Anti-Cheat Safe GPU Capture, On-Demand LGPL FFmpeg & Segmented Ring Buffer Engine (COMPLETED)
- [x] **MIT-Safe FFmpeg Resolver & Persistent One-Click Download/Upgrade (`ffmpeg.rs`, `useSettingsStore.ts`):** Keeps the base installer `< 6 MB` by resolving `custom -> AppData/Local/com.replayviewer.app/bin -> PATH`, caching probe results in `AppState`, exposing a native `Browse...` binary picker, and persisting background **Download / Upgrade Engine** progress across tab switches while gating buffer toggles until ready.
- [x] **100% Anti-Cheat Safe Capture & Auto GPU Encoder Probing:** Implements `ReplayBufferProvider` using zero-copy DXGI Desktop Duplication (`ddagrab` with automatic `gdigrab` fallback on Windows, `avfoundation` on macOS, `x11grab` on Linux) and hardware encoder auto-detection (`h264_nvenc` -> `h264_amf` -> `h264_qsv` -> `h264_mf` -> `libx264`).
- [x] **Always-Ready Buffer & Alt-Tab Continuity (`replay.rs`):** Auto-starts on launch (default **`60s / 1m`** @ `60 FPS`, `20 Mbps`; selectable `30s`, `1m`, `2m`, `5m`, `10m`, `20m`), keeping `O(1)` RAM (`~50 MB`) & VRAM (`~120 MB`) while continuously rolling 2-second `.ts` chunks with CFR + forced 2-second keyframes (`expr:gte(t,n_forced*2)`), graceful `"q\n"` flushing, trailing in-flight segment inclusion (preventing 2s-early clip cutoffs), and `ffprobe` duration readout.

### Milestone 2.0.2: Dynamic Multi-Track Audio & Instant `Alt + C` Save (COMPLETED)
- [x] **Dynamic OS Audio Session Discovery (`IAudioSessionManager2` + `IAudioMeterInformation`):** Live mixer in `AudioMixerCard.tsx` polls zero-SQLite peak levels at `~14 Hz` (`get_live_audio_peaks`) on a perceptual `-48..0 dBFS` meter scale, pins app rows alphabetically, filters noisy Windows hosts (`svchost.exe`), resolves multi-process browser root PIDs (`CreateToolhelp32Snapshot` for `msedge.exe`/`chrome.exe`/`Discord.exe`), and includes a **`Tracks: X/4` dropdown + `Clear Tracks`** manager for removing closed/stale isolated entries.
- [x] **Zero-Restart Hot-Swappable WASAPI Audio Pipes & Live Gain (`audio.rs`):** Pre-allocates `Desktop Audio`, `Microphone`, and 4 reusable `ProcessSlot` pipes (`48 kHz stereo s16le`, 20ms wall-clock silence padding) driven by lock-free `LiveAudioGains` (`0%–200%`) and `LiveAudioRoutePlan`, so Mute, Unmute, Isolate, and `Desktop`/`Mic`/`Track Vol` changes hot-swap in-place with **zero FFmpeg restarts and zero dropped video frames**—while `Master Mix` (`0:a:0`) blends `Desktop + Mic` cleanly without comb-filtering/phasing.
- [x] **Microphone Device Selector, WASAPI Live Test & Instant `Alt + C` Save (`audio.rs`, `replay.rs`):** Enumerates active capture endpoints (`IMMDeviceEnumerator::EnumAudioEndpoints`) with a built-in WASAPI **Microphone Test** meter + optional **Hear Yourself** (`IAudioRenderClient`) sidetone, stitches `.ts` chunks via `-f concat -c copy -movflags +faststart` while stripping unused `ProcessSlot` streams, generates a `320px` WebP thumbnail, and indexes into SQLite/FTS5.

### Milestone 2.0.3: Built-In Lossless Video Trimmer & Multi-Track Audio Export (COMPLETED)
- [x] **Interactive Lightbox Dual-Handle Trimmer (`VideoTrimmerPanel.tsx`, `MediaLightbox.tsx`):** Drag In/Out range handles (`>= 500 ms` minimum span), `Set In` / `Set Out` to playhead, `Play Selection` loop preview, and live proportional output size estimation inside the Gallery Lightbox.
- [x] **Embedded Audio Track Waveforms, Live Solo/Mute Auditioning & Discord Mixdown (`prepare_video_track_previews_cmd`, `trim_video_clip_cmd`):** Lazily extracts per-track waveform peaks + synced `.m4a` audition streams in one FFmpeg pass when opening the Lightbox Trimmer, renders aligned waveform lanes under the trim bar with **Solo (`S`)**, **Monitor Mute (`M`)**, and **`Exp`** export toggles, and optionally mixes checked tracks via `amix` into one Discord/web-ready AAC stream.
- [x] **Lossless Keyframe Cut, Size-Capped Export & 1-Click Replace Original (`replay.rs`):** Supports instant `-ss ... -i ... -c:v copy` stream copy, bitrate-budgeted **`Discord 10 MB`**, **`25 MB`**, and **`50 MB`** target presets (using active GPU/CPU H.264 encoder), generates a fresh `320px` WebP thumbnail, indexes into SQLite/FTS5, and optionally deletes the multi-gigabyte source clip on completion.

### Milestone 2.0.4: Privacy, Password Manager Protection & True Redaction Security (COMPLETED)
- [x] **Password Manager & Clipboard Privacy Compliance:** Respect OS clipboard privacy flags (`ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`, and `CanUploadToCloudClipboard`), add a *"Pause / Resume Clipboard History"* System Tray toggle, and implement an **Excluded Apps & Window Titles Blocklist** (`1password`, `bitwarden`, `keepass`, `keepassxc`, `dashlane`, `lastpass`, `inprivate`, `incognito`, etc.) so sensitive copies are never recorded in `Alt + V`.
- [x] **True Redaction Security (Destructive Flatten Before OCR/Disk):** Pixelation boxes drawn over sensitive info in `react-konva` are destructively flattened into a single rasterized bitmap before writing to disk.

### Milestone 2.0.5: Pluggable "BYOK" Vision Tagging API (Scope-Disciplined Alternative to Bundled Local ML)
- [ ] **Architectural Scope Discipline (`0 MB` Binary & Idle RAM Bloat):** Rather than shipping a 150+ MB embedded neural runtime (`ONNX` + `CLIP`) or inaccurate pixel heuristics that bloat the app and dilute focus from capture/replay performance, ReplayViewer keeps **Native OS OCR (`v1.5.4`)** as the zero-cost offline default and exposes a clean **BYOK (Bring Your Own Key / Endpoint) `VisionTaggingProvider` adapter**.
- [ ] **Multi-Provider BYOK Support:** Allow users to plug in **Google Gemini (`gemini-2.5-flash`)**, **OpenAI / OpenRouter**, **AWS Rekognition**, or a **local self-hosted Ollama endpoint (`http://localhost:11434/v1`)** in Settings.
- [ ] **FTS5 Tag Enrichment (`< 2 ms` Offline Query Speed):** Send only the redacted `320px` `.webp` thumbnail (~15 KB) to the user's configured provider and store returned semantic labels directly as standard SQLite `FTS5` tags—keeping search instantaneous and 100% offline at query time.

---

## Phase 3.0 (Post-2027): Mobile Expansion (Android & iOS)

- [ ] **Milestone 3.1 (Android):** Opt-in "Gaming Mode" `MediaProjection` 30s–2m rolling replay buffer, Quick Settings Tile, and Floating Assist Bubble.
- [ ] **Milestone 3.2 (iOS):** Action Button / Back-Tap Apple Shortcut, Share Sheet Annotator Extension, and `PHPhotoLibrary` Camera Roll Screenshot De-Clutterer.
- [ ] **Milestone 3.3 (Cross-Device LAN Sync):** Zero-cloud mDNS + QUIC/WebRTC local Wi-Fi transfer between PC and phone.
