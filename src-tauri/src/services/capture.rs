use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use chrono::Utc;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use uuid::Uuid;
use xcap::Monitor;

use crate::error::{AppError, AppResult};
use crate::state::{CaptureSessionMeta, FrozenFrame};

pub struct RawMonitorCapture {
    pub monitor_id: u32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub rgba_bytes: Vec<u8>,
}

/// Cross-platform trait abstraction for screen capture (Desktop via xcap now,
/// Android MediaProjection / iOS Shortcuts & ReplayKit in Phase 3.0).
pub trait ScreenCaptureProvider: Send + Sync {
    fn capture_active_monitor(&self) -> AppResult<RawMonitorCapture>;
}

pub struct XcapCaptureProvider;

impl ScreenCaptureProvider for XcapCaptureProvider {
    fn capture_active_monitor(&self) -> AppResult<RawMonitorCapture> {
        let monitors = Monitor::all().map_err(|e| AppError::Capture(e.to_string()))?;
        let primary = monitors
            .into_iter()
            .find(|m| m.is_primary())
            .or_else(|| Monitor::all().ok()?.into_iter().next())
            .ok_or_else(|| AppError::Capture("No active monitor detected".to_string()))?;

        let image = primary
            .capture_image()
            .map_err(|e| AppError::Capture(e.to_string()))?;

        let width = image.width();
        let height = image.height();
        let scale_factor = primary.scale_factor() as f64;
        let monitor_id = primary.id();

        Ok(RawMonitorCapture {
            monitor_id,
            width,
            height,
            scale_factor,
            rgba_bytes: image.into_raw(),
        })
    }
}

/// Captures the active monitor and writes an uncompressed BMP in RAM (< 3ms)
/// so the `overlay` window can stream it via `http://capture.localhost/current`
/// with zero PNG compression lag and zero Base64 JSON overhead.
pub fn freeze_active_screen(provider: &dyn ScreenCaptureProvider) -> AppResult<FrozenFrame> {
    let raw = provider.capture_active_monitor()?;
    let rgba_img = RgbaImage::from_raw(raw.width, raw.height, raw.rgba_bytes.clone())
        .ok_or_else(|| AppError::Capture("Invalid RGBA buffer dimensions".to_string()))?;

    // Use BMP in RAM so encoding takes ~2ms instead of ~250ms for PNG compression
    let mut bmp_buf = Vec::with_capacity((raw.width * raw.height * 4 + 128) as usize);
    DynamicImage::ImageRgba8(rgba_img)
        .write_to(&mut Cursor::new(&mut bmp_buf), ImageFormat::Bmp)?;

    let meta = CaptureSessionMeta {
        session_id: Uuid::new_v4().to_string(),
        monitor_id: raw.monitor_id,
        width: raw.width,
        height: raw.height,
        scale_factor: raw.scale_factor,
        captured_at: Utc::now().to_rfc3339(),
    };

    Ok(FrozenFrame {
        meta,
        bmp_bytes: Arc::new(bmp_buf),
        raw_rgba: Arc::new(raw.rgba_bytes),
    })
}

/// Fast path for `Alt + Shift + S` (Instant Fullscreen Capture): saves directly from
/// the raw RGBA pixel buffer without double-encoding/decoding through PNG.
pub fn persist_raw_rgba_and_thumbnail(
    raw_rgba: &[u8],
    width: u32,
    height: u32,
    captures_dir: &Path,
    thumbs_dir: &Path,
    ext: &str,
) -> AppResult<(String, String, u64)> {
    let id = Uuid::new_v4().to_string();
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let safe_ext = if ext == "png" { "png" } else { "webp" };

    let file_name = format!("RV_{}_{}.{}", timestamp, &id[..8], safe_ext);
    let thumb_name = format!("RV_{}_{}_thumb.webp", timestamp, &id[..8]);

    let file_path = captures_dir.join(file_name);
    let thumb_path = thumbs_dir.join(thumb_name);

    let rgba_img = RgbaImage::from_raw(width, height, raw_rgba.to_vec())
        .ok_or_else(|| AppError::Capture("Invalid RGBA buffer".to_string()))?;
    let dynamic = DynamicImage::ImageRgba8(rgba_img);

    let format = if safe_ext == "png" {
        ImageFormat::Png
    } else {
        ImageFormat::WebP
    };
    dynamic.save_with_format(&file_path, format)?;

    let size_bytes = std::fs::metadata(&file_path)?.len();

    let thumb_width = 320.min(width);
    let thumb_height = ((height as f64 / width.max(1) as f64) * thumb_width as f64)
        .round()
        .max(1.0) as u32;
    let thumbnail = dynamic.thumbnail(thumb_width, thumb_height);
    thumbnail.save_with_format(&thumb_path, ImageFormat::WebP)?;

    Ok((
        file_path.to_string_lossy().to_string(),
        thumb_path.to_string_lossy().to_string(),
        size_bytes,
    ))
}

