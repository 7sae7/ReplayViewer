use std::path::{Path, PathBuf};
use std::process::Command;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegStatus {
    pub available: bool,
    pub binary_path: Option<String>,
    pub source: String, // "custom" | "managed" | "system" | "missing"
    pub version: Option<String>,
    pub supports_ddagrab: bool,
    pub available_encoders: Vec<String>,
    pub recommended_encoder: String,
    #[serde(default)]
    pub downloading: bool,
    #[serde(default)]
    pub download_stage: Option<String>,
}

fn new_hidden_command(bin: &str) -> Command {
    let mut cmd = Command::new(bin);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// Checks if a given `ffmpeg` binary executes cleanly and returns its first version line.
fn probe_ffmpeg_version(bin: &str) -> Option<String> {
    let out = new_hidden_command(bin).arg("-version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string())
}

/// Recursively locates `ffmpeg.exe` (Windows) or `ffmpeg` inside `dir`.
fn find_ffmpeg_in_dir(dir: &Path) -> Option<PathBuf> {
    let exe_name = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };

    let direct = dir.join(exe_name);
    if direct.is_file() {
        return Some(direct);
    }

    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_ffmpeg_in_dir(&p) {
                return Some(found);
            }
        }
    }
    None
}

/// Resolves the active FFmpeg binary path using MIT-safe precedence:
/// 1. User-supplied custom binary path (e.g., Full GPL build with `libx264`)
/// 2. Managed minimal LGPL build inside `%LOCALAPPDATA%/com.replayviewer.app/bin/`
/// 3. Pre-installed `ffmpeg` on system `PATH`
pub fn resolve_ffmpeg_binary(
    custom_path: &str,
    managed_bin_dir: &Path,
) -> (Option<PathBuf>, String) {
    let trimmed = custom_path.trim();
    if !trimmed.is_empty() {
        let custom = PathBuf::from(trimmed);
        if custom.exists() && probe_ffmpeg_version(trimmed).is_some() {
            return (Some(custom), "custom".to_string());
        }
    }

    if let Some(managed) = find_ffmpeg_in_dir(managed_bin_dir) {
        if let Some(s) = managed.to_str() {
            if probe_ffmpeg_version(s).is_some() {
                return (Some(managed), "managed".to_string());
            }
        }
    }

    let path_bin = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if probe_ffmpeg_version(path_bin).is_some() {
        return (Some(PathBuf::from(path_bin)), "system".to_string());
    }

    (None, "missing".to_string())
}

/// Probes available hardware H.264 encoders (NVENC / AMF / QuickSync / VideoToolbox / MediaFoundation)
/// and checks whether the resolved FFmpeg build supports `ddagrab` Desktop Duplication on Windows.
pub fn probe_ffmpeg_status(custom_path: &str, managed_bin_dir: &Path) -> FfmpegStatus {
    let (resolved, source) = resolve_ffmpeg_binary(custom_path, managed_bin_dir);
    let Some(bin_path) = resolved else {
        return FfmpegStatus {
            available: false,
            binary_path: None,
            source: "missing".to_string(),
            version: None,
            supports_ddagrab: false,
            available_encoders: Vec::new(),
            recommended_encoder: "auto".to_string(),
            downloading: false,
            download_stage: None,
        };
    };

    let bin_str = bin_path.to_string_lossy().to_string();
    let version = probe_ffmpeg_version(&bin_str);

    let mut available_encoders = Vec::new();
    if let Ok(out) = new_hidden_command(&bin_str)
        .args(["-hide_banner", "-encoders"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let priority_candidates = [
            "h264_nvenc",
            "hevc_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_videotoolbox",
            "h264_vaapi",
            "h264_mf",
            "libx264",
            "libopenh264",
        ];
        for cand in priority_candidates {
            if stdout.contains(cand) {
                available_encoders.push(cand.to_string());
            }
        }
    }

    let mut supports_ddagrab = false;
    if cfg!(target_os = "windows") {
        if let Ok(out) = new_hidden_command(&bin_str)
            .args(["-hide_banner", "-filters"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            supports_ddagrab = stdout.contains("ddagrab");
        }
    }

    let recommended_encoder = select_best_encoder("auto", &available_encoders, Some(&bin_str));

    FfmpegStatus {
        available: true,
        binary_path: Some(bin_str),
        source,
        version,
        supports_ddagrab,
        available_encoders,
        recommended_encoder,
        downloading: false,
        download_stage: None,
    }
}

/// Runs a 1-frame synthetic smoke test on a hardware encoder candidate so we never pick `h264_nvenc`
/// on an AMD/Intel-only PC just because FFmpeg compiled NVENC support into its binary!
pub fn verify_encoder_works_on_gpu(bin_str: &str, encoder: &str) -> bool {
    if encoder == "libx264" || encoder == "libopenh264" || encoder == "h264_mf" {
        return true;
    }
    let out = new_hidden_command(bin_str)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=256x256:r=30:d=0.1",
            "-c:v",
            encoder,
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ])
        .output();

    matches!(out, Ok(o) if o.status.success())
}

