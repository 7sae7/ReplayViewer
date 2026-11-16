use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::db::captures_repo::CaptureRecord;
use crate::error::{AppError, AppResult};
use crate::services::audio::{AudioInputDevice, AudioSessionInfo};
use crate::services::ffmpeg::{
    download_minimal_ffmpeg_to_dir, get_cached_or_probe_ffmpeg, FfmpegStatus,
};
use crate::services::replay::{
    cleanup_video_track_previews, prepare_video_track_previews, probe_capture_video_info,
    save_replay_clip, trim_video_clip, FfmpegReplayBufferProvider, ReplayBufferProvider,
    ReplayBufferSnapshot, VideoAudioTrackPreview, VideoProbeInfo,
};
use crate::state::{AppState, ReplayRuntimeState};

#[tauri::command]
pub async fn get_ffmpeg_status(
    app: AppHandle,
    force_refresh: Option<bool>,
) -> AppResult<FfmpegStatus> {
    let force = force_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<'_, AppState> = app.state();
        let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
        Ok(get_cached_or_probe_ffmpeg(
            &state,
            &cfg.ffmpeg_custom_path,
            force,
        ))
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn download_ffmpeg_engine(app: AppHandle) -> AppResult<FfmpegStatus> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<'_, AppState> = app.state();

        // Prevent duplicate concurrent downloads when navigating between tabs
        if state.ffmpeg_downloading.swap(true, Ordering::SeqCst) {
            let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
            return Ok(get_cached_or_probe_ffmpeg(
                &state,
                &cfg.ffmpeg_custom_path,
                false,
            ));
        }

        struct DownloadResetGuard<'a> {
            state: &'a AppState,
        }
        impl<'a> Drop for DownloadResetGuard<'a> {
            fn drop(&mut self) {
                self.state.ffmpeg_downloading.store(false, Ordering::SeqCst);
                if let Ok(mut g) = self.state.ffmpeg_download_stage.write() {
                    g.clear();
                }
            }
        }
        let _guard = DownloadResetGuard { state: &state };

        // Pause any active replay child while installing/upgrading managed binary
        state.replay_restart_requested.store(true, Ordering::SeqCst);

        let mut status =
            download_minimal_ffmpeg_to_dir(&app, &state, &state.replay_bin_dir)?;
        status.downloading = false;
        status.download_stage = None;
        if let Ok(mut guard) = state.cached_ffmpeg_status.write() {
            *guard = Some(status.clone());
        }
        state.replay_restart_requested.store(true, Ordering::SeqCst);
        Ok(status)
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn get_replay_buffer_snapshot(app: AppHandle) -> AppResult<ReplayBufferSnapshot> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<'_, AppState> = app.state();
        let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
        let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
        let runtime = state
            .replay_runtime
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| ReplayRuntimeState::default());

        Ok(ReplayBufferSnapshot {
            runtime,
            buffer_secs: cfg.replay_buffer_secs,
            fps: cfg.replay_fps,
            bitrate_mbps: cfg.replay_bitrate_mbps,
            configured_encoder: cfg.replay_encoder,
            ffmpeg_available: ff.available,
            ffmpeg_source: ff.source,
            available_encoders: ff.available_encoders,
            recommended_encoder: ff.recommended_encoder,
        })
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub fn set_replay_buffer_active(
    app: AppHandle,
    state: State<'_, AppState>,
    active: bool,
) -> AppResult<bool> {
    if active {
        if state.ffmpeg_downloading.load(Ordering::SeqCst) {
            return Err(AppError::General(
                "FFmpeg Engine download/upgrade is in progress. Please wait until it completes."
                    .to_string(),
            ));
        }
        let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
        let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
        let needs_upgrade =
            cfg!(target_os = "windows") && ff.source == "system" && !ff.supports_ddagrab;
        if !ff.available || needs_upgrade {
            return Err(AppError::General(
                "Please Download or Upgrade the FFmpeg Engine in Settings before starting Hardware Replay Buffer."
                    .to_string(),
            ));
        }
    }

    if let Ok(conn) = state.db.lock() {
        let val = if active { "true" } else { "false" };
        let _ = conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES ('replay_enabled', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            rusqlite::params![val],
        );
    }
    state.replay_should_run.store(active, Ordering::SeqCst);
    state.replay_restart_requested.store(true, Ordering::SeqCst);

    if let Ok(mut rt) = state.replay_runtime.write() {
        if active {
            rt.last_error = None;
        } else {
            rt.is_recording = false;
        }
        let _ = app.emit("replay:status_changed", rt.clone());
    }
    Ok(active)
}

