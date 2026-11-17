use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use chrono::Utc;
use image::{imageops::FilterType, ImageFormat};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::db::captures_repo::{delete_captures, get_capture_by_id, insert_capture, CaptureRecord};
use crate::error::{AppError, AppResult};
use crate::services::audio::{
    build_audio_track_plan, spawn_named_pipe_audio_worker, AudioPipeTrack, LiveAudioGains,
};
use crate::services::ffmpeg::{get_cached_or_probe_ffmpeg, select_best_encoder};
use crate::state::{AppState, ReplayRuntimeState};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoAudioTrack {
    /// 0-based audio stream index (`0:a:{stream_index}`)
    pub stream_index: usize,
    pub title: String,
    pub channels: u32,
    pub codec: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProbeInfo {
    pub duration_ms: u64,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub audio_tracks: Vec<VideoAudioTrack>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoAudioTrackPreview {
    pub stream_index: usize,
    pub title: String,
    pub audio_path: String,
    pub peaks: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBufferSnapshot {
    pub runtime: ReplayRuntimeState,
    pub buffer_secs: u32,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub configured_encoder: String,
    pub ffmpeg_available: bool,
    pub ffmpeg_source: String,
    pub available_encoders: Vec<String>,
    pub recommended_encoder: String,
}

/// Cross-platform abstraction for segmented rolling video replay buffers (`ARCHITECTURE.md` §6).
pub trait ReplayBufferProvider {
    fn save_clip(&self, app: &AppHandle) -> AppResult<CaptureRecord>;
}

pub struct FfmpegReplayBufferProvider;

impl ReplayBufferProvider for FfmpegReplayBufferProvider {
    fn save_clip(&self, app: &AppHandle) -> AppResult<CaptureRecord> {
        save_replay_clip(app)
    }
}

fn new_hidden_command(bin: &str) -> Command {
    let mut cmd = Command::new(bin);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Lists sorted `.ts` ring segment paths and their total disk size in `ring_dir`.
fn list_sorted_segments(ring_dir: &Path) -> Vec<(u64, PathBuf, u64)> {
    let Ok(entries) = fs::read_dir(ring_dir) else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("seg_") || !name.ends_with(".ts") {
            continue;
        }
        let idx_str = name
            .trim_start_matches("seg_")
            .trim_end_matches(".ts");
        if let Ok(idx) = idx_str.parse::<u64>() {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            list.push((idx, p, size));
        }
    }
    list.sort_by_key(|(idx, _, _)| *idx);
    list
}

/// Prunes only `.ts` segments older than `keep_count` so Alt-Tab & mode switches never wipe history.
fn prune_old_segments(ring_dir: &Path, keep_count: usize) -> (usize, u64) {
    let segments = list_sorted_segments(ring_dir);
    let total = segments.len();
    if total <= keep_count {
        let bytes: u64 = segments.iter().map(|(_, _, sz)| *sz).sum();
        return (total, bytes);
    }

    let remove_cnt = total - keep_count;
    for (_, path, _) in segments.iter().take(remove_cnt) {
        let _ = fs::remove_file(path);
    }

    let remaining = &segments[remove_cnt..];
    let bytes: u64 = remaining.iter().map(|(_, _, sz)| *sz).sum();
    (remaining.len(), bytes)
}

fn next_segment_start_number(ring_dir: &Path) -> u64 {
    list_sorted_segments(ring_dir)
        .last()
        .map(|(idx, _, _)| idx + 1)
        .unwrap_or(0)
}

fn update_runtime_state(
    app: &AppHandle,
    is_recording: bool,
    active_encoder: String,
    capture_method: String,
    active_tracks: Vec<String>,
    last_error: Option<String>,
) {
    let state: tauri::State<'_, AppState> = app.state();
    let segments = list_sorted_segments(&state.replay_ring_dir);
    let seg_count = segments.len();
    let ring_bytes: u64 = segments.iter().map(|(_, _, sz)| *sz).sum();

    let next_rt = ReplayRuntimeState {
        is_recording,
        active_encoder,
        capture_method,
        segment_count: seg_count,
        ring_bytes,
        active_tracks,
        last_error,
    };

    if let Ok(mut guard) = state.replay_runtime.write() {
        *guard = next_rt.clone();
    }
    let _ = app.emit("replay:status_changed", &next_rt);
}

fn kill_child_process(child: &mut Child, state: &AppState) {
    state.replay_child_pid.store(0, Ordering::SeqCst);
    // Ask FFmpeg to flush & finalize its current .ts segment cleanly via 'q\n' on stdin
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    let deadline = Instant::now() + Duration::from_millis(1200);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawns the background Replay Buffer supervisor loop:
/// - Resolves FFmpeg and auto-detects the user's best GPU hardware encoder (`NVENC -> AMF -> QSV -> MF -> x264`).
/// - Runs non-destructive 2-second `.ts` rolling segmentation (`ddagrab` GPU texture capture on Windows with `gdigrab` fallback).
/// - Survives Alt-Tab & window focus changes cleanly without clearing prior segments.
pub fn spawn_replay_supervisor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut use_ddagrab_if_supported = true;
        let mut reported_stopped = false;
        let mut reported_missing_ffmpeg = false;

        loop {
            let state: tauri::State<'_, AppState> = app.state();
            if !state.replay_should_run.load(Ordering::Relaxed) {
                reported_missing_ffmpeg = false;
                if !reported_stopped {
                    let _ = fs::create_dir_all(&state.replay_ring_dir);
                    let cfg = crate::commands::settings_cmds::get_settings(state.clone()).ok();
                    let custom = cfg.as_ref().map(|c| c.ffmpeg_custom_path.as_str()).unwrap_or("");
                    let ff_status = get_cached_or_probe_ffmpeg(&state, custom, false);
                    update_runtime_state(
                        &app,
                        false,
                        ff_status.recommended_encoder,
                        "Stopped".to_string(),
                        Vec::new(),
                        None,
                    );
                    reported_stopped = true;
                }
                std::thread::sleep(Duration::from_millis(400));
                continue;
            }
            reported_stopped = false;

            state.replay_restart_requested.store(false, Ordering::SeqCst);

            let Ok(cfg) = crate::commands::settings_cmds::get_settings(state.clone()) else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };

            if state.ffmpeg_downloading.load(Ordering::Relaxed) {
                update_runtime_state(
                    &app,
                    false,
                    "auto".to_string(),
                    "Downloading FFmpeg".to_string(),
                    Vec::new(),
                    None,
                );
                std::thread::sleep(Duration::from_millis(400));
                continue;
            }

            let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
            let needs_windows_upgrade =
                cfg!(target_os = "windows") && ff.source == "system" && !ff.supports_ddagrab;
            let Some(bin_str) = ff.binary_path.clone().filter(|_| !needs_windows_upgrade) else {
                if !reported_missing_ffmpeg {
                    let msg = if needs_windows_upgrade {
                        "Legacy system FFmpeg found without ddagrab. Click 'Upgrade Engine' in Settings -> Hardware Replay Buffer."
                    } else {
                        "FFmpeg not found in PATH. Click 'Download Engine' in Settings -> Hardware Replay Buffer."
                    };
                    update_runtime_state(
                        &app,
                        false,
                        "auto".to_string(),
                        "Waiting for FFmpeg".to_string(),
                        Vec::new(),
                        Some(msg.to_string()),
                    );
                    reported_missing_ffmpeg = true;
                }
                // Sleep calmly without re-probing OS PATH until user clicks Download Engine, sets Custom Path, or toggles buffer
                for _ in 0..40 {
                    if state.replay_restart_requested.load(Ordering::Relaxed)
                        || !state.replay_should_run.load(Ordering::Relaxed)
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                continue;
            };
            reported_missing_ffmpeg = false;

            let _ = fs::create_dir_all(&state.replay_ring_dir);
            let encoder = if cfg.replay_encoder.trim().is_empty()
                || cfg.replay_encoder.trim() == "auto"
            {
                ff.recommended_encoder.clone()
            } else {
                select_best_encoder(&cfg.replay_encoder, &ff.available_encoders, None)
            };
            let fps = cfg.replay_fps.clamp(15, 120);
            let bitrate_mbps = cfg.replay_bitrate_mbps.clamp(5, 80);
            let buffer_secs = cfg.replay_buffer_secs.clamp(30, 1200);
            // Keep +3 headroom segments so we always have full `buffer_secs` even mid-rotation
            let keep_segments = ((buffer_secs as usize) / 2).saturating_add(3);

            // Reset audio activity flags for this epoch
            for flag in state.slot_had_audio.iter() {
                flag.store(false, Ordering::Relaxed);
            }

            // Build dynamic audio track plan (Desktop Remainder + Isolated Active Apps + Mic)
            let audio_tracks: Vec<AudioPipeTrack> = if cfg!(target_os = "windows") {
                build_audio_track_plan(
                    &app,
                    cfg.audio_record_desktop,
                    cfg.audio_record_mic,
                    cfg.audio_desktop_volume,
                    cfg.audio_mic_volume,
                    &cfg.audio_app_volumes,
                    &cfg.audio_mic_device_id,
                    &cfg.audio_muted_apps,
                    &cfg.audio_isolated_apps,
                )
            } else {
                Vec::new()
            };

            let pipe_stop = Arc::new(AtomicBool::new(false));
            let live_gains = LiveAudioGains {
                desktop_bits: state.audio_desktop_gain_bits.clone(),
                mic_bits: state.audio_mic_gain_bits.clone(),
                app_gains: state.audio_app_gains.clone(),
                route_gen: state.audio_route_gen.clone(),
                route_plan: state.audio_route_plan.clone(),
                slot_had_audio: state.slot_had_audio.clone(),
            };
            let mut audio_workers = Vec::new();
            for track in &audio_tracks {
                let had_flag = if let crate::services::audio::AudioSourceKind::ProcessSlot { slot_idx } = track.kind {
                    if let Ok(plan_g) = state.audio_route_plan.read() {
                        if plan_g.isolated_slots.get(slot_idx).and_then(|o| o.as_ref()).is_some() {
                            state.slot_had_audio[slot_idx].store(true, Ordering::Relaxed);
                        }
                    }
                    Arc::new(AtomicBool::new(false))
                } else {
                    Arc::new(AtomicBool::new(false))
                };
                audio_workers.push(spawn_named_pipe_audio_worker(
                    track.clone(),
                    pipe_stop.clone(),
                    had_flag,
                    live_gains.clone(),
                ));
            }
            // Allow named pipe servers a brief moment to bind before FFmpeg connects
            if !audio_tracks.is_empty() {
                std::thread::sleep(Duration::from_millis(20));
            }

            let start_num = next_segment_start_number(&state.replay_ring_dir);
            let seg_pattern = state
                .replay_ring_dir
                .join("seg_%06d.ts")
                .to_string_lossy()
                .to_string();

            let use_ddagrab = cfg!(target_os = "windows") && ff.supports_ddagrab && use_ddagrab_if_supported;
            let capture_method_label = if use_ddagrab {
                "DXGI Desktop Duplication (ddagrab)".to_string()
            } else if cfg!(target_os = "windows") {
                "Windows Desktop GDI (gdigrab)".to_string()
            } else if cfg!(target_os = "macos") {
                "macOS AVFoundation".to_string()
            } else {
                "Linux X11 / PipeWire".to_string()
            };

            let mut cmd = new_hidden_command(&bin_str);
            cmd.args(["-hide_banner", "-loglevel", "error"]);

            // Input 0: Video source
            #[cfg(target_os = "windows")]
            {
                if use_ddagrab {
                    cmd.args([
                        "-f",
                        "lavfi",
                        "-i",
                        &format!(
                            "ddagrab=framerate={}:draw_mouse=1,hwdownload,format=bgra,format=yuv420p",
                            fps
                        ),
                    ]);
                } else {
                    cmd.args([
                        "-f",
                        "gdigrab",
                        "-framerate",
                        &fps.to_string(),
                        "-draw_mouse",
                        "1",
                        "-i",
                        "desktop",
                    ]);
                }
            }

            #[cfg(target_os = "macos")]
            {
                cmd.args([
                    "-f",
                    "avfoundation",
                    "-framerate",
                    &fps.to_string(),
                    "-capture_cursor",
                    "1",
                    "-i",
                    "1:none",
                ]);
            }

            #[cfg(target_os = "linux")]
            {
                cmd.args([
                    "-f",
                    "x11grab",
                    "-framerate",
                    &fps.to_string(),
                    "-i",
                    ":0.0",
                ]);
            }

            // Inputs 1..K: Windows WASAPI Named Pipes (48 kHz Stereo s16le)
            for track in &audio_tracks {
                cmd.args([
                    "-f",
                    "s16le",
                    "-ar",
                    "48000",
                    "-ac",
                    "2",
                    "-i",
                    &track.pipe_path,
                ]);
            }

            // Stream mapping & Audio mix:
            // - Stream 0 (a:0) is ALWAYS Master Mix (when 2+ sources are active) so HTML5 <video>,
            //   MediaLightbox, and Discord play BOTH Desktop + Microphone by default.
            // - When Single Mix is OFF, discrete tracks (Desktop, Isolated Apps, Microphone) are
            //   additionally preserved as a:1..a:K for NLE multi-track editing!
            cmd.args(["-map", "0:v"]);
            let audio_map_args = build_audio_mapping_args(&audio_tracks, cfg.audio_single_mix);
            cmd.args(&audio_map_args);

            // Record stream slot metadata so `save_replay_clip` strips unused dynamic slots cleanly
            if let Ok(mut sl_guard) = state.replay_stream_slots.write() {
                sl_guard.clear();
                if !audio_tracks.is_empty() {
                    if cfg.audio_single_mix || audio_tracks.len() == 1 {
                        sl_guard.push((None, audio_tracks[0].title.clone()));
                    } else {
                        sl_guard.push((None, "Master Mix".to_string()));
                        for t in &audio_tracks {
                            let slot = match t.kind {
                                crate::services::audio::AudioSourceKind::ProcessSlot { slot_idx } => {
                                    Some(slot_idx)
                                }
                                _ => None,
                            };
                            sl_guard.push((slot, t.title.clone()));
                        }
                    }
                }
            }

            // Video hardware encoder settings with clean GOP = 2s keyframes (`g = fps * 2`) so `-f segment -segment_time 2` cuts cleanly!
            let gop = (fps * 2).to_string();
            let br_str = format!("{}M", bitrate_mbps);
            let maxrate_str = format!("{}M", (bitrate_mbps * 3) / 2);
            let bufsize_str = format!("{}M", bitrate_mbps * 2);

            cmd.args([
                "-r",
                &fps.to_string(),
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                &encoder,
                "-b:v",
                &br_str,
                "-maxrate",
                &maxrate_str,
                "-bufsize",
                &bufsize_str,
                "-g",
                &gop,
                "-force_key_frames",
                "expr:gte(t,n_forced*2)",
            ]);
            if encoder == "h264_nvenc" {
                cmd.args(["-preset", "p4", "-tune", "ll"]);
            } else if encoder == "libx264" {
                cmd.args(["-preset", "veryfast", "-tune", "zerolatency"]);
            }

            cmd.args([
                "-max_muxing_queue_size",
                "1024",
                "-f",
                "segment",
                "-segment_time",
                "2",
                "-segment_format",
                "mpegts",
                "-reset_timestamps",
                "1",
                "-segment_start_number",
                &start_num.to_string(),
                &seg_pattern,
            ]);

            cmd.stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());

            let started_at = Instant::now();
            let track_titles: Vec<String> = audio_tracks.iter().map(|t| t.title.clone()).collect();

            match cmd.spawn() {
                Ok(mut child) => {
                    state.replay_child_pid.store(child.id(), Ordering::SeqCst);
                    update_runtime_state(
                        &app,
                        true,
                        encoder.clone(),
                        capture_method_label.clone(),
                        track_titles.clone(),
                        None,
                    );

                    let mut tick = 0u32;
                    loop {
                        std::thread::sleep(Duration::from_millis(50));
                        tick += 1;

                        if !state.replay_should_run.load(Ordering::Relaxed)
                            || state.replay_restart_requested.load(Ordering::Relaxed)
                        {
                            kill_child_process(&mut child, &state);
                            break;
                        }

                        // Every 1s (20 * 50ms) prune expired segments and broadcast ring size/count
                        if tick % 20 == 0 {
                            let _ = prune_old_segments(&state.replay_ring_dir, keep_segments);
                            update_runtime_state(
                                &app,
                                true,
                                encoder.clone(),
                                capture_method_label.clone(),
                                track_titles.clone(),
                                None,
                            );
                        }

                        if let Ok(Some(exit_status)) = child.try_wait() {
                            state.replay_child_pid.store(0, Ordering::SeqCst);
                            let runtime_ms = started_at.elapsed().as_millis();
                            let err_msg = child
                                .stderr
                                .take()
                                .and_then(|mut s| {
                                    use std::io::Read;
                                    let mut buf = String::new();
                                    let _ = s.read_to_string(&mut buf);
                                    Some(buf.trim().to_string())
                                })
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| format!("FFmpeg exited with {}", exit_status));

                            // If `ddagrab` failed quickly (e.g., hybrid GPU adapter / headless VM), auto-fallback to `gdigrab`!
                            if use_ddagrab && runtime_ms < 2500 {
                                use_ddagrab_if_supported = false;
                            }

                            update_runtime_state(
                                &app,
                                false,
                                encoder.clone(),
                                capture_method_label.clone(),
                                track_titles.clone(),
                                Some(err_msg),
                            );
                            break;
                        }
                    }
                }
                Err(err) => {
                    update_runtime_state(
                        &app,
                        false,
                        encoder,
                        capture_method_label,
                        track_titles,
                        Some(format!("Failed to start FFmpeg: {}", err)),
                    );
                }
            }

            pipe_stop.store(true, Ordering::SeqCst);
            // Wake any pipe worker still waiting in ConnectNamedPipe before joining threads
            for track in &audio_tracks {
                let _ = std::fs::OpenOptions::new().read(true).open(&track.pipe_path);
            }
            for w in audio_workers {
                let _ = w.join();
            }

            if !state.replay_restart_requested.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    });
}

