use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// Metadata and raw BMP bytes of the frozen monitor buffer held in RAM
/// for zero-Base64 streaming over the `http://capture.localhost/current` URI scheme.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSessionMeta {
    pub session_id: String,
    pub monitor_id: u32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub captured_at: String,
}

#[derive(Clone)]
pub struct FrozenFrame {
    pub meta: CaptureSessionMeta,
    pub bmp_bytes: Arc<Vec<u8>>,
    pub raw_rgba: Arc<Vec<u8>>,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    pub latest_capture: RwLock<Option<FrozenFrame>>,
    pub captures_dir: RwLock<PathBuf>,
    pub thumbs_dir: RwLock<PathBuf>,
}
