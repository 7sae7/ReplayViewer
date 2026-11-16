use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

use crate::db::captures_repo::{list_pending_ocr_captures, update_capture_ocr};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

static OCR_BACKFILL_RUNNING: AtomicBool = AtomicBool::new(false);

/// Cross-platform trait for zero-bloat background OCR text extraction (Milestone 1.5.4).
/// - Windows: Native WinRT `Windows.Media.Ocr` in-process via `SoftwareBitmap` (`0 MB` bloat).
/// - macOS: Native Apple Vision framework (`VNRecognizeTextRequest`).
/// - Linux: On-demand `ocrs` or `tesseract` CLI fallback.
pub trait OcrProvider: Send + Sync {
    fn extract_text_from_file(&self, image_path: &Path) -> AppResult<String>;
}

pub struct NativeOsOcrProvider;

impl OcrProvider for NativeOsOcrProvider {
    fn extract_text_from_file(&self, image_path: &Path) -> AppResult<String> {
        if !image_path.exists() {
            return Err(AppError::General("Image file does not exist for OCR".to_string()));
        }

        #[cfg(target_os = "windows")]
        {
            return extract_text_windows_winrt(image_path);
        }

        #[cfg(target_os = "macos")]
        {
            return extract_text_macos_vision(image_path);
        }

        #[cfg(target_os = "linux")]
        {
            return extract_text_linux_fallback(image_path);
        }
    }
}

#[cfg(target_os = "windows")]
fn extract_text_windows_winrt(image_path: &Path) -> AppResult<String> {
    use image::imageops::FilterType;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    let decoded = image::open(image_path)?;
    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(4096).max(512);

    let rgba = if decoded.width() > max_dim || decoded.height() > max_dim {
        decoded
            .resize(max_dim, max_dim, FilterType::Triangle)
            .to_rgba8()
    } else {
        decoded.to_rgba8()
    };

    let width = rgba.width();
    let height = rgba.height();
    if width == 0 || height == 0 {
        return Ok(String::new());
    }

    // Convert RGBA8 -> BGRA8 in RAM so `Windows.Media.Ocr` works on `.webp`, `.png`, and `.bmp`
    // without requiring external Windows Store WebP WIC codecs!
    let mut bgra = rgba.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }

    let writer = DataWriter::new().map_err(|e| AppError::General(e.to_string()))?;
    writer
        .WriteBytes(&bgra)
        .map_err(|e| AppError::General(e.to_string()))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|e| AppError::General(e.to_string()))?;

    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
    )
    .map_err(|e| AppError::General(e.to_string()))?;

    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| AppError::General(e.to_string()))?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| AppError::General(e.to_string()))?
        .get()
        .map_err(|e| AppError::General(e.to_string()))?;

    // Join recognized lines with newlines so multi-line code/chat logs remain readable
    if let Ok(lines) = result.Lines() {
        let mut collected = Vec::new();
        for line in lines {
            if let Ok(t) = line.Text() {
                let s = t.to_string();
                if !s.trim().is_empty() {
                    collected.push(s);
                }
            }
        }
        if !collected.is_empty() {
            return Ok(collected.join("\n"));
        }
    }

    let text = result
        .Text()
        .map(|h| h.to_string())
        .unwrap_or_default();
    Ok(text.trim().to_string())
}