/// Builds FFmpeg `-filter_complex` + `-map` + `-metadata` audio track arguments.
/// `Master Mix` (`a:0`) blends ONLY base tracks (`Desktop Audio` + `Microphone`) so isolated app
/// slots (`ProcessSlot`) never double-sum with Desktop Audio and cause comb-filtering/robotic audio.
pub fn build_audio_mapping_args(audio_tracks: &[AudioPipeTrack], single_mix: bool) -> Vec<String> {
    let mut args = Vec::new();
    if audio_tracks.is_empty() {
        return args;
    }

    let base_indices: Vec<usize> = audio_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| !matches!(t.kind, crate::services::audio::AudioSourceKind::ProcessSlot { .. }))
        .map(|(idx, _)| idx + 1)
        .collect();

    if single_mix || audio_tracks.len() == 1 {
        if base_indices.len() <= 1 {
            let single_input = base_indices.first().copied().unwrap_or(1);
            let title = audio_tracks
                .get(single_input - 1)
                .map(|t| t.title.as_str())
                .unwrap_or("Master Mix");
            args.extend([
                "-map".to_string(),
                format!("{}:a", single_input),
                "-metadata:s:a:0".to_string(),
                format!("title={}", title),
            ]);
        } else {
            let inputs: String = base_indices.iter().map(|i| format!("[{}:a]", i)).collect();
            let mix_cnt = base_indices.len();
            args.extend([
                "-filter_complex".to_string(),
                format!(
                    "{}amix=inputs={}:duration=longest:dropout_transition=0,volume={},alimiter=limit=0.95:attack=5:release=50[aout]",
                    inputs, mix_cnt, mix_cnt
                ),
                "-map".to_string(),
                "[aout]".to_string(),
                "-metadata:s:a:0".to_string(),
                "title=Master Mix".to_string(),
            ]);
        }
        args.extend([
            "-c:a".to_string(),
            "aac".to_string(),
            "-b:a".to_string(),
            "192k".to_string(),
        ]);
        return args;
    }

    // Multi-track: Stream a:0 = Master Mix (Desktop + Mic only), Streams a:1.. = discrete tracks
    if base_indices.len() >= 2 {
        let mut filter_parts: Vec<String> = Vec::with_capacity(base_indices.len() + 1);
        let mut mix_inputs = String::new();
        for &in_num in &base_indices {
            filter_parts.push(format!("[{}:a]asplit=2[mix{}][disc{}]", in_num, in_num, in_num));
            mix_inputs.push_str(&format!("[mix{}]", in_num));
        }
        let mix_cnt = base_indices.len();
        filter_parts.push(format!(
            "{}amix=inputs={}:duration=longest:dropout_transition=0,volume={},alimiter=limit=0.95:attack=5:release=50[master]",
            mix_inputs, mix_cnt, mix_cnt
        ));

        args.extend([
            "-filter_complex".to_string(),
            filter_parts.join("; "),
            "-map".to_string(),
            "[master]".to_string(),
            "-metadata:s:a:0".to_string(),
            "title=Master Mix".to_string(),
        ]);

        for (idx, track) in audio_tracks.iter().enumerate() {
            let input_num = idx + 1;
            let out_stream_idx = idx + 1;
            let map_label = if base_indices.contains(&input_num) {
                format!("[disc{}]", input_num)
            } else {
                format!("{}:a", input_num)
            };
            args.extend([
                "-map".to_string(),
                map_label,
                format!("-metadata:s:a:{}", out_stream_idx),
                format!("title={}", track.title),
            ]);
        }
    } else {
        // Only 1 base track + ProcessSlots: direct stream mapping
        let base_in = base_indices.first().copied().unwrap_or(1);
        args.extend([
            "-map".to_string(),
            format!("{}:a", base_in),
            "-metadata:s:a:0".to_string(),
            "title=Master Mix".to_string(),
        ]);
        for (idx, track) in audio_tracks.iter().enumerate() {
            let input_num = idx + 1;
            let out_stream_idx = idx + 1;
            args.extend([
                "-map".to_string(),
                format!("{}:a", input_num),
                format!("-metadata:s:a:{}", out_stream_idx),
                format!("title={}", track.title),
            ]);
        }
    }

    args.extend([
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
    ]);
    args
}