#[tauri::command]
pub async fn save_replay_buffer(app: AppHandle) -> AppResult<CaptureRecord> {
    tauri::async_runtime::spawn_blocking(move || FfmpegReplayBufferProvider.save_clip(&app))
        .await
        .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn probe_video_audio_tracks(
    app: AppHandle,
    id: String,
) -> AppResult<VideoProbeInfo> {
    tauri::async_runtime::spawn_blocking(move || probe_capture_video_info(&app, &id))
        .await
        .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn prepare_video_track_previews_cmd(
    app: AppHandle,
    id: String,
    buckets: Option<usize>,
    force_refresh: Option<bool>,
) -> AppResult<Vec<VideoAudioTrackPreview>> {
    let bucket_count = buckets.unwrap_or(180);
    let force = force_refresh.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        prepare_video_track_previews(&app, &id, bucket_count, force)
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub fn cleanup_video_track_previews_cmd(id: Option<String>) {
    cleanup_video_track_previews(id.as_deref());
}

#[tauri::command]
pub async fn trim_video_clip_cmd(
    app: AppHandle,
    id: String,
    start_ms: u64,
    end_ms: u64,
    selected_audio_indices: Option<Vec<usize>>,
    mix_selected_audio: Option<bool>,
    target_max_mb: Option<u32>,
    replace_original: Option<bool>,
) -> AppResult<CaptureRecord> {
    tauri::async_runtime::spawn_blocking(move || {
        trim_video_clip(
            &app,
            id,
            start_ms,
            end_ms,
            selected_audio_indices,
            mix_selected_audio.unwrap_or(false),
            target_max_mb,
            replace_original.unwrap_or(false),
        )
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn list_active_audio_sessions(
    app: AppHandle,
    include_idle: Option<bool>,
) -> AppResult<Vec<AudioSessionInfo>> {
    let idle = include_idle.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        crate::services::audio::list_active_audio_sessions(&app, idle)
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn get_live_audio_peaks() -> AppResult<std::collections::HashMap<String, f32>> {
    tauri::async_runtime::spawn_blocking(crate::services::audio::get_live_app_audio_peaks)
        .await
        .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn set_audio_track_volumes(
    app: AppHandle,
    desktop_volume: Option<f32>,
    mic_volume: Option<f32>,
    app_exe_name: Option<String>,
    app_volume: Option<f32>,
) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<'_, AppState> = app.state();

        // Update live atomic gains immediately (< 1 µs) — NEVER restarts FFmpeg or drops segments!
        if let Some(dv) = desktop_volume {
            state
                .audio_desktop_gain_bits
                .store(dv.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
        }
        if let Some(mv) = mic_volume {
            let clamped = mv.clamp(0.0, 2.0);
            state
                .audio_mic_gain_bits
                .store(clamped.to_bits(), Ordering::Relaxed);
            state
                .mic_test_gain_bits
                .store(clamped.to_bits(), Ordering::Relaxed);
        }
        if let (Some(ref exe), Some(av)) = (&app_exe_name, app_volume) {
            let key = exe.trim().to_lowercase();
            if !key.is_empty() {
                if let Ok(mut g) = state.audio_app_gains.write() {
                    g.insert(key, av.clamp(0.0, 2.0));
                }
            }
        }

        // Persist directly in SQLite without triggering replay_restart_requested
        let mut cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
        if let Some(dv) = desktop_volume {
            cfg.audio_desktop_volume = dv.clamp(0.0, 2.0);
        }
        if let Some(mv) = mic_volume {
            cfg.audio_mic_volume = mv.clamp(0.0, 2.0);
        }
        if let (Some(exe), Some(av)) = (app_exe_name, app_volume) {
            let key = exe.trim().to_lowercase();
            if !key.is_empty() {
                cfg.audio_app_volumes.insert(key, av.clamp(0.0, 2.0));
            }
        }
        if let Ok(conn) = state.db.lock() {
            let pairs = [
                ("audio_desktop_volume", serde_json::to_string(&cfg.audio_desktop_volume)?),
                ("audio_mic_volume", serde_json::to_string(&cfg.audio_mic_volume)?),
                ("audio_app_volumes", serde_json::to_string(&cfg.audio_app_volumes)?),
            ];
            for (k, v) in pairs {
                conn.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    rusqlite::params![k, v],
                )?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub fn start_mic_test(
    state: State<'_, AppState>,
    device_id: Option<String>,
    sidetone: Option<bool>,
) -> AppResult<bool> {
    // Stop any existing test thread cleanly first
    if state.mic_test_running.swap(false, Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    if let Ok(cfg) = crate::commands::settings_cmds::get_settings(state.clone()) {
        state
            .mic_test_gain_bits
            .store(cfg.audio_mic_volume.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    state
        .mic_test_sidetone
        .store(sidetone.unwrap_or(false), Ordering::SeqCst);
    state
        .mic_test_peak_bits
        .store(0.0f32.to_bits(), Ordering::Relaxed);
    state.mic_test_running.store(true, Ordering::SeqCst);

    crate::services::audio::spawn_microphone_test_worker(
        device_id.unwrap_or_default(),
        state.mic_test_running.clone(),
        state.mic_test_sidetone.clone(),
        state.mic_test_peak_bits.clone(),
        state.mic_test_gain_bits.clone(),
    );
    Ok(true)
}

#[tauri::command]
pub fn set_mic_test_gain(state: State<'_, AppState>, gain: f32) -> AppResult<f32> {
    let clamped = gain.clamp(0.0, 2.0);
    state
        .mic_test_gain_bits
        .store(clamped.to_bits(), Ordering::Relaxed);
    Ok(clamped)
}

#[tauri::command]
pub fn stop_mic_test(state: State<'_, AppState>) -> AppResult<bool> {
    state.mic_test_running.store(false, Ordering::SeqCst);
    state
        .mic_test_peak_bits
        .store(0.0f32.to_bits(), Ordering::Relaxed);
    Ok(false)
}

#[tauri::command]
pub fn set_mic_test_sidetone(state: State<'_, AppState>, enabled: bool) -> AppResult<bool> {
    state.mic_test_sidetone.store(enabled, Ordering::SeqCst);
    Ok(enabled)
}

#[tauri::command]
pub fn get_mic_test_level(state: State<'_, AppState>) -> AppResult<f32> {
    if !state.mic_test_running.load(Ordering::Relaxed) {
        return Ok(0.0);
    }
    Ok(f32::from_bits(state.mic_test_peak_bits.load(Ordering::Relaxed)))
}

#[tauri::command]
pub async fn list_audio_input_devices() -> AppResult<Vec<AudioInputDevice>> {
    tauri::async_runtime::spawn_blocking(crate::services::audio::list_audio_input_devices)
        .await
        .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn pick_ffmpeg_dialog() -> AppResult<Option<String>> {
    tauri::async_runtime::spawn_blocking(|| {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let ps_script = r#"
            Add-Type -AssemblyName System.Windows.Forms
            $dlg = New-Object System.Windows.Forms.OpenFileDialog
            $dlg.Title = 'Select ffmpeg.exe (FFmpeg 7+ recommended for ddagrab)'
            $dlg.Filter = 'FFmpeg Executable (ffmpeg.exe)|ffmpeg.exe|Executable Files (*.exe)|*.exe|All Files (*.*)|*.*'
            if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
                Write-Output $dlg.FileName
            }
            "#;
            let out = std::process::Command::new("powershell")
                .args(["-NoProfile", "-STA", "-Command", ps_script])
                .creation_flags(CREATE_NO_WINDOW)
                .output()?;
            let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if chosen.is_empty() {
                Ok(None)
            } else {
                Ok(Some(chosen))
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(None)
        }
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

#[tauri::command]
pub async fn update_app_audio_rule(
    app: AppHandle,
    exe_name: String,
    muted: Option<bool>,
    isolated: Option<bool>,
) -> AppResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state: State<'_, AppState> = app.state();
        let mut cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
        let clean_lower = exe_name.trim().to_lowercase();
        if clean_lower.is_empty() {
            return Ok(());
        }

        if let Some(m) = muted {
            cfg.audio_muted_apps
                .retain(|a| a.trim().to_lowercase() != clean_lower);
            if m {
                cfg.audio_muted_apps.push(exe_name.trim().to_string());
                cfg.audio_isolated_apps
                    .retain(|a| a.trim().to_lowercase() != clean_lower);
            }
        }

        if let Some(iso) = isolated {
            cfg.audio_isolated_apps
                .retain(|a| a.trim().to_lowercase() != clean_lower);
            if iso {
                if cfg.audio_isolated_apps.len() >= crate::services::audio::MAX_ISOLATED_TRACKS {
                    return Err(AppError::General(format!(
                        "All {} Isolated track slots are already in use. Remove an isolated track first.",
                        crate::services::audio::MAX_ISOLATED_TRACKS
                    )));
                }
                cfg.audio_isolated_apps.push(exe_name.trim().to_string());
                cfg.audio_muted_apps
                    .retain(|a| a.trim().to_lowercase() != clean_lower);
            }
        }

        // Persist directly in SQLite + hot-swap WASAPI audio route plan with ZERO FFmpeg restart!
        if let Ok(conn) = state.db.lock() {
            let pairs = [
                ("audio_muted_apps", serde_json::to_string(&cfg.audio_muted_apps)?),
                ("audio_isolated_apps", serde_json::to_string(&cfg.audio_isolated_apps)?),
            ];
            for (k, v) in pairs {
                conn.execute(
                    "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    rusqlite::params![k, v],
                )?;
            }
        }

        crate::services::audio::refresh_live_audio_route_plan(
            &app,
            &cfg.audio_muted_apps,
            &cfg.audio_isolated_apps,
        );
        Ok(())
    })
    .await
    .map_err(|e| AppError::General(e.to_string()))?
}

/// Spawns `save_replay_clip` on a background thread when triggered via OS hotkey (`Alt + C`)
/// so the global shortcut handler returns in < 1ms without blocking desktop input.
pub fn spawn_save_replay_in_background(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(err) = save_replay_clip(&app) {
            if let Some(main_win) = app.get_webview_window("main") {
                let _ = app.emit("shortcut:toast", format!("Replay Save: {}", err));
                let _ = main_win.is_visible();
            }
        }
    });
}