/// Resolves `"auto"` or an explicit encoder preference into a GPU-verified encoder name.
pub fn select_best_encoder(
    preferred: &str,
    available_encoders: &[String],
    bin_str: Option<&str>,
) -> String {
    let pref = preferred.trim();
    if !pref.is_empty() && pref != "auto" && available_encoders.iter().any(|e| e == pref) {
        return pref.to_string();
    }

    let order = if cfg!(target_os = "macos") {
        vec!["h264_videotoolbox", "libx264", "libopenh264"]
    } else if cfg!(target_os = "linux") {
        vec!["h264_nvenc", "h264_vaapi", "h264_qsv", "libx264", "libopenh264"]
    } else {
        vec![
            "h264_nvenc",
            "h264_amf",
            "h264_qsv",
            "h264_mf",
            "libx264",
            "libopenh264",
        ]
    };

    for cand in order {
        if available_encoders.iter().any(|e| e == cand) {
            if let Some(bin) = bin_str {
                if verify_encoder_works_on_gpu(bin, cand) {
                    return cand.to_string();
                }
            } else {
                return cand.to_string();
            }
        }
    }

    available_encoders
        .first()
        .cloned()
        .unwrap_or_else(|| "h264_mf".to_string())
}

/// Returns the cached `FfmpegStatus` in < 1 µs, or probes once on startup / after download / settings change.
pub fn get_cached_or_probe_ffmpeg(
    state: &crate::state::AppState,
    custom_path: &str,
    force_refresh: bool,
) -> FfmpegStatus {
    use std::sync::atomic::Ordering;

    let mut status = if !force_refresh {
        if let Ok(guard) = state.cached_ffmpeg_status.read() {
            if let Some(ref cached) = *guard {
                cached.clone()
            } else {
                drop(guard);
                let probed = probe_ffmpeg_status(custom_path, &state.replay_bin_dir);
                if let Ok(mut w) = state.cached_ffmpeg_status.write() {
                    *w = Some(probed.clone());
                }
                probed
            }
        } else {
            probe_ffmpeg_status(custom_path, &state.replay_bin_dir)
        }
    } else {
        let probed = probe_ffmpeg_status(custom_path, &state.replay_bin_dir);
        if let Ok(mut guard) = state.cached_ffmpeg_status.write() {
            *guard = Some(probed.clone());
        }
        probed
    };

    let is_dl = state.ffmpeg_downloading.load(Ordering::Relaxed);
    status.downloading = is_dl;
    status.download_stage = if is_dl {
        state
            .ffmpeg_download_stage
            .read()
            .ok()
            .map(|s| s.clone())
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    status
}

fn emit_download_stage(app: &AppHandle, state: &crate::state::AppState, stage: &str, message: &str) {
    if let Ok(mut guard) = state.ffmpeg_download_stage.write() {
        *guard = message.to_string();
    }
    let _ = app.emit(
        "replay:ffmpeg_download_progress",
        serde_json::json!({ "stage": stage, "message": message }),
    );
}

/// Downloads an official minimal LGPL FFmpeg build on demand into `managed_bin_dir`
/// (`%LOCALAPPDATA%/com.replayviewer.app/bin/`) using native OS tools so the base installer stays < 6 MB
/// and 100% MIT-licensed.
pub fn download_minimal_ffmpeg_to_dir(
    app: &AppHandle,
    state: &crate::state::AppState,
    managed_bin_dir: &Path,
) -> AppResult<FfmpegStatus> {
    std::fs::create_dir_all(managed_bin_dir)?;
    emit_download_stage(
        app,
        state,
        "downloading",
        "Downloading minimal LGPL FFmpeg build (~45 MB)...",
    );

    #[cfg(target_os = "windows")]
    {
        // BtbN minimal LGPL shared/static Windows x64 build (supports ddagrab + NVENC + AMF + QSV + MediaFoundation)
        let url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-lgpl.zip";
        let zip_path = managed_bin_dir.join("ffmpeg_lgpl.zip");

        let dl_out = new_hidden_command("curl.exe")
            .args([
                "-L",
                "--fail",
                "--retry",
                "2",
                "-o",
                &zip_path.to_string_lossy(),
                url,
            ])
            .output()?;

        if !dl_out.status.success() || !zip_path.exists() {
            let err_txt = String::from_utf8_lossy(&dl_out.stderr);
            return Err(AppError::General(format!(
                "Failed to download FFmpeg archive: {}",
                err_txt.trim()
            )));
        }

        emit_download_stage(
            app,
            state,
            "extracting",
            "Extracting ffmpeg.exe into AppData/Local/com.replayviewer.app/bin...",
        );

        let ex_out = new_hidden_command("tar.exe")
            .args([
                "-xf",
                &zip_path.to_string_lossy(),
                "-C",
                &managed_bin_dir.to_string_lossy(),
            ])
            .output()?;

        let _ = std::fs::remove_file(&zip_path);
        if !ex_out.status.success() {
            let err_txt = String::from_utf8_lossy(&ex_out.stderr);
            return Err(AppError::General(format!(
                "Failed to extract FFmpeg archive: {}",
                err_txt.trim()
            )));
        }
    }

    #[cfg(target_os = "macos")]
    {
        let zip_path = managed_bin_dir.join("ffmpeg_macos.zip");
        let url = "https://evermeet.cx/ffmpeg/getrelease/zip";
        let dl_out = new_hidden_command("curl")
            .args(["-L", "--fail", "-o", &zip_path.to_string_lossy(), url])
            .output()?;
        if !dl_out.status.success() {
            return Err(AppError::General(
                "Failed to download macOS FFmpeg build".to_string(),
            ));
        }
        let _ = new_hidden_command("unzip")
            .args([
                "-o",
                &zip_path.to_string_lossy(),
                "-d",
                &managed_bin_dir.to_string_lossy(),
            ])
            .output()?;
        let _ = std::fs::remove_file(&zip_path);
    }

    #[cfg(target_os = "linux")]
    {
        let tar_path = managed_bin_dir.join("ffmpeg_linux.tar.xz");
        let url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-lgpl.tar.xz";
        let dl_out = new_hidden_command("curl")
            .args(["-L", "--fail", "-o", &tar_path.to_string_lossy(), url])
            .output()?;
        if !dl_out.status.success() {
            return Err(AppError::General(
                "Failed to download Linux FFmpeg build".to_string(),
            ));
        }
        let _ = new_hidden_command("tar")
            .args([
                "-xf",
                &tar_path.to_string_lossy(),
                "-C",
                &managed_bin_dir.to_string_lossy(),
            ])
            .output()?;
        let _ = std::fs::remove_file(&tar_path);
    }

    let status = probe_ffmpeg_status("", managed_bin_dir);
    emit_download_stage(
        app,
        state,
        if status.available { "ready" } else { "error" },
        if status.available {
            "FFmpeg Replay Engine ready!"
        } else {
            "FFmpeg extracted but failed verification."
        },
    );

    if !status.available {
        return Err(AppError::General(
            "FFmpeg binary was downloaded but could not be verified.".to_string(),
        ));
    }

    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_selection_priority_and_override() {
        let avail = vec![
            "h264_nvenc".to_string(),
            "h264_amf".to_string(),
            "h264_qsv".to_string(),
            "libx264".to_string(),
        ];
        // Explicit user selection honored when available
        assert_eq!(select_best_encoder("h264_amf", &avail, None), "h264_amf");
        assert_eq!(select_best_encoder("libx264", &avail, None), "libx264");
        // Missing preferred falls back cleanly
        let amd_only = vec!["h264_amf".to_string(), "libx264".to_string()];
        assert_eq!(select_best_encoder("h264_nvenc", &amd_only, None), "h264_amf");
    }
}