/// Immediately stitches the last `replay_buffer_secs` of 2-second `.ts` segments into a fast-start `.mp4`
/// inside `captures_dir` via lossless FFmpeg `-f concat -c copy` (zero video re-encoding),
/// generates a 320px WebP thumbnail, and indexes into SQLite + FTS5.
pub fn save_replay_clip(app: &AppHandle) -> AppResult<CaptureRecord> {
    let state: tauri::State<'_, AppState> = app.state();
    let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
    let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
    let bin_str = ff.binary_path.ok_or_else(|| {
        AppError::General("FFmpeg is not installed. Click 'Download Replay Engine' in Settings first.".to_string())
    })?;

    let all_segments = list_sorted_segments(&state.replay_ring_dir);
    if all_segments.is_empty() {
        return Err(AppError::General(
            "Replay buffer currently has 0 recorded segments. Ensure Replay Buffer is running and wait ~2 seconds.".to_string(),
        ));
    }

    let wanted_segs = ((cfg.replay_buffer_secs.clamp(30, 1200) as usize) / 2).max(1);
    // Include the live trailing MPEG-TS segment whenever it already contains valid packets (>= 8 KB),
    // and add +1 segment when available so a partially written tail segment never trims the clip 2s short!
    let trailing_is_tiny = all_segments
        .last()
        .map(|(_, _, sz)| *sz < 8192)
        .unwrap_or(false);
    let safe_end = if all_segments.len() > 1 && trailing_is_tiny {
        all_segments.len() - 1
    } else {
        all_segments.len()
    };
    let take_segs = if safe_end > wanted_segs && !trailing_is_tiny {
        wanted_segs + 1
    } else {
        wanted_segs
    };
    let safe_start = safe_end.saturating_sub(take_segs);
    let chosen: Vec<&(u64, PathBuf, u64)> = all_segments[safe_start..safe_end].iter().collect();

    if chosen.is_empty() {
        return Err(AppError::General("No finalized replay segments available yet.".to_string()));
    }

    let concat_list_path = state
        .replay_ring_dir
        .join(format!("concat_{}.txt", Uuid::new_v4()));
    {
        let mut f = File::create(&concat_list_path)?;
        for (_, seg_path, _) in &chosen {
            let escaped = seg_path
                .to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''");
            writeln!(f, "file '{}'", escaped)?;
        }
    }

    let captures_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let thumbs_dir = state
        .thumbs_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock thumbs_dir".to_string()))?
        .clone();
    fs::create_dir_all(&captures_dir)?;
    fs::create_dir_all(&thumbs_dir)?;

    let id = Uuid::new_v4().to_string();
    let stamp = Utc::now().format("%Y%m%d_%H%M%S");
    let mp4_path = captures_dir.join(format!("Replay_{}_{}.mp4", stamp, &id[..6]));
    let thumb_path = thumbs_dir.join(format!("{}_thumb.webp", id));
    let temp_png_thumb = state.replay_ring_dir.join(format!("thumb_{}.png", id));

    let mut concat_args: Vec<String> = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        concat_list_path.to_string_lossy().to_string(),
        "-map".to_string(),
        "0:v".to_string(),
    ];

    let stream_slots = state
        .replay_stream_slots
        .read()
        .map(|g| g.clone())
        .unwrap_or_default();

    if stream_slots.is_empty() {
        concat_args.extend(["-map".to_string(), "0:a?".to_string()]);
    } else {
        for (stream_idx, (slot_opt, _)) in stream_slots.iter().enumerate() {
            let keep = match slot_opt {
                None => true,
                Some(slot_idx) => state
                    .slot_had_audio
                    .get(*slot_idx)
                    .map(|b| b.load(Ordering::Relaxed))
                    .unwrap_or(false),
            };
            if keep {
                concat_args.extend(["-map".to_string(), format!("0:a:{}?", stream_idx)]);
            }
        }
    }

    concat_args.extend([
        "-c:v".to_string(),
        "copy".to_string(),
        "-c:a".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        mp4_path.to_string_lossy().to_string(),
    ]);

    let concat_out = new_hidden_command(&bin_str).args(&concat_args).output()?;

    let _ = fs::remove_file(&concat_list_path);

    if !concat_out.status.success() || !mp4_path.exists() {
        let stderr = String::from_utf8_lossy(&concat_out.stderr);
        return Err(AppError::General(format!(
            "Failed to stitch replay segments: {}",
            stderr.trim()
        )));
    }

    let (width, height, probed_duration_ms) =
        generate_video_thumbnail_and_probe(&bin_str, &temp_png_thumb, &mp4_path, &thumb_path);

    let size_bytes = fs::metadata(&mp4_path)?.len();
    let duration_ms = probed_duration_ms.unwrap_or_else(|| (chosen.len() as u64) * 2000);
    let now_iso = Utc::now().to_rfc3339();
    let dur_tag = format!("{}s", duration_ms / 1000);

    let record = CaptureRecord {
        id: id.clone(),
        media_type: "video".to_string(),
        file_path: mp4_path.to_string_lossy().to_string(),
        thumb_path: thumb_path.to_string_lossy().to_string(),
        width,
        height,
        size_bytes,
        duration_ms: Some(duration_ms),
        source_app: Some(format!(
            "Instant Replay ({}s)",
            duration_ms / 1000
        )),
        ocr_text: String::new(),
        ocr_status: "done".to_string(),
        is_pinned: false,
        group_name: "Replays".to_string(),
        last_copied_at: None,
        queued_delete_at: None,
        tags: vec!["replay".to_string(), dur_tag],
        created_at: now_iso,
    };

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        insert_capture(&conn, &record)?;
    }

    let _ = crate::services::storage_cleanup::enforce_storage_policies_now(app);
    let _ = app.emit("gallery:updated", &record);
    let _ = app.emit(
        "shortcut:toast",
        format!(
            "Saved {}s Instant Replay clip ({:.1} MB)!",
            duration_ms / 1000,
            size_bytes as f64 / (1024.0 * 1024.0)
        ),
    );

    Ok(record)
}

