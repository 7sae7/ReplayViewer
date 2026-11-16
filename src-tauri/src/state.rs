use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64},
    Arc, Mutex, RwLock,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri_plugin_global_shortcut::Shortcut;

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
    pub source_clipboard_id: Option<String>,
    pub initial_annotations_json: Option<String>,
}

#[derive(Clone)]
pub struct FrozenFrame {
    pub meta: CaptureSessionMeta,
    pub bmp_bytes: Arc<Vec<u8>>,
    pub raw_rgba: Arc<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct ActiveShortcuts {
    pub region_capture: Option<Shortcut>,
    pub fullscreen_capture: Option<Shortcut>,
    pub open_gallery: Option<Shortcut>,
    pub visual_clipboard: Option<Shortcut>,
    pub save_replay: Option<Shortcut>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayRuntimeState {
    pub is_recording: bool,
    pub active_encoder: String,
    pub capture_method: String,
    pub segment_count: usize,
    pub ring_bytes: u64,
    pub active_tracks: Vec<String>,
    pub last_error: Option<String>,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    pub latest_capture: RwLock<Option<FrozenFrame>>,
    pub captures_dir: RwLock<PathBuf>,
    pub thumbs_dir: RwLock<PathBuf>,
    pub clipboard_cache_dir: PathBuf,
    pub replay_bin_dir: PathBuf,
    pub replay_ring_dir: PathBuf,
    pub clipboard_paused: AtomicBool,
    pub active_clipboard_hash: RwLock<Option<String>>,
    pub active_shortcuts: RwLock<ActiveShortcuts>,
    pub shortcut_recording: AtomicBool,
    pub replay_should_run: AtomicBool,
    pub replay_restart_requested: AtomicBool,
    pub replay_child_pid: AtomicU32,
    pub replay_runtime: RwLock<ReplayRuntimeState>,
    pub cached_ffmpeg_status: RwLock<Option<crate::services::ffmpeg::FfmpegStatus>>,
    pub ffmpeg_downloading: AtomicBool,
    pub ffmpeg_download_stage: RwLock<String>,
    pub mic_test_running: Arc<AtomicBool>,
    pub mic_test_sidetone: Arc<AtomicBool>,
    pub mic_test_peak_bits: Arc<AtomicU32>,
    pub mic_test_gain_bits: Arc<AtomicU32>,
    pub audio_desktop_gain_bits: Arc<AtomicU32>,
    pub audio_mic_gain_bits: Arc<AtomicU32>,
    pub audio_app_gains: Arc<RwLock<HashMap<String, f32>>>,
    pub audio_route_gen: Arc<AtomicU64>,
    pub audio_route_plan: Arc<RwLock<crate::services::audio::LiveAudioRoutePlan>>,
    pub replay_stream_slots: RwLock<Vec<(Option<usize>, String)>>,
    pub audio_last_active_ms: Mutex<HashMap<u32, u64>>,
    pub slot_had_audio: Arc<[AtomicBool; 4]>,
}