/// Writes the flattened annotated image to disk and generates a 320px WebP thumbnail
/// so the virtualized gallery never loads full-resolution 4K images into grid cards.
pub fn persist_capture_and_thumbnail(
    image_bytes: &[u8],
    captures_dir: &Path,
    thumbs_dir: &Path,
    ext: &str,
) -> AppResult<(String, String, u32, u32, u64)> {
    let id = Uuid::new_v4().to_string();
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let safe_ext = if ext == "png" { "png" } else { "webp" };

    let file_name = format!("RV_{}_{}.{}", timestamp, &id[..8], safe_ext);
    let thumb_name = format!("RV_{}_{}_thumb.webp", timestamp, &id[..8]);

    let file_path = captures_dir.join(file_name);
    let thumb_path = thumbs_dir.join(thumb_name);

    let decoded = image::load_from_memory(image_bytes)?;
    let width = decoded.width();
    let height = decoded.height();

    let format = if safe_ext == "png" {
        ImageFormat::Png
    } else {
        ImageFormat::WebP
    };
    decoded.save_with_format(&file_path, format)?;

    let size_bytes = std::fs::metadata(&file_path)?.len();

    let thumb_width = 320.min(width);
    let thumb_height = ((height as f64 / width.max(1) as f64) * thumb_width as f64)
        .round()
        .max(1.0) as u32;
    let thumbnail = decoded.thumbnail(thumb_width, thumb_height);
    thumbnail.save_with_format(&thumb_path, ImageFormat::WebP)?;

    Ok((
        file_path.to_string_lossy().to_string(),
        thumb_path.to_string_lossy().to_string(),
        width,
        height,
        size_bytes,
    ))
}

/// Generates a real sample WebP image on disk for synthetic benchmark entries
/// so `+500 Benchmark Items` works in the native Tauri desktop app and passes orphan checks.
pub fn ensure_benchmark_sample_image(
    captures_dir: &Path,
    thumbs_dir: &Path,
    variant: usize,
) -> AppResult<(String, String, u64)> {
    let file_path = captures_dir.join(format!("RV_benchmark_sample_{}.webp", variant));
    let thumb_path = thumbs_dir.join(format!("RV_benchmark_sample_{}_thumb.webp", variant));

    if file_path.exists() && thumb_path.exists() {
        let size = std::fs::metadata(&file_path)?.len();
        return Ok((
            file_path.to_string_lossy().to_string(),
            thumb_path.to_string_lossy().to_string(),
            size,
        ));
    }

    let w = 640u32;
    let h = 360u32;
    let mut img = RgbaImage::new(w, h);
    let palettes: [(u8, u8, u8); 5] = [
        (37, 99, 235),  // Blue
        (16, 185, 129), // Emerald
        (245, 158, 11), // Amber
        (225, 29, 72),  // Rose
        (124, 58, 237), // Violet
    ];
    let (r_acc, g_acc, b_acc) = palettes[variant % palettes.len()];

    for y in 0..h {
        for x in 0..w {
            let border = x < 4 || x >= w - 4 || y < 4 || y >= h - 4;
            let stripe = (x + y) % 48 == 0;
            if border {
                img.put_pixel(x, y, Rgba([r_acc, g_acc, b_acc, 255]));
            } else if stripe {
                img.put_pixel(x, y, Rgba([r_acc / 2, g_acc / 2, b_acc / 2, 255]));
            } else {
                img.put_pixel(x, y, Rgba([15, 23, 42, 255]));
            }
        }
    }

    let dyn_img = DynamicImage::ImageRgba8(img);
    dyn_img.save_with_format(&file_path, ImageFormat::WebP)?;
    let thumb = dyn_img.thumbnail(320, 180);
    thumb.save_with_format(&thumb_path, ImageFormat::WebP)?;

    let size = std::fs::metadata(&file_path)?.len();
    Ok((
        file_path.to_string_lossy().to_string(),
        thumb_path.to_string_lossy().to_string(),
        size,
    ))
}