/// Generates a 320px WebP thumbnail and probes video `(width, height, duration_ms)` via FFmpeg/FFprobe.
fn generate_video_thumbnail_and_probe(
    bin_str: &str,
    temp_png_thumb: &Path,
    mp4_path: &Path,
    thumb_path: &Path,
) -> (u32, u32, Option<u64>) {
    let _ = new_hidden_command(bin_str)
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &mp4_path.to_string_lossy(),
            "-frames:v",
            "1",
            "-vf",
            "scale=320:-2",
            &temp_png_thumb.to_string_lossy(),
        ])
        .output();

    if temp_png_thumb.exists() {
        if let Ok(decoded) = image::open(temp_png_thumb) {
            let thumb = decoded.resize(320, 320, FilterType::Triangle);
            let _ = thumb.save_with_format(thumb_path, ImageFormat::WebP);
        }
        let _ = fs::remove_file(temp_png_thumb);
    }

    let info = probe_video_file_info(bin_str, mp4_path);
    let dur = if info.duration_ms > 0 {
        Some(info.duration_ms)
    } else {
        None
    };
    (info.width, info.height, dur)
}

/// Resolves `ffprobe` next to the resolved `ffmpeg` binary without mutating parent folder names
/// such as `ffmpeg-master-latest-win64-lgpl`.
pub fn resolve_ffprobe_binary(ffmpeg_bin: &str) -> PathBuf {
    let p = Path::new(ffmpeg_bin);
    let target_name = match p.file_name().and_then(|n| n.to_str()) {
        Some(name) if name.to_ascii_lowercase().ends_with(".exe") => {
            if name.to_ascii_lowercase().contains("ffmpeg") {
                name.replacen("ffmpeg", "ffprobe", 1)
            } else {
                "ffprobe.exe".to_string()
            }
        }
        Some(name) if name.to_ascii_lowercase().contains("ffmpeg") => {
            name.replacen("ffmpeg", "ffprobe", 1)
        }
        _ => {
            if cfg!(target_os = "windows") {
                "ffprobe.exe".to_string()
            } else {
                "ffprobe".to_string()
            }
        }
    };
    p.with_file_name(target_name)
}

/// Parses `ffmpeg -hide_banner -i <video>` stderr output for duration, resolution, and audio streams.
pub fn parse_ffmpeg_stderr_probe(stderr: &str) -> (u64, u32, u32, Vec<VideoAudioTrack>) {
    let mut duration_ms = 0u64;
    let mut width = 1920u32;
    let mut height = 1080u32;
    let mut audio_tracks: Vec<VideoAudioTrack> = Vec::new();
    let mut in_audio_stream = false;

    for line in stderr.lines() {
        let trimmed = line.trim();
        if let Some(dur_part) = trimmed.strip_prefix("Duration:") {
            if let Some(hms) = dur_part.split(',').next() {
                let parts: Vec<&str> = hms.trim().split(':').collect();
                if parts.len() == 3 {
                    if let (Ok(h), Ok(m), Ok(s)) = (
                        parts[0].parse::<f64>(),
                        parts[1].parse::<f64>(),
                        parts[2].parse::<f64>(),
                    ) {
                        duration_ms = ((h * 3600.0 + m * 60.0 + s) * 1000.0).round() as u64;
                    }
                }
            }
        }

        if trimmed.starts_with("Stream #") && trimmed.contains("Video:") {
            in_audio_stream = false;
            for token in trimmed.split([',', ' ']) {
                if let Some((w_s, h_s)) = token.split_once('x') {
                    if let (Ok(w), Ok(h)) = (w_s.parse::<u32>(), h_s.parse::<u32>()) {
                        if w >= 64 && h >= 64 {
                            width = w;
                            height = h;
                            break;
                        }
                    }
                }
            }
        } else if trimmed.starts_with("Stream #") && trimmed.contains("Audio:") {
            in_audio_stream = true;
            let idx = audio_tracks.len();
            let codec = trimmed
                .split_once("Audio:")
                .and_then(|(_, rest)| rest.trim().split([' ', ',', '(']).next())
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| "aac".to_string());
            audio_tracks.push(VideoAudioTrack {
                stream_index: idx,
                title: format!("Audio Track {}", idx + 1),
                channels: if trimmed.contains("mono") { 1 } else { 2 },
                codec,
            });
        } else if in_audio_stream && trimmed.starts_with("title") {
            if let Some((_, val)) = trimmed.split_once(':') {
                if let Some(last) = audio_tracks.last_mut() {
                    let t = val.trim();
                    if !t.is_empty() {
                        last.title = t.to_string();
                    }
                }
            }
        } else if in_audio_stream && trimmed.starts_with("handler_name") {
            if let Some((_, val)) = trimmed.split_once(':') {
                if let Some(last) = audio_tracks.last_mut() {
                    let t = val.trim();
                    if !t.is_empty()
                        && t != "SoundHandler"
                        && last.title.starts_with("Audio Track ")
                    {
                        last.title = t.to_string();
                    }
                }
            }
        } else if trimmed.starts_with("Stream #") || trimmed.starts_with("Input #") {
            in_audio_stream = false;
        }
    }

    (duration_ms, width, height, audio_tracks)
}