#[cfg(target_os = "macos")]
fn extract_text_macos_vision(image_path: &Path) -> AppResult<String> {
    use std::process::Command;
    let script = r#"
import Foundation
import Vision
import AppKit
let path = CommandLine.arguments[1]
guard let img = NSImage(contentsOfFile: path),
      let cgImg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else { exit(0) }
let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
let handler = VNImageRequestHandler(cgImage: cgImg, options: [:])
try? handler.perform([req])
let text = (req.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
print(text)
"#;
    if let Ok(out) = Command::new("swift")
        .args(["-e", script, &image_path.to_string_lossy()])
        .output()
    {
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    Ok(String::new())
}

#[cfg(target_os = "linux")]
fn extract_text_linux_fallback(image_path: &Path) -> AppResult<String> {
    use std::process::Command;
    let path_str = image_path.to_string_lossy().to_string();

    if let Ok(out) = Command::new("ocrs").arg(&path_str).output() {
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }

    if let Ok(out) = Command::new("tesseract")
        .args([&path_str, "stdout", "--oem", "1", "quiet"])
        .output()
    {
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }

    Ok(String::new())
}

/// Runs OCR asynchronously on a newly saved capture (`capture_id`, `file_path`),
/// updates `captures.ocr_text` + `captures.ocr_status` + `captures_fts`, and then
/// runs the non-destructive 3-day storage quota check.
pub fn spawn_background_ocr_and_storage_check(
    app: AppHandle,
    capture_id: String,
    file_path: String,
) {
    std::thread::spawn(move || {
        let provider = NativeOsOcrProvider;
        let path = Path::new(&file_path);

        match provider.extract_text_from_file(path) {
            Ok(extracted_text) => {
                let state: tauri::State<'_, AppState> = app.state();
                if let Ok(conn) = state.db.lock() {
                    let _ = update_capture_ocr(&conn, &capture_id, &extracted_text, "done");
                }
                let _ = app.emit(
                    "gallery:ocr_updated",
                    serde_json::json!({
                        "id": capture_id,
                        "ocrText": extracted_text,
                        "ocrStatus": "done"
                    }),
                );
            }
            Err(_) => {
                let state: tauri::State<'_, AppState> = app.state();
                if let Ok(conn) = state.db.lock() {
                    let _ = update_capture_ocr(&conn, &capture_id, "", "failed");
                };
            }
        }

        // Coordinate with Milestone 1.5.3: after OCR finishes on the new capture,
        // run the storage quota pass so any oldest unpinned captures exceeding quota
        // are moved into the 3-day "Queued Deletion" grace buffer.
        let _ = crate::services::storage_cleanup::enforce_storage_policies_now(&app);
    });
}

/// Scans all pending (`ocr_status = 'pending'`) screenshots in the active folder on a background thread
/// and returns how many items were queued/processed.
pub fn run_ocr_backfill_batch(app: &AppHandle, max_items: usize) -> AppResult<usize> {
    if OCR_BACKFILL_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(0);
    }

    let pending: Vec<(String, String)> = {
        let state: tauri::State<'_, AppState> = app.state();
        let active_dir = state
            .captures_dir
            .read()
            .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
            .clone();
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        list_pending_ocr_captures(&conn, Some(&active_dir), max_items)?
    };

    let count = pending.len();
    if count == 0 {
        OCR_BACKFILL_RUNNING.store(false, Ordering::SeqCst);
        return Ok(0);
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        let provider = NativeOsOcrProvider;
        for (id, file_path) in pending {
            let path = Path::new(&file_path);
            if !path.exists() {
                let state: tauri::State<'_, AppState> = app_clone.state();
                if let Ok(conn) = state.db.lock() {
                    let _ = update_capture_ocr(&conn, &id, "", "failed");
                };
                continue;
            }

            match provider.extract_text_from_file(path) {
                Ok(text) => {
                    let state: tauri::State<'_, AppState> = app_clone.state();
                    if let Ok(conn) = state.db.lock() {
                        let _ = update_capture_ocr(&conn, &id, &text, "done");
                    };
                }
                Err(_) => {
                    let state: tauri::State<'_, AppState> = app_clone.state();
                    if let Ok(conn) = state.db.lock() {
                        let _ = update_capture_ocr(&conn, &id, "", "failed");
                    };
                }
            }
        }
        OCR_BACKFILL_RUNNING.store(false, Ordering::SeqCst);
        let _ = app_clone.emit("gallery:updated", true);
    });

    Ok(count)
}