/// Probes video dimensions, duration, size, and every discrete audio stream (`ffprobe` JSON with `ffmpeg -i` fallback).
pub fn probe_video_file_info(bin_str: &str, video_path: &Path) -> VideoProbeInfo {
    let size_bytes = fs::metadata(video_path).map(|m| m.len()).unwrap_or(0);
    let mut width = 1920u32;
    let mut height = 1080u32;
    let mut duration_ms = 0u64;
    let mut audio_tracks: Vec<VideoAudioTrack> = Vec::new();

    let ffprobe_bin = resolve_ffprobe_binary(bin_str);
    let ffprobe_str = ffprobe_bin.to_string_lossy().to_string();
    if let Ok(out) = new_hidden_command(&ffprobe_str)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=index,codec_type,codec_name,channels,width,height:stream_tags=title,handler_name",
            "-of",
            "json",
            &video_path.to_string_lossy(),
        ])
        .output()
    {
        if out.status.success() {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(secs) = json
                    .get("format")
                    .and_then(|f| f.get("duration"))
                    .and_then(|d| d.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    if secs > 0.0 {
                        duration_ms = (secs * 1000.0).round() as u64;
                    }
                }
                if let Some(streams) = json.get("streams").and_then(|s| s.as_array()) {
                    let mut audio_ordinal = 0usize;
                    for st in streams {
                        let ctype = st.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
                        if ctype == "video" {
                            if let Some(w) = st.get("width").and_then(|v| v.as_u64()) {
                                width = w as u32;
                            }
                            if let Some(h) = st.get("height").and_then(|v| v.as_u64()) {
                                height = h as u32;
                            }
                        } else if ctype == "audio" {
                            let codec = st
                                .get("codec_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("aac")
                                .to_string();
                            let channels = st.get("channels").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
                            let tags = st.get("tags");
                            let title = tags
                                .and_then(|t| t.get("title"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .or_else(|| {
                                    tags.and_then(|t| t.get("handler_name"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.trim().to_string())
                                        .filter(|s| !s.is_empty() && s != "SoundHandler")
                                })
                                .unwrap_or_else(|| format!("Track {}", audio_ordinal + 1));
                            audio_tracks.push(VideoAudioTrack {
                                stream_index: audio_ordinal,
                                title,
                                channels,
                                codec,
                            });
                            audio_ordinal += 1;
                        }
                    }
                }
                if !audio_tracks.is_empty() {
                    return VideoProbeInfo {
                        duration_ms,
                        size_bytes,
                        width,
                        height,
                        audio_tracks,
                    };
                }
            }
        }
    }

    // Fallback: parse `ffmpeg -hide_banner -i <video>` stderr if ffprobe binary is absent or found 0 audio streams
    if let Ok(out) = new_hidden_command(bin_str)
        .args(["-hide_banner", "-i", &video_path.to_string_lossy()])
        .output()
    {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let (fb_dur, fb_w, fb_h, fb_tracks) = parse_ffmpeg_stderr_probe(&stderr);
        if duration_ms == 0 && fb_dur > 0 {
            duration_ms = fb_dur;
        }
        if fb_w > 0 && fb_h > 0 {
            width = fb_w;
            height = fb_h;
        }
        if audio_tracks.is_empty() && !fb_tracks.is_empty() {
            audio_tracks = fb_tracks;
        }
    }

    VideoProbeInfo {
        duration_ms,
        size_bytes,
        width,
        height,
        audio_tracks,
    }
}

/// Computes target `(video_kbps, audio_kbps, copy_unmixed_audio)` for `Discord 10/25/50 MB` exports
/// so audio preserves original `-c:a copy` or crisp `128–192 kbps` AAC instead of robotic `64–96 kbps`,
/// while accounting for `audio_stream_count` discrete tracks when multi-track export is kept unmixed.
pub fn compute_trim_bitrates_kbps(
    max_mb: u32,
    dur_sec: f64,
    audio_stream_count: usize,
    need_mix_audio: bool,
) -> (u32, u32, bool) {
    let target_bits = (max_mb.max(2) as f64) * 8_000_000.0 * 0.90;
    let total_kbps = (target_bits / dur_sec.max(0.5)) / 1000.0;

    if audio_stream_count == 0 {
        let video_kbps = total_kbps.clamp(180.0, 24_000.0).round() as u32;
        return (video_kbps, 0, false);
    }

    let streams = audio_stream_count as f64;
    let per_stream_copy_cost = 192.0 * streams;

    // Preserve un-re-encoded source AAC tracks whenever copy cost stays <= 20% of total budget
    if !need_mix_audio && total_kbps >= per_stream_copy_cost * 5.0 {
        let video_kbps = (total_kbps - per_stream_copy_cost)
            .clamp(180.0, 24_000.0)
            .round() as u32;
        return (video_kbps, 192, true);
    }

    let audio_kbps: u32 = if total_kbps >= 2_400.0 * streams {
        192
    } else if total_kbps >= 1_200.0 * streams {
        160
    } else if total_kbps >= 650.0 * streams {
        128
    } else {
        96
    };

    let video_kbps = (total_kbps - (audio_kbps as f64) * streams)
        .clamp(180.0, 24_000.0)
        .round() as u32;
    (video_kbps, audio_kbps, false)
}

/// Builds FFmpeg CLI arguments for Milestone 2.0.3 video trimming:
/// - Lossless `-c copy` by default
/// - Lossless video + mixed custom audio tracks when `mix_selected_audio == true`
/// - Bitrate-capped GPU/CPU re-encode when `target_max_mb` is set (Discord 10/25/50 MB presets)
pub fn build_trim_ffmpeg_args(
    src_path: &str,
    out_path: &str,
    start_ms: u64,
    end_ms: u64,
    selected_audio_indices: &[usize],
    mix_selected_audio: bool,
    target_max_mb: Option<u32>,
    encoder: &str,
) -> Vec<String> {
    let start_sec = (start_ms as f64) / 1000.0;
    let dur_sec = ((end_ms.saturating_sub(start_ms)) as f64) / 1000.0;

    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-ss".to_string(),
        format!("{:.3}", start_sec),
        "-i".to_string(),
        src_path.to_string(),
        "-t".to_string(),
        format!("{:.3}", dur_sec),
    ];

    // Respect `mix_selected_audio` strictly even on size-capped presets so selecting 2+ tracks
    // keeps N discrete audio streams unless the user explicitly chooses to mix into 1 stream.
    let need_mix_audio = selected_audio_indices.len() > 1 && mix_selected_audio;

    if need_mix_audio {
        let inputs: String = selected_audio_indices
            .iter()
            .map(|idx| format!("[0:a:{}]", idx))
            .collect();
        let cnt = selected_audio_indices.len();
        args.extend([
            "-filter_complex".to_string(),
            format!(
                "{}amix=inputs={}:duration=longest:dropout_transition=0,volume={},alimiter=limit=0.95:attack=5:release=50[aout]",
                inputs, cnt, cnt
            ),
            "-map".to_string(),
            "0:v:0".to_string(),
            "-map".to_string(),
            "[aout]".to_string(),
        ]);
    } else {
        args.extend(["-map".to_string(), "0:v:0".to_string()]);
        for idx in selected_audio_indices {
            args.extend(["-map".to_string(), format!("0:a:{}?", idx)]);
        }
    }

    if let Some(max_mb) = target_max_mb {
        let output_audio_streams = if selected_audio_indices.is_empty() {
            0
        } else if need_mix_audio {
            1
        } else {
            selected_audio_indices.len()
        };
        let (video_kbps, audio_kbps, copy_unmixed_audio) = compute_trim_bitrates_kbps(
            max_mb,
            dur_sec,
            output_audio_streams,
            need_mix_audio,
        );
        let maxrate_kbps = ((video_kbps as f64) * 1.15).round() as u32;
        let bufsize_kbps = video_kbps * 2;

        args.extend([
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-c:v".to_string(),
            encoder.to_string(),
            "-b:v".to_string(),
            format!("{}k", video_kbps),
            "-maxrate".to_string(),
            format!("{}k", maxrate_kbps),
            "-bufsize".to_string(),
            format!("{}k", bufsize_kbps),
        ]);
        if !selected_audio_indices.is_empty() {
            if copy_unmixed_audio {
                args.extend(["-c:a".to_string(), "copy".to_string()]);
            } else {
                args.extend([
                    "-c:a".to_string(),
                    "aac".to_string(),
                    "-ar".to_string(),
                    "48000".to_string(),
                    "-b:a".to_string(),
                    format!("{}k", audio_kbps),
                ]);
            }
        }
    } else {
        args.extend(["-c:v".to_string(), "copy".to_string()]);
        if !selected_audio_indices.is_empty() {
            if need_mix_audio {
                args.extend([
                    "-c:a".to_string(),
                    "aac".to_string(),
                    "-ar".to_string(),
                    "48000".to_string(),
                    "-b:a".to_string(),
                    "192k".to_string(),
                ]);
            } else {
                args.extend(["-c:a".to_string(), "copy".to_string()]);
            }
        }
    }

    args.extend(["-movflags".to_string(), "+faststart".to_string(), out_path.to_string()]);
    args
}

/// Probes embedded audio tracks & duration of a saved Gallery video capture (`v2.0.3`).
pub fn probe_capture_video_info(app: &AppHandle, capture_id: &str) -> AppResult<VideoProbeInfo> {
    let state: tauri::State<'_, AppState> = app.state();
    let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
    let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
    let bin_str = ff.binary_path.ok_or_else(|| {
        AppError::General("FFmpeg is required to inspect video audio tracks.".to_string())
    })?;

    let rec = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_capture_by_id(&conn, capture_id)?
    }
    .ok_or_else(|| AppError::General("Video capture not found.".to_string()))?;

    let path = PathBuf::from(&rec.file_path);
    if !path.exists() {
        return Err(AppError::General("Video file is missing from disk.".to_string()));
    }

    let mut info = probe_video_file_info(&bin_str, &path);
    if info.duration_ms == 0 {
        info.duration_ms = rec.duration_ms.unwrap_or(0);
    }
    Ok(info)
}

/// Milestone 2.0.3: Trims a video clip with optional audio track selection/mixdown,
/// optional Discord target file size compression, and optional original file replacement.
pub fn trim_video_clip(
    app: &AppHandle,
    id: String,
    start_ms: u64,
    end_ms: u64,
    selected_audio_indices: Option<Vec<usize>>,
    mix_selected_audio: bool,
    target_max_mb: Option<u32>,
    replace_original: bool,
) -> AppResult<CaptureRecord> {
    if end_ms <= start_ms.saturating_add(200) {
        return Err(AppError::General(
            "Selected trim range must be at least 0.2 seconds.".to_string(),
        ));
    }

    let state: tauri::State<'_, AppState> = app.state();
    let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
    let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
    let bin_str = ff.binary_path.ok_or_else(|| {
        AppError::General("FFmpeg is not available. Download FFmpeg Engine in Settings first.".to_string())
    })?;

    let src_rec = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_capture_by_id(&conn, &id)?
    }
    .ok_or_else(|| AppError::General("Source video capture not found.".to_string()))?;

    let src_path = PathBuf::from(&src_rec.file_path);
    if !src_path.exists() {
        return Err(AppError::General("Source video file no longer exists on disk.".to_string()));
    }

    let audio_indices = match selected_audio_indices {
        Some(list) => list,
        None => {
            let probe = probe_video_file_info(&bin_str, &src_path);
            if probe.audio_tracks.is_empty() {
                Vec::new()
            } else {
                vec![0]
            }
        }
    };

    let captures_dir = state
        .captures_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock captures_dir".to_string()))?
        .clone();
    let thumbs_dir = state
        .thumbs_dir
        .read()
        .map_err(|_| AppError::General("Failed to lock thumbs_dir".to_string()))?
        .clone();
    fs::create_dir_all(&captures_dir)?;
    fs::create_dir_all(&thumbs_dir)?;

    let new_id = Uuid::new_v4().to_string();
    let stamp = Utc::now().format("%Y%m%d_%H%M%S");
    let out_path = captures_dir.join(format!("Trim_{}_{}.mp4", stamp, &new_id[..6]));
    let thumb_path = thumbs_dir.join(format!("{}_thumb.webp", new_id));
    let temp_png_thumb = state.replay_ring_dir.join(format!("thumb_{}.png", new_id));

    let primary_encoder = if target_max_mb.is_some() {
        ff.recommended_encoder.as_str()
    } else {
        "copy"
    };

    let trim_args = build_trim_ffmpeg_args(
        &src_path.to_string_lossy(),
        &out_path.to_string_lossy(),
        start_ms,
        end_ms,
        &audio_indices,
        mix_selected_audio,
        target_max_mb,
        primary_encoder,
    );

    let mut out = new_hidden_command(&bin_str).args(&trim_args).output()?;
    // If GPU target-size re-encode failed (e.g., strict driver constraints), retry cleanly with software fallback
    if (!out.status.success() || !out_path.exists()) && target_max_mb.is_some() && primary_encoder != "libx264" {
        let fallback_enc = select_best_encoder("libx264", &ff.available_encoders, None);
        let fallback_args = build_trim_ffmpeg_args(
            &src_path.to_string_lossy(),
            &out_path.to_string_lossy(),
            start_ms,
            end_ms,
            &audio_indices,
            mix_selected_audio,
            target_max_mb,
            &fallback_enc,
        );
        out = new_hidden_command(&bin_str).args(&fallback_args).output()?;
    }

    if !out.status.success() || !out_path.exists() {
        let _ = fs::remove_file(&out_path);
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::General(format!(
            "Failed to trim video clip: {}",
            stderr.trim()
        )));
    }

    let (width, height, probed_duration_ms) =
        generate_video_thumbnail_and_probe(&bin_str, &temp_png_thumb, &out_path, &thumb_path);
    let size_bytes = fs::metadata(&out_path)?.len();
    let duration_ms = probed_duration_ms.unwrap_or_else(|| end_ms.saturating_sub(start_ms));

    let mut new_tags = src_rec.tags.clone();
    if !new_tags.iter().any(|t| t == "trimmed") {
        new_tags.push("trimmed".to_string());
    }

    let trimmed_record = CaptureRecord {
        id: new_id,
        media_type: "video".to_string(),
        file_path: out_path.to_string_lossy().to_string(),
        thumb_path: thumb_path.to_string_lossy().to_string(),
        width,
        height,
        size_bytes,
        duration_ms: Some(duration_ms),
        source_app: Some(format!("Trimmed Highlight ({}s)", duration_ms / 1000)),
        ocr_text: String::new(),
        ocr_status: "done".to_string(),
        is_pinned: if replace_original { src_rec.is_pinned } else { false },
        group_name: src_rec.group_name.clone(),
        last_copied_at: None,
        queued_delete_at: None,
        tags: new_tags,
        created_at: Utc::now().to_rfc3339(),
    };

    {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        insert_capture(&conn, &trimmed_record)?;
        if replace_original {
            let _ = delete_captures(&conn, &[id.clone()]);
            cleanup_video_track_previews(Some(&id));
        }
    }

    let _ = crate::services::storage_cleanup::enforce_storage_policies_now(app);
    let _ = app.emit("gallery:updated", &trimmed_record);
    let _ = app.emit(
        "shortcut:toast",
        format!(
            "Saved {}s trimmed clip ({:.1} MB){}",
            duration_ms / 1000,
            size_bytes as f64 / (1024.0 * 1024.0),
            if replace_original { " — original deleted" } else { "" }
        ),
    );

    Ok(trimmed_record)
}

fn preview_root_dir() -> PathBuf {
    std::env::temp_dir().join("replayviewer_track_preview")
}

/// Computes `bucket_count` normalized amplitude peaks (`0.0..=1.0`) from a mono `s16le` PCM buffer.
pub fn compute_pcm_waveform_peaks(pcm_bytes: &[u8], bucket_count: usize) -> Vec<f32> {
    let buckets = bucket_count.clamp(32, 320);
    let sample_count = pcm_bytes.len() / 2;
    if sample_count == 0 {
        return vec![0.0; buckets];
    }

    let mut out = Vec::with_capacity(buckets);
    for b in 0..buckets {
        let start = (b * sample_count) / buckets;
        let end = (((b + 1) * sample_count) / buckets).max(start + 1).min(sample_count);
        let mut max_abs: f32 = 0.0;
        for s in start..end {
            let lo = pcm_bytes[s * 2];
            let hi = pcm_bytes[s * 2 + 1];
            let val = i16::from_le_bytes([lo, hi]);
            let norm = (val as i32).abs() as f32 / 32768.0;
            if norm > max_abs {
                max_abs = norm;
            }
        }
        let curved = if max_abs > 0.0 {
            max_abs.clamp(0.0, 1.0).powf(0.65)
        } else {
            0.0
        };
        out.push((curved * 1000.0).round() / 1000.0);
    }
    out
}

/// Returns the raw `s16le` PCM payload inside a standard RIFF WAVE byte slice (skipping RIFF/LIST headers safely).
pub fn extract_wav_pcm_slice(wav_bytes: &[u8]) -> &[u8] {
    if wav_bytes.len() >= 12 && &wav_bytes[0..4] == b"RIFF" && &wav_bytes[8..12] == b"WAVE" {
        let mut pos = 12usize;
        while pos + 8 <= wav_bytes.len() {
            let chunk_id = &wav_bytes[pos..pos + 4];
            let chunk_len = u32::from_le_bytes([
                wav_bytes[pos + 4],
                wav_bytes[pos + 5],
                wav_bytes[pos + 6],
                wav_bytes[pos + 7],
            ]) as usize;
            let data_start = pos + 8;
            if chunk_id == b"data" {
                let data_end = data_start.saturating_add(chunk_len).min(wav_bytes.len());
                return &wav_bytes[data_start..data_end];
            }
            pos = data_start.saturating_add(chunk_len + (chunk_len % 2));
        }
    }
    wav_bytes.get(44..).unwrap_or(&[])
}

/// Builds single-pass FFmpeg arguments that decode and timestamp-align each audio stream to `.wav`
/// (`aresample=async=1:first_pts=0`) for 100% reliable WebView2 Solo/Mute playback and waveform peaks.
pub fn build_preview_demux_ffmpeg_args(
    src_path: &str,
    targets: &[(usize, PathBuf)],
    sample_rate_hz: u32,
) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-i".to_string(),
        src_path.to_string(),
    ];

    let sr_str = sample_rate_hz.clamp(8000, 48000).to_string();
    for (stream_idx, wav_path) in targets {
        args.extend([
            "-map".to_string(),
            format!("0:a:{}?", stream_idx),
            "-vn".to_string(),
            "-af".to_string(),
            "aresample=async=1:first_pts=0".to_string(),
            "-c:a".to_string(),
            "pcm_s16le".to_string(),
            "-ac".to_string(),
            "2".to_string(),
            "-ar".to_string(),
            sr_str.clone(),
            "-f".to_string(),
            "wav".to_string(),
            wav_path.to_string_lossy().to_string(),
        ]);
    }
    args
}

fn cleanup_stale_preview_dirs(keep_id: Option<&str>) {
    let root = preview_root_dir();
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(keep) = keep_id {
            if path.file_name().and_then(|n| n.to_str()) == Some(keep) {
                continue;
            }
        }
        let is_old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|age| age > Duration::from_secs(7200))
            .unwrap_or(false);
        if is_old {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

/// Removes temporary demuxed track preview files for one capture (or all previews when `id` is `None`).
pub fn cleanup_video_track_previews(id: Option<&str>) {
    let root = preview_root_dir();
    if let Some(capture_id) = id {
        let dir = root.join(capture_id);
        if dir.exists() {
            let _ = fs::remove_dir_all(dir);
        }
    } else if root.exists() {
        let _ = fs::remove_dir_all(root);
    }
}

/// Extracts per-track `.wav` streams and downsampled waveform peaks in a single FFmpeg pass
/// for lazy Lightbox Solo/Mute auditioning (`v2.0.3`).
pub fn prepare_video_track_previews(
    app: &AppHandle,
    capture_id: &str,
    bucket_count: usize,
    force_refresh: bool,
) -> AppResult<Vec<VideoAudioTrackPreview>> {
    cleanup_stale_preview_dirs(Some(capture_id));

    let state: tauri::State<'_, AppState> = app.state();
    let cfg = crate::commands::settings_cmds::get_settings(state.clone())?;
    let ff = get_cached_or_probe_ffmpeg(&state, &cfg.ffmpeg_custom_path, false);
    let bin_str = ff.binary_path.ok_or_else(|| {
        AppError::General("FFmpeg is required to generate audio track waveforms.".to_string())
    })?;

    let rec = {
        let conn = state
            .db
            .lock()
            .map_err(|_| AppError::General("Failed to lock database".to_string()))?;
        get_capture_by_id(&conn, capture_id)?
    }
    .ok_or_else(|| AppError::General("Video capture not found.".to_string()))?;

    let src_path = PathBuf::from(&rec.file_path);
    if !src_path.exists() {
        return Err(AppError::General("Video file is missing from disk.".to_string()));
    }

    let probe = probe_video_file_info(&bin_str, &src_path);
    if probe.audio_tracks.is_empty() {
        return Ok(vec![]);
    }

    let preview_dir = preview_root_dir().join(capture_id);
    if force_refresh && preview_dir.exists() {
        let _ = fs::remove_dir_all(&preview_dir);
    }
    fs::create_dir_all(&preview_dir)?;
    let peaks_cache_path = preview_dir.join("peaks_cache.json");

    if !force_refresh && peaks_cache_path.exists() {
        if let Ok(json_str) = fs::read_to_string(&peaks_cache_path) {
            if let Ok(cached) = serde_json::from_str::<Vec<VideoAudioTrackPreview>>(&json_str) {
                let all_present = cached.len() == probe.audio_tracks.len()
                    && cached
                        .iter()
                        .all(|p| p.audio_path.ends_with(".wav") && Path::new(&p.audio_path).exists());
                if all_present {
                    return Ok(cached);
                }
            }
        }
    }

    let sample_rate_hz = if probe.duration_ms > 600_000 {
        12_000
    } else if probe.duration_ms > 180_000 {
        16_000
    } else {
        24_000
    };

    let src_str = src_path.to_string_lossy().to_string();
    let targets: Vec<(usize, PathBuf)> = probe
        .audio_tracks
        .iter()
        .map(|t| {
            (
                t.stream_index,
                preview_dir.join(format!("track_{}.wav", t.stream_index)),
            )
        })
        .collect();

    let args = build_preview_demux_ffmpeg_args(&src_str, &targets, sample_rate_hz);
    let output = new_hidden_command(&bin_str)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::General(format!(
            "FFmpeg track preview extraction failed: {}",
            stderr.trim()
        )));
    }

    let buckets = bucket_count.clamp(32, 320);
    let mut previews = Vec::with_capacity(probe.audio_tracks.len());
    for (track, (_, wav_path)) in probe.audio_tracks.iter().zip(targets.iter()) {
        let peaks = match fs::read(wav_path) {
            Ok(bytes) => compute_pcm_waveform_peaks(extract_wav_pcm_slice(&bytes), buckets),
            Err(_) => vec![0.0; buckets],
        };
        previews.push(VideoAudioTrackPreview {
            stream_index: track.stream_index,
            title: track.title.clone(),
            audio_path: wav_path.to_string_lossy().to_string(),
            peaks,
        });
    }

    if let Ok(json_str) = serde_json::to_string(&previews) {
        let _ = fs::write(&peaks_cache_path, json_str);
    }

    Ok(previews)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::audio::AudioSourceKind;

    #[test]
    fn test_audio_mapping_puts_master_mix_at_stream_zero() {
        let tracks = vec![
            AudioPipeTrack {
                slot_index: 0,
                title: "Desktop Audio".to_string(),
                pipe_path: r"\\.\pipe\test0".to_string(),
                kind: AudioSourceKind::DesktopLoopback { exclude_pid: None },
                volume_gain: 1.0,
            },
            AudioPipeTrack {
                slot_index: 1,
                title: "Isolated App 1".to_string(),
                pipe_path: r"\\.\pipe\test1".to_string(),
                kind: AudioSourceKind::ProcessSlot { slot_idx: 0 },
                volume_gain: 1.0,
            },
            AudioPipeTrack {
                slot_index: 2,
                title: "Microphone".to_string(),
                pipe_path: r"\\.\pipe\test2".to_string(),
                kind: AudioSourceKind::Microphone {
                    device_id: String::new(),
                },
                volume_gain: 1.0,
            },
        ];

        let multi = build_audio_mapping_args(&tracks, false);
        assert!(multi.contains(&"title=Master Mix".to_string()));
        assert!(multi.contains(&"-metadata:s:a:0".to_string()));
        assert!(multi.contains(&"title=Desktop Audio".to_string()));
        assert!(multi.contains(&"title=Isolated App 1".to_string()));
        assert!(multi.contains(&"title=Microphone".to_string()));
        // Master Mix must mix ONLY Desktop (1:a) + Mic (3:a) and never double-mix Isolated App (2:a)
        assert!(multi.iter().any(|a| a.contains("[mix1][mix3]amix=inputs=2") && !a.contains("[mix2]")));

        let single = build_audio_mapping_args(&tracks, true);
        assert!(single.contains(&"title=Master Mix".to_string()));
        assert!(!single.contains(&"title=Microphone".to_string()));
    }

    #[test]
    fn test_trim_ffmpeg_args_lossless_mix_and_target_mb() {
        // 1. Lossless multi-track copy (keep streams 1 and 3 cleanly)
        let lossless = build_trim_ffmpeg_args(
            "in.mp4",
            "out.mp4",
            12_500,
            27_500,
            &[1, 3],
            false,
            None,
            "copy",
        );
        assert!(lossless.windows(2).any(|w| w == ["-ss", "12.500"]));
        assert!(lossless.windows(2).any(|w| w == ["-t", "15.000"]));
        assert!(lossless.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(lossless.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(lossless.contains(&"0:a:1?".to_string()));
        assert!(lossless.contains(&"0:a:3?".to_string()));

        // 2. Lossless video + custom audio mixdown into 1 Discord-ready AAC stream
        let custom_mix = build_trim_ffmpeg_args(
            "in.mp4",
            "out.mp4",
            0,
            10_000,
            &[1, 2],
            true,
            None,
            "copy",
        );
        assert!(custom_mix.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(custom_mix.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert!(custom_mix.iter().any(|a| a.contains("[0:a:1][0:a:2]amix=inputs=2")));

        // 3. Single-track Discord 10 MB re-encode (20s clip -> plenty of budget -> preserves -c:a copy)
        let discord_10_single = build_trim_ffmpeg_args(
            "in.mp4",
            "out.mp4",
            5_000,
            25_000,
            &[0],
            false,
            Some(10),
            "h264_nvenc",
        );
        assert!(discord_10_single.windows(2).any(|w| w == ["-c:v", "h264_nvenc"]));
        assert!(discord_10_single.windows(2).any(|w| w == ["-c:a", "copy"]));
        assert!(discord_10_single.contains(&"-b:v".to_string()));

        // 4. Multi-track Discord 10 MB mixdown (20s clip -> full 192k AAC + alimiter)
        let discord_10_mixed = build_trim_ffmpeg_args(
            "in.mp4",
            "out.mp4",
            0,
            20_000,
            &[1, 3],
            true,
            Some(10),
            "h264_nvenc",
        );
        assert!(discord_10_mixed.windows(2).any(|w| w == ["-c:a", "aac"]));
        assert!(discord_10_mixed.windows(2).any(|w| w == ["-b:a", "192k"]));
        assert!(discord_10_mixed.iter().any(|a| a.contains("alimiter=limit=0.95")));

        // 5. Multi-track Discord 10 MB WITHOUT mixdown keeps BOTH discrete audio streams!
        let discord_10_unmixed = build_trim_ffmpeg_args(
            "in.mp4",
            "out.mp4",
            0,
            20_000,
            &[1, 3],
            false,
            Some(10),
            "h264_nvenc",
        );
        assert!(discord_10_unmixed.contains(&"0:a:1?".to_string()));
        assert!(discord_10_unmixed.contains(&"0:a:3?".to_string()));
        assert!(!discord_10_unmixed.iter().any(|a| a.contains("amix")));
        assert!(discord_10_unmixed.windows(2).any(|w| w == ["-c:a", "copy"]));

        // 6. Long 90s clip into 10 MB scales gently (128k, never robotic 64k)
        let (_v_long, a_long, copy_long) = compute_trim_bitrates_kbps(10, 90.0, 1, false);
        assert!(!copy_long);
        assert_eq!(a_long, 128);

        // 7. 50 MB clip at 120s preserves high bitrate (192k)
        let (_v_50, a_50, copy_50) = compute_trim_bitrates_kbps(50, 120.0, 1, true);
        assert!(!copy_50);
        assert_eq!(a_50, 192);
    }

    #[test]
    fn test_resolve_ffprobe_and_parse_stderr_four_tracks() {
        let managed_ffmpeg = r"C:\Users\user\AppData\Local\com.replayviewer.app\bin\ffmpeg-master-latest-win64-lgpl\bin\ffmpeg.exe";
        let resolved = resolve_ffprobe_binary(managed_ffmpeg);
        let resolved_str = resolved.to_string_lossy();
        assert!(resolved_str.contains("ffmpeg-master-latest-win64-lgpl"));
        assert!(resolved_str.ends_with("ffprobe.exe"));

        let sample_stderr = r#"
  Duration: 00:00:30.50, start: 0.000000, bitrate: 15000 kb/s
    Stream #0:0(und): Video: h264 (Main), yuv420p, 1920x1080 [SAR 1:1 DAR 16:9], 60 fps
    Stream #0:1(und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 192 kb/s
      Metadata:
        title           : Master Mix
    Stream #0:2(und): Audio: aac (LC), 48000 Hz, stereo, fltp, 192 kb/s
      Metadata:
        title           : Desktop Audio
    Stream #0:3(und): Audio: aac (LC), 48000 Hz, stereo, fltp, 192 kb/s
      Metadata:
        title           : msedge.exe
    Stream #0:4(und): Audio: aac (LC), 48000 Hz, stereo, fltp, 192 kb/s
      Metadata:
        title           : Microphone
"#;
        let (dur, w, h, tracks) = parse_ffmpeg_stderr_probe(sample_stderr);
        assert_eq!(dur, 30_500);
        assert_eq!((w, h), (1920, 1080));
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[0].title, "Master Mix");
        assert_eq!(tracks[2].title, "msedge.exe");
        assert_eq!(tracks[3].title, "Microphone");
    }

    #[test]
    fn test_preview_demux_ffmpeg_args_and_pcm_peaks() {
        let targets = vec![
            (0usize, PathBuf::from("t0.wav")),
            (1usize, PathBuf::from("t1.wav")),
        ];
        let wav_args = build_preview_demux_ffmpeg_args("clip.mp4", &targets, 24_000);
        assert!(wav_args.contains(&"0:a:0?".to_string()));
        assert!(wav_args.contains(&"0:a:1?".to_string()));
        assert!(wav_args.windows(2).any(|w| w == ["-c:a", "pcm_s16le"]));
        assert!(wav_args.windows(2).any(|w| w == ["-f", "wav"]));
        assert!(wav_args.iter().any(|a| a.contains("aresample=async=1:first_pts=0")));

        // Build minimal RIFF WAVE bytes with a 44-byte header + 4 s16le samples
        let samples: [i16; 4] = [0, 16384, 0, -32768];
        let mut wav = Vec::with_capacity(44 + 8);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + 8).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&[1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 16, 0]);
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8u32.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let peaks = compute_pcm_waveform_peaks(extract_wav_pcm_slice(&wav), 32);
        assert_eq!(peaks.len(), 32);
        assert!(peaks.iter().any(|&p| (p - 1.0).abs() < 0.01));
        assert!(peaks.iter().any(|&p| p > 0.5 && p < 0.9));
        assert!(peaks[0] == 0.0);
    }
}
