use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::error::AppResult;
use crate::state::AppState;

/// Maximum dynamic per-app isolated audio tracks multiplexed alongside Desktop + Microphone.
pub const MAX_ISOLATED_TRACKS: usize = 4;
pub const PCM_SAMPLE_RATE: u32 = 48_000;
pub const PCM_CHANNELS: u16 = 2;
/// 20 ms of 48 kHz 2-channel 16-bit PCM (`48000 * 0.02 * 2ch * 2bytes` = 3,840 bytes).
pub const CHUNK_FRAMES_20MS: usize = 960;
pub const CHUNK_BYTES_20MS: usize = CHUNK_FRAMES_20MS * (PCM_CHANNELS as usize) * 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSessionInfo {
    pub pid: u32,
    pub exe_name: String,
    pub display_name: String,
    /// `"active"` | `"inactive"` | `"system"`
    pub state: String,
    /// Live peak amplitude `0.0 ..= 1.0` from `IAudioMeterInformation`
    pub peak_level: f32,
    /// `true` when actively emitting audio (or emitted within the 10-second hysteresis window)
    pub is_emitting_audio: bool,
    pub is_muted: bool,
    pub is_isolated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInputDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub enum AudioSourceKind {
    DesktopLoopback { exclude_pid: Option<u32> },
    ProcessSlot { slot_idx: usize },
    Microphone { device_id: String },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveAudioRoutePlan {
    pub desktop_exclude_pid: Option<u32>,
    pub isolated_slots: [Option<(u32, String)>; MAX_ISOLATED_TRACKS],
}

#[derive(Clone)]
pub struct LiveAudioGains {
    pub desktop_bits: Arc<std::sync::atomic::AtomicU32>,
    pub mic_bits: Arc<std::sync::atomic::AtomicU32>,
    pub app_gains: Arc<std::sync::RwLock<HashMap<String, f32>>>,
    pub route_gen: Arc<std::sync::atomic::AtomicU64>,
    pub route_plan: Arc<std::sync::RwLock<LiveAudioRoutePlan>>,
    pub slot_had_audio: Arc<[AtomicBool; MAX_ISOLATED_TRACKS]>,
}

#[derive(Debug, Clone)]
pub struct AudioPipeTrack {
    pub slot_index: usize,
    pub title: String,
    pub pipe_path: String,
    pub kind: AudioSourceKind,
    pub volume_gain: f32,
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn query_pid_exe_name(pid: u32) -> String {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return "System Sounds".to_string();
    }

    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return format!("pid_{}.exe", pid);
    };

    let mut buf = [0u16; 512];
    let mut size = buf.len() as u32;
    let mut result = format!("pid_{}.exe", pid);

    if unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    }
    .is_ok()
        && size > 0
    {
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        result = std::path::Path::new(&full)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(full);
    }

    let _ = unsafe { CloseHandle(handle) };
    result
}

/// Returns `true` for Windows system audio plumbing hosts (e.g. `svchost.exe`) that shouldn't
/// clutter the app mixer or steal the OS single-PID exclusion slot from user apps like browsers/games.
pub fn is_ignored_system_audio_host(exe_name_or_lower: &str) -> bool {
    matches!(
        exe_name_or_lower.trim().to_lowercase().as_str(),
        "svchost.exe"
            | "audiodg.exe"
            | "runtimebroker.exe"
            | "shellexperiencehost.exe"
            | "textinputhost.exe"
            | "searchhost.exe"
    )
}

/// Walks parent processes via Toolhelp snapshot up to the topmost ancestor sharing the same
/// executable filename (e.g. `msedge.exe`, `chrome.exe`, `Discord.exe`).
/// This ensures Windows `INCLUDE/EXCLUDE_TARGET_PROCESS_TREE` catches Chromium's `AudioService`
/// sibling utility process rather than only a single tab renderer PID.
#[cfg(target_os = "windows")]
fn resolve_same_exe_root_pid(pid: u32) -> u32 {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    if pid == 0 {
        return 0;
    }

    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return pid;
    };

    let mut parent_of: HashMap<u32, (u32, String)> = HashMap::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: core::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    unsafe {
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe = String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase();
                parent_of.insert(entry.th32ProcessID, (entry.th32ParentProcessID, exe));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }

    let Some((_, target_exe)) = parent_of.get(&pid) else {
        return pid;
    };
    let target_exe = target_exe.clone();
    let mut cur = pid;
    for _ in 0..16 {
        let Some((ppid, _)) = parent_of.get(&cur) else {
            break;
        };
        if *ppid == 0 || *ppid == cur {
            break;
        }
        match parent_of.get(ppid) {
            Some((_, p_exe)) if *p_exe == target_exe => cur = *ppid,
            _ => break,
        }
    }
    cur
}

#[cfg(not(target_os = "windows"))]
fn resolve_same_exe_root_pid(pid: u32) -> u32 {
    pid
}

/// Enumerates live OS audio sessions (`IAudioSessionManager2` + `IAudioMeterInformation` on Windows).
/// - Apps with no audio stream (Notepad, Calculator, Sublime Text) are never present in `IAudioSessionEnumerator`.
/// - Idle apps (`VS Code` idle, paused Spotify) with `state == Inactive` and `peak == 0.0` are tagged
///   `is_emitting_audio = false` unless they played sound within the last 10 seconds (`10,000 ms` hysteresis).
pub fn list_active_audio_sessions(
    app: &AppHandle,
    include_idle: bool,
) -> AppResult<Vec<AudioSessionInfo>> {
    let state: tauri::State<'_, AppState> = app.state();

    let (muted_set, isolated_set): (HashSet<String>, HashSet<String>) = {
        if let Ok(cfg) = crate::commands::settings_cmds::get_settings(state.clone()) {
            (
                cfg.audio_muted_apps
                    .into_iter()
                    .map(|s| s.to_lowercase())
                    .collect(),
                cfg.audio_isolated_apps
                    .into_iter()
                    .map(|s| s.to_lowercase())
                    .collect(),
            )
        } else {
            (HashSet::new(), HashSet::new())
        }
    };

    #[cfg(target_os = "windows")]
    {
        use windows::core::Interface;
        use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
        use windows::Win32::Media::Audio::{
            eConsole, eRender, AudioSessionStateActive, AudioSessionStateExpired,
            IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED,
        };

        let mut sessions_out: Vec<AudioSessionInfo> = Vec::new();
        let mut seen_exes: HashMap<String, usize> = HashMap::new();
        let now_ms = now_epoch_ms();
        let own_pid = std::process::id();

        unsafe {
            let coinit_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();

            if let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            {
                if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                    if let Ok(mgr) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                        if let Ok(session_enum) = mgr.GetSessionEnumerator() {
                            let count = session_enum.GetCount().unwrap_or(0);
                            let mut last_active_map = state
                                .audio_last_active_ms
                                .lock()
                                .map(|g| g.clone())
                                .unwrap_or_default();

                            for i in 0..count {
                                let Ok(ctrl) = session_enum.GetSession(i) else {
                                    continue;
                                };
                                let Ok(session_state) = ctrl.GetState() else {
                                    continue;
                                };
                                if session_state == AudioSessionStateExpired {
                                    continue;
                                }

                                let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
                                    continue;
                                };
                                let pid = ctrl2.GetProcessId().unwrap_or(0);
                                // COM S_OK (HRESULT 0) == System Sounds; S_FALSE (1) == regular app session
                                let is_sys = ctrl2.IsSystemSoundsSession() == windows::core::HRESULT(0);
                                if pid == 0 || is_sys || pid == own_pid {
                                    continue;
                                }

                                let peak = ctrl
                                    .cast::<IAudioMeterInformation>()
                                    .ok()
                                    .and_then(|m| m.GetPeakValue().ok())
                                    .unwrap_or(0.0);

                                let raw_display = ctrl2
                                    .GetDisplayName()
                                    .ok()
                                    .map(|pw| {
                                        let s = pw.to_string().unwrap_or_default();
                                        CoTaskMemFree(Some(pw.0 as *const _));
                                        s
                                    })
                                    .unwrap_or_default();

                                let exe_name = query_pid_exe_name(pid);
                                if is_ignored_system_audio_host(&exe_name) {
                                    continue;
                                }
                                let display_name = if raw_display.trim().is_empty()
                                    || raw_display.starts_with('@')
                                {
                                    exe_name
                                        .strip_suffix(".exe")
                                        .or_else(|| exe_name.strip_suffix(".EXE"))
                                        .unwrap_or(&exe_name)
                                        .to_string()
                                } else {
                                    raw_display
                                };

                                let is_currently_loud =
                                    session_state == AudioSessionStateActive && peak > 0.0001;
                                if is_currently_loud {
                                    last_active_map.insert(pid, now_ms);
                                }
                                let recently_active = last_active_map
                                    .get(&pid)
                                    .map(|&t| now_ms.saturating_sub(t) <= 10_000)
                                    .unwrap_or(false);

                                let is_emitting_audio = is_currently_loud
                                    || session_state == AudioSessionStateActive
                                    || recently_active;

                                let exe_lower = exe_name.to_lowercase();
                                let is_muted = muted_set.contains(&exe_lower);
                                let is_isolated = isolated_set.contains(&exe_lower);

                                if !include_idle && !is_emitting_audio && !is_muted && !is_isolated {
                                    continue;
                                }

                                let state_label = if is_currently_loud || session_state == AudioSessionStateActive {
                                    "active".to_string()
                                } else {
                                    "inactive".to_string()
                                };

                                // Merge multiple subprocesses sharing the same executable name (e.g., Chrome audio helper tabs)
                                if let Some(&existing_idx) = seen_exes.get(&exe_lower) {
                                    let existing = &mut sessions_out[existing_idx];
                                    if peak > existing.peak_level {
                                        existing.peak_level = peak;
                                    }
                                    if is_emitting_audio {
                                        existing.is_emitting_audio = true;
                                        existing.state = "active".to_string();
                                    }
                                    continue;
                                }

                                seen_exes.insert(exe_lower, sessions_out.len());
                                sessions_out.push(AudioSessionInfo {
                                    pid,
                                    exe_name,
                                    display_name,
                                    state: state_label,
                                    peak_level: peak,
                                    is_emitting_audio,
                                    is_muted,
                                    is_isolated,
                                });
                            }

                            if let Ok(mut guard) = state.audio_last_active_ms.lock() {
                                last_active_map.retain(|_, &mut t| now_ms.saturating_sub(t) <= 30_000);
                                *guard = last_active_map;
                            }
                        }
                    }
                }
            }

            if coinit_ok {
                CoUninitialize();
            }
        }

        // Sort strictly by executable name so app rows stay stationary while peaks fluctuate or apps are muted
        sessions_out.sort_by(|a, b| a.exe_name.to_lowercase().cmp(&b.exe_name.to_lowercase()));

        return Ok(sessions_out);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (muted_set, isolated_set, include_idle);
        Ok(Vec::new())
    }
}

/// Ultra-fast peak-only scanner (< 0.3 ms) skipping SQLite locks & display name lookups so UI meters
/// animate smoothly at ~15 Hz without lag.
pub fn get_live_app_audio_peaks() -> AppResult<HashMap<String, f32>> {
    let mut peaks: HashMap<String, f32> = HashMap::new();
    #[cfg(target_os = "windows")]
    {
        use windows::core::Interface;
        use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
        use windows::Win32::Media::Audio::{
            eConsole, eRender, AudioSessionStateActive, IAudioSessionControl2,
            IAudioSessionManager2, IMMDeviceEnumerator, MMDeviceEnumerator,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        };

        let own_pid = std::process::id();
        unsafe {
            let coinit_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            {
                if let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                    if let Ok(mgr) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                        if let Ok(session_enum) = mgr.GetSessionEnumerator() {
                            let count = session_enum.GetCount().unwrap_or(0);
                            for i in 0..count {
                                let Ok(ctrl) = session_enum.GetSession(i) else { continue };
                                if ctrl.GetState().ok() != Some(AudioSessionStateActive) {
                                    continue;
                                }
                                let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else { continue };
                                let pid = ctrl2.GetProcessId().unwrap_or(0);
                                if pid == 0
                                    || pid == own_pid
                                    || ctrl2.IsSystemSoundsSession() == windows::core::HRESULT(0)
                                {
                                    continue;
                                }
                                let peak = ctrl
                                    .cast::<IAudioMeterInformation>()
                                    .ok()
                                    .and_then(|m| m.GetPeakValue().ok())
                                    .unwrap_or(0.0);
                                let exe_lower = query_pid_exe_name(pid).to_lowercase();
                                if is_ignored_system_audio_host(&exe_lower) {
                                    continue;
                                }
                                let entry = peaks.entry(exe_lower).or_insert(0.0);
                                if peak > *entry {
                                    *entry = peak;
                                }
                            }
                        }
                    }
                }
            }
            if coinit_ok {
                CoUninitialize();
            }
        }
    }
    Ok(peaks)
}

/// Enumerates active microphone / capture input devices on the system (headsets, USB mics, array mics).
pub fn list_audio_input_devices() -> AppResult<Vec<AudioInputDevice>> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
        use windows::Win32::Media::Audio::{
            eCapture, eConsole, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
            COINIT_MULTITHREADED, STGM_READ,
        };

        let mut devices_out: Vec<AudioInputDevice> = Vec::new();
        unsafe {
            let coinit_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Ok(enumerator) =
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            {
                let default_id = enumerator
                    .GetDefaultAudioEndpoint(eCapture, eConsole)
                    .ok()
                    .and_then(|dev| {
                        dev.GetId().ok().map(|pw| {
                            let s = pw.to_string().unwrap_or_default();
                            CoTaskMemFree(Some(pw.0 as *const _));
                            s
                        })
                    })
                    .unwrap_or_default();

                if let Ok(coll) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) {
                    let count = coll.GetCount().unwrap_or(0);
                    for i in 0..count {
                        let Ok(dev) = coll.Item(i) else { continue };
                        let id = dev
                            .GetId()
                            .ok()
                            .map(|pw| {
                                let s = pw.to_string().unwrap_or_default();
                                CoTaskMemFree(Some(pw.0 as *const _));
                                s
                            })
                            .unwrap_or_default();
                        if id.is_empty() {
                            continue;
                        }

                        let friendly = dev
                            .OpenPropertyStore(STGM_READ)
                            .ok()
                            .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName).ok())
                            .map(|pv| pv.to_string().trim().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| format!("Microphone {}", i + 1));

                        devices_out.push(AudioInputDevice {
                            is_default: !default_id.is_empty() && id == default_id,
                            id,
                            name: friendly,
                        });
                    }
                }
            }
            if coinit_ok {
                CoUninitialize();
            }
        }
        devices_out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
        return Ok(devices_out);
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(Vec::new())
    }
}

/// Builds the list of dynamic WASAPI audio pipe tracks to stream into FFmpeg based on user settings
/// and live active audio sessions.
/// Recomputes active PID assignments for `DesktopLoopback` + `MAX_ISOLATED_TRACKS` dynamic
/// app pipes and bumps `audio_route_gen` so running WASAPI workers hot-swap with zero video frame skips.
pub fn refresh_live_audio_route_plan(
    app: &AppHandle,
    muted_apps: &[String],
    isolated_apps: &[String],
) -> LiveAudioRoutePlan {
    let state: tauri::State<'_, AppState> = app.state();
    let live_sessions = list_active_audio_sessions(app, true).unwrap_or_default();

    let mut plan = LiveAudioRoutePlan::default();
    let mut slot_pos = 0usize;

    for iso_exe in isolated_apps {
        if slot_pos >= MAX_ISOLATED_TRACKS {
            break;
        }
        let iso_lower = iso_exe.trim().to_lowercase();
        if iso_lower.is_empty()
            || muted_apps.iter().any(|m| m.trim().eq_ignore_ascii_case(&iso_lower))
        {
            continue;
        }
        if let Some(sess) = live_sessions
            .iter()
            .find(|s| s.exe_name.to_lowercase() == iso_lower)
        {
            let root_pid = resolve_same_exe_root_pid(sess.pid);
            plan.isolated_slots[slot_pos] = Some((root_pid, iso_lower));
            state.slot_had_audio[slot_pos].store(true, Ordering::Relaxed);
            slot_pos += 1;
        }
    }

    // Reserve Windows' single Desktop exclusion PID strictly for real user muted apps
    // (never svchost.exe, and never Isolated tracks since Master Mix only blends Desktop + Mic cleanly)
    for muted in muted_apps {
        let m_lower = muted.trim().to_lowercase();
        if is_ignored_system_audio_host(&m_lower) {
            continue;
        }
        if let Some(sess) = live_sessions
            .iter()
            .find(|s| s.exe_name.to_lowercase() == m_lower)
        {
            plan.desktop_exclude_pid = Some(resolve_same_exe_root_pid(sess.pid));
            break;
        }
    }

    if let Ok(mut guard) = state.audio_route_plan.write() {
        *guard = plan.clone();
    }
    state.audio_route_gen.fetch_add(1, Ordering::SeqCst);
    plan
}

/// Builds fixed named-pipe tracks for FFmpeg (Desktop + 4 Hot-Swappable App Slots + Microphone)
/// so Mute, Unmute, Isolate, and Track Volume never restart FFmpeg or drop video frames.
pub fn build_audio_track_plan(
    app: &AppHandle,
    record_desktop: bool,
    record_mic: bool,
    desktop_volume: f32,
    mic_volume: f32,
    _app_volumes: &HashMap<String, f32>,
    mic_device_id: &str,
    muted_apps: &[String],
    isolated_apps: &[String],
) -> Vec<AudioPipeTrack> {
    static PIPE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = PIPE_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tracks: Vec<AudioPipeTrack> = Vec::new();
    let app_pid = std::process::id();
    let mut pipe_idx = 0usize;

    let plan = refresh_live_audio_route_plan(app, muted_apps, isolated_apps);

    // Track 1: Desktop Audio (hot-swaps exclude_pid inside worker thread)
    if record_desktop {
        tracks.push(AudioPipeTrack {
            slot_index: pipe_idx,
            title: "Desktop Audio".to_string(),
            pipe_path: format!(r"\\.\pipe\rv_audio_{}_{}_{}", app_pid, seq, pipe_idx),
            kind: AudioSourceKind::DesktopLoopback {
                exclude_pid: plan.desktop_exclude_pid,
            },
            volume_gain: desktop_volume.clamp(0.0, 2.0),
        });
        pipe_idx += 1;
    }

    // 4 Pre-allocated Hot-Swappable App Track Slots (silent when idle, stripped on save if unused)
    if cfg!(target_os = "windows") {
        for slot_idx in 0..MAX_ISOLATED_TRACKS {
            let initial_title = plan.isolated_slots[slot_idx]
                .as_ref()
                .map(|(_, exe)| exe.clone())
                .unwrap_or_else(|| format!("Isolated App {}", slot_idx + 1));
            tracks.push(AudioPipeTrack {
                slot_index: pipe_idx,
                title: initial_title,
                pipe_path: format!(r"\\.\pipe\rv_audio_{}_{}_{}", app_pid, seq, pipe_idx),
                kind: AudioSourceKind::ProcessSlot { slot_idx },
                volume_gain: 1.0,
            });
            pipe_idx += 1;
        }
    }

    // Microphone Track
    if record_mic {
        tracks.push(AudioPipeTrack {
            slot_index: pipe_idx,
            title: "Microphone".to_string(),
            pipe_path: format!(r"\\.\pipe\rv_audio_{}_{}_{}", app_pid, seq, pipe_idx),
            kind: AudioSourceKind::Microphone {
                device_id: mic_device_id.trim().to_string(),
            },
            volume_gain: mic_volume.clamp(0.0, 2.0),
        });
    }

    tracks
}

#[cfg(target_os = "windows")]
mod win_wasapi_pipe {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU32;
    use windows::core::{implement, IUnknown, Interface, HSTRING, PROPVARIANT};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, ActivateAudioInterfaceAsync,
        IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
        IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
        IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, AUDIOCLIENT_ACTIVATION_PARAMS,
        AUDIOCLIENT_ACTIVATION_PARAMS_0, AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, WAVEFORMATEX,
    };
    use windows::Win32::Storage::FileSystem::{WriteFile, PIPE_ACCESS_OUTBOUND};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

    const WAVE_FORMAT_PCM_TAG: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 3;

    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationHandler {
        event: HANDLE,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
        fn ActivateCompleted(
            &self,
            _operation: Option<&IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            unsafe {
                let _ = SetEvent(self.event);
            }
            Ok(())
        }
    }

    unsafe fn activate_process_loopback_client(
        target_pid: u32,
        include_tree: bool,
    ) -> Option<IAudioClient> {
        let mode = if include_tree {
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
        } else {
            PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
        };

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: target_pid,
                    ProcessLoopbackMode: mode,
                },
            },
        };

        // CRITICAL: Wrap PROPVARIANT in ManuallyDrop so PropVariantClear NEVER calls
        // CoTaskMemFree() on our stack-allocated `params` (prevents 0xc0000374 heap corruption)!
        let mut propvar: core::mem::ManuallyDrop<PROPVARIANT> =
            core::mem::ManuallyDrop::new(core::mem::zeroed());
        #[repr(C)]
        struct RawBlobPropVariant {
            vt: u16,
            r1: u16,
            r2: u16,
            r3: u16,
            cb_size: u32,
            p_blob_data: *mut u8,
        }
        let raw_pv = (&mut *propvar) as *mut PROPVARIANT as *mut RawBlobPropVariant;
        (*raw_pv).vt = 65; // VT_BLOB
        (*raw_pv).cb_size = core::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;
        (*raw_pv).p_blob_data = &mut params as *mut _ as *mut u8;

        let event = CreateEventW(None, false, false, None).ok()?;
        let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler { event }.into();
        let iid = IAudioClient::IID;

        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &iid,
            Some((&*propvar) as *const _),
            &handler,
        )
        .ok()?;

        let wait = WaitForSingleObject(event, 2000);
        let _ = CloseHandle(event);
        if wait != WAIT_OBJECT_0 {
            return None;
        }

        let mut hr = windows::core::HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk).ok()?;
        if hr.is_err() {
            return None;
        }
        unk?.cast::<IAudioClient>().ok()
    }

    unsafe fn init_endpoint_client(
        is_mic: bool,
        device_id: Option<&str>,
    ) -> Option<(IAudioClient, WAVEFORMATEX)> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let data_flow = if is_mic { eCapture } else { eRender };
        let device = if let Some(id) = device_id.map(str::trim).filter(|s| !s.is_empty()) {
            enumerator
                .GetDevice(&HSTRING::from(id))
                .ok()
                .or_else(|| enumerator.GetDefaultAudioEndpoint(data_flow, eConsole).ok())?
        } else {
            enumerator
                .GetDefaultAudioEndpoint(data_flow, eConsole)
                .ok()?
        };
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;

        let mix_ptr = client.GetMixFormat().ok()?;
        let fmt = *mix_ptr;
        CoTaskMemFree(Some(mix_ptr as *const c_void));

        let flags = if is_mic {
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        } else {
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        };

        // Request 48kHz stereo 16-bit PCM with Windows automatic sample-rate conversion
        let desired = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM_TAG,
            nChannels: PCM_CHANNELS,
            nSamplesPerSec: PCM_SAMPLE_RATE,
            nAvgBytesPerSec: PCM_SAMPLE_RATE * (PCM_CHANNELS as u32) * 2,
            nBlockAlign: PCM_CHANNELS * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };

        if client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &desired, None)
            .is_ok()
        {
            return Some((client, desired));
        }

        // Many laptop arrays & headsets require mono initialization before resampling
        if is_mic {
            let desired_mono = WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_PCM_TAG,
                nChannels: 1,
                nSamplesPerSec: PCM_SAMPLE_RATE,
                nAvgBytesPerSec: PCM_SAMPLE_RATE * 2,
                nBlockAlign: 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            if client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &desired_mono, None)
                .is_ok()
            {
                return Some((client, desired_mono));
            }
        }

        // Fallback to native mix format
        if client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &fmt, None)
            .is_ok()
        {
            return Some((client, fmt));
        }

        None
    }

    unsafe fn init_process_loopback_client(
        pid: u32,
        include: bool,
    ) -> Option<(IAudioClient, WAVEFORMATEX)> {
        let client = activate_process_loopback_client(pid, include)?;
        let desired = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM_TAG,
            nChannels: PCM_CHANNELS,
            nSamplesPerSec: PCM_SAMPLE_RATE,
            nAvgBytesPerSec: PCM_SAMPLE_RATE * (PCM_CHANNELS as u32) * 2,
            nBlockAlign: PCM_CHANNELS * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 400_000, 0, &desired, None)
            .is_ok()
        {
            return Some((client, desired));
        }
        None
    }

    pub(crate) fn convert_packet_to_stereo_s16le(
        raw: &[u8],
        frames: usize,
        fmt: &WAVEFORMATEX,
        out_samples: &mut Vec<i16>,
    ) -> f32 {
        let ch = fmt.nChannels.max(1) as usize;
        let bits = fmt.wBitsPerSample;
        let mut peak = 0.0f32;
        let is_pcm_16 = bits == 16 && (fmt.wFormatTag == WAVE_FORMAT_PCM_TAG || fmt.wFormatTag == 0xFFFE);
        let is_pcm_24 = bits == 24 && (fmt.wFormatTag == WAVE_FORMAT_PCM_TAG || fmt.wFormatTag == 0xFFFE);
        let is_float = fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT_TAG
            || (fmt.wFormatTag == 0xFFFE && bits == 32);

        if is_pcm_16 {
            let needed = frames * ch * 2;
            if raw.len() >= needed {
                for f in 0..frames {
                    let base = f * ch * 2;
                    let l = i16::from_le_bytes([raw[base], raw[base + 1]]);
                    let r = if ch >= 2 {
                        i16::from_le_bytes([raw[base + 2], raw[base + 3]])
                    } else {
                        l
                    };
                    let lf = (l as f32 / 32768.0).abs();
                    let rf = (r as f32 / 32768.0).abs();
                    if lf > peak {
                        peak = lf;
                    }
                    if rf > peak {
                        peak = rf;
                    }
                    out_samples.push(l);
                    out_samples.push(r);
                }
            }
        } else if is_pcm_24 {
            let needed = frames * ch * 3;
            if raw.len() >= needed {
                for f in 0..frames {
                    let base = f * ch * 3;
                    let l = i16::from_le_bytes([raw[base + 1], raw[base + 2]]);
                    let r = if ch >= 2 {
                        i16::from_le_bytes([raw[base + 4], raw[base + 5]])
                    } else {
                        l
                    };
                    peak = peak
                        .max((l as f32 / 32768.0).abs())
                        .max((r as f32 / 32768.0).abs());
                    out_samples.push(l);
                    out_samples.push(r);
                }
            }
        } else if is_float && bits == 32 {
            let needed = frames * ch * 4;
            if raw.len() >= needed {
                for f in 0..frames {
                    let base = f * ch * 4;
                    let lf = f32::from_le_bytes([
                        raw[base],
                        raw[base + 1],
                        raw[base + 2],
                        raw[base + 3],
                    ])
                    .clamp(-1.0, 1.0);
                    let rf = if ch >= 2 {
                        f32::from_le_bytes([
                            raw[base + 4],
                            raw[base + 5],
                            raw[base + 6],
                            raw[base + 7],
                        ])
                        .clamp(-1.0, 1.0)
                    } else {
                        lf
                    };
                    peak = peak.max(lf.abs()).max(rf.abs());
                    out_samples.push((lf * 32767.0) as i16);
                    out_samples.push((rf * 32767.0) as i16);
                }
            }
        }
        peak
    }

    /// Spawns a background thread serving a Windows Named Pipe for one dynamic audio track
    /// with a strict 20 ms monotonic silence-padding loop so every track stays in lip-sync with video.
    /// Reads live gain cleanly on every 20 ms tick without restarting FFmpeg.
    pub fn spawn_named_pipe_audio_worker(
        track: AudioPipeTrack,
        stop_flag: Arc<AtomicBool>,
        had_audio_flag: Arc<AtomicBool>,
        live_gains: LiveAudioGains,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || unsafe {
            let coinit_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();

            let pipe_name = HSTRING::from(&track.pipe_path);
            let pipe_handle = CreateNamedPipeW(
                &pipe_name,
                PIPE_ACCESS_OUTBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                (CHUNK_BYTES_20MS * 8) as u32,
                (CHUNK_BYTES_20MS * 8) as u32,
                0,
                None,
            );
            if pipe_handle == INVALID_HANDLE_VALUE || pipe_handle.is_invalid() {
                if coinit_ok {
                    CoUninitialize();
                }
                return;
            }

            // Initialize initial WASAPI client BEFORE waiting for FFmpeg
            let mut last_route_gen = live_gains.route_gen.load(Ordering::Relaxed);
            let initial_plan = live_gains
                .route_plan
                .read()
                .map(|g| g.clone())
                .unwrap_or_default();

            let mut current_target_pid: Option<u32> = match &track.kind {
                AudioSourceKind::DesktopLoopback { .. } => initial_plan.desktop_exclude_pid,
                AudioSourceKind::ProcessSlot { slot_idx } => initial_plan
                    .isolated_slots
                    .get(*slot_idx)
                    .and_then(|o| o.as_ref().map(|(pid, _)| *pid)),
                AudioSourceKind::Microphone { .. } => None,
            };
            let mut current_slot_exe: Option<String> = match &track.kind {
                AudioSourceKind::ProcessSlot { slot_idx } => initial_plan
                    .isolated_slots
                    .get(*slot_idx)
                    .and_then(|o| o.as_ref().map(|(_, e)| e.clone())),
                _ => None,
            };

            let client_opt = match &track.kind {
                AudioSourceKind::DesktopLoopback { .. } => match current_target_pid {
                    Some(ex_pid) => init_process_loopback_client(ex_pid, false)
                        .or_else(|| init_endpoint_client(false, None)),
                    None => init_endpoint_client(false, None),
                },
                AudioSourceKind::ProcessSlot { .. } => match current_target_pid {
                    Some(pid) => init_process_loopback_client(pid, true),
                    None => None,
                },
                AudioSourceKind::Microphone { device_id } => {
                    init_endpoint_client(true, Some(device_id.as_str()))
                }
            };

            // Block until FFmpeg connects to this pipe (or bail if ConnectNamedPipe fails)
            let connected = ConnectNamedPipe(pipe_handle, None).is_ok()
                || windows::Win32::Foundation::GetLastError().0 == 535; // ERROR_PIPE_CONNECTED
            if !connected || stop_flag.load(Ordering::Relaxed) {
                let _ = CloseHandle(pipe_handle);
                if coinit_ok {
                    CoUninitialize();
                }
                return;
            }

            let (mut audio_client, mut capture_client, mut active_fmt) =
                if let Some((cl, fmt)) = client_opt {
                    let cap = cl.GetService::<IAudioCaptureClient>().ok();
                    if cap.is_some() {
                        let _ = cl.Start();
                    }
                    (Some(cl), cap, Some(fmt))
                } else {
                    (None, None, None)
                };

            let silence_20ms = vec![0u8; CHUNK_BYTES_20MS];
            let mut sample_queue: Vec<i16> = Vec::with_capacity(CHUNK_FRAMES_20MS * 8);
            let start_instant = Instant::now();
            let mut tick_idx: u64 = 0;

            while !stop_flag.load(Ordering::Relaxed) {
                // Check for live Mute / Unmute / Isolate route changes without restarting FFmpeg!
                let cur_gen = live_gains.route_gen.load(Ordering::Relaxed);
                if cur_gen != last_route_gen {
                    last_route_gen = cur_gen;
                    let plan = live_gains
                        .route_plan
                        .read()
                        .map(|g| g.clone())
                        .unwrap_or_default();

                    match &track.kind {
                        AudioSourceKind::DesktopLoopback { .. } => {
                            if plan.desktop_exclude_pid != current_target_pid {
                                current_target_pid = plan.desktop_exclude_pid;
                                if let Some(cl) = audio_client.take() {
                                    let _ = cl.Stop();
                                }
                                capture_client = None;
                                active_fmt = None;
                                sample_queue.clear();

                                let next_client = match current_target_pid {
                                    Some(ex_pid) => init_process_loopback_client(ex_pid, false)
                                        .or_else(|| init_endpoint_client(false, None)),
                                    None => init_endpoint_client(false, None),
                                };
                                if let Some((cl, fmt)) = next_client {
                                    let cap = cl.GetService::<IAudioCaptureClient>().ok();
                                    if cap.is_some() {
                                        let _ = cl.Start();
                                    }
                                    audio_client = Some(cl);
                                    capture_client = cap;
                                    active_fmt = Some(fmt);
                                }
                            }
                        }
                        AudioSourceKind::ProcessSlot { slot_idx } => {
                            let assigned = plan.isolated_slots.get(*slot_idx).and_then(|o| o.clone());
                            let next_pid = assigned.as_ref().map(|(p, _)| *p);
                            current_slot_exe = assigned.map(|(_, e)| e);
                            if next_pid != current_target_pid {
                                current_target_pid = next_pid;
                                if let Some(cl) = audio_client.take() {
                                    let _ = cl.Stop();
                                }
                                capture_client = None;
                                active_fmt = None;
                                sample_queue.clear();

                                if let Some(pid) = current_target_pid {
                                    live_gains.slot_had_audio[*slot_idx].store(true, Ordering::Relaxed);
                                    if let Some((cl, fmt)) = init_process_loopback_client(pid, true) {
                                        let cap = cl.GetService::<IAudioCaptureClient>().ok();
                                        if cap.is_some() {
                                            let _ = cl.Start();
                                        }
                                        audio_client = Some(cl);
                                        capture_client = cap;
                                        active_fmt = Some(fmt);
                                    }
                                }
                            }
                        }
                        AudioSourceKind::Microphone { .. } => {}
                    }
                }

                // Drain available WASAPI packets into sample_queue
                if let (Some(ref cap), Some(ref fmt)) = (&capture_client, &active_fmt) {
                    loop {
                        let Ok(packet_frames) = cap.GetNextPacketSize() else {
                            break;
                        };
                        if packet_frames == 0 {
                            break;
                        }
                        let mut data_ptr: *mut u8 = std::ptr::null_mut();
                        let mut num_frames = 0u32;
                        let mut flags = 0u32;
                        if cap
                            .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                            .is_err()
                        {
                            break;
                        }

                        const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
                        if num_frames > 0 && !data_ptr.is_null() && (flags & AUDCLNT_BUFFERFLAGS_SILENT) == 0 {
                            let byte_len =
                                (num_frames as usize) * (fmt.nBlockAlign.max(1) as usize);
                            let slice = std::slice::from_raw_parts(data_ptr, byte_len);
                            let peak = convert_packet_to_stereo_s16le(
                                slice,
                                num_frames as usize,
                                fmt,
                                &mut sample_queue,
                            );
                            if peak > 0.0002 {
                                had_audio_flag.store(true, Ordering::Relaxed);
                            }
                        } else if num_frames > 0 {
                            // Explicit silent buffer packet
                            sample_queue.resize(
                                sample_queue.len() + (num_frames as usize) * 2,
                                0i16,
                            );
                        }
                        let _ = cap.ReleaseBuffer(num_frames);
                    }
                }

                // Cap buffered backlog to max ~120ms so tracks never drift far behind wall clock
                let max_backlog_samples = CHUNK_FRAMES_20MS * 2 * 6;
                if sample_queue.len() > max_backlog_samples {
                    let drop_cnt = sample_queue.len() - max_backlog_samples;
                    sample_queue.drain(0..drop_cnt);
                }

                // Read live gain atomically on every 20ms tick (zero FFmpeg restart when moving sliders!)
                let gain = match &track.kind {
                    AudioSourceKind::DesktopLoopback { .. } => {
                        f32::from_bits(live_gains.desktop_bits.load(Ordering::Relaxed))
                    }
                    AudioSourceKind::Microphone { .. } => {
                        f32::from_bits(live_gains.mic_bits.load(Ordering::Relaxed))
                    }
                    AudioSourceKind::ProcessSlot { .. } => {
                        if let Some(ref key) = current_slot_exe {
                            live_gains
                                .app_gains
                                .read()
                                .ok()
                                .and_then(|g| g.get(key).copied())
                                .unwrap_or(1.0)
                        } else {
                            0.0
                        }
                    }
                }
                .clamp(0.0, 2.0);
                let target_samples = CHUNK_FRAMES_20MS * 2;
                let mut written = 0u32;
                let write_ok = if sample_queue.len() >= target_samples {
                    let mut out_bytes = Vec::with_capacity(CHUNK_BYTES_20MS);
                    for s in sample_queue.drain(0..target_samples) {
                        let scaled = if (gain - 1.0).abs() > 0.005 {
                            ((s as f32) * gain).clamp(-32768.0, 32767.0) as i16
                        } else {
                            s
                        };
                        out_bytes.extend_from_slice(&scaled.to_le_bytes());
                    }
                    WriteFile(pipe_handle, Some(&out_bytes), Some(&mut written), None).is_ok()
                } else {
                    WriteFile(pipe_handle, Some(&silence_20ms), Some(&mut written), None).is_ok()
                };

                if !write_ok {
                    break;
                }

                tick_idx += 1;
                let target_elapsed = Duration::from_millis(tick_idx * 20);
                let now_elapsed = start_instant.elapsed();
                if target_elapsed > now_elapsed {
                    std::thread::sleep(target_elapsed - now_elapsed);
                }
            }

            if let Some(cl) = audio_client {
                let _ = cl.Stop();
            }
            let _ = DisconnectNamedPipe(pipe_handle);
            let _ = CloseHandle(pipe_handle);
            if coinit_ok {
                CoUninitialize();
            }
        })
    }

    unsafe fn init_render_s16le_client() -> Option<(IAudioClient, IAudioRenderClient, u32)> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).ok()?;
        let desired = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM_TAG,
            nChannels: PCM_CHANNELS,
            nSamplesPerSec: PCM_SAMPLE_RATE,
            nAvgBytesPerSec: PCM_SAMPLE_RATE * (PCM_CHANNELS as u32) * 2,
            nBlockAlign: PCM_CHANNELS * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 600_000, 0, &desired, None)
            .is_err()
        {
            return None;
        }
        let buf_frames = client.GetBufferSize().unwrap_or(PCM_SAMPLE_RATE / 10);
        let render = client.GetService::<IAudioRenderClient>().ok()?;
        let _ = client.Start();
        Some((client, render, buf_frames))
    }

    /// Streams live WASAPI microphone capture peaks into `peak_bits` and optionally renders
    /// low-latency sidetone playback when `sidetone_flag` is enabled.
    pub fn spawn_microphone_test_worker(
        device_id: String,
        running_flag: Arc<AtomicBool>,
        sidetone_flag: Arc<AtomicBool>,
        peak_bits: Arc<AtomicU32>,
        gain_bits: Arc<AtomicU32>,
    ) {
        std::thread::spawn(move || unsafe {
            let coinit_ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let Some((cap_client, cap_fmt)) = init_endpoint_client(true, Some(&device_id)) else {
                running_flag.store(false, Ordering::SeqCst);
                peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                if coinit_ok {
                    CoUninitialize();
                }
                return;
            };
            let Ok(capture) = cap_client.GetService::<IAudioCaptureClient>() else {
                running_flag.store(false, Ordering::SeqCst);
                if coinit_ok {
                    CoUninitialize();
                }
                return;
            };
            let _ = cap_client.Start();

            let mut render_opt: Option<(IAudioClient, IAudioRenderClient, u32)> = None;
            let mut sample_buf: Vec<i16> = Vec::with_capacity(CHUNK_FRAMES_20MS * 8);
            let mut smoothed = 0.0f32;

            while running_flag.load(Ordering::Relaxed) {
                let mut packet_peak = 0.0f32;
                sample_buf.clear();

                loop {
                    let Ok(pkt_frames) = capture.GetNextPacketSize() else {
                        break;
                    };
                    if pkt_frames == 0 {
                        break;
                    }
                    let mut data_ptr: *mut u8 = std::ptr::null_mut();
                    let mut num_frames = 0u32;
                    let mut flags = 0u32;
                    if capture
                        .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                        .is_err()
                    {
                        break;
                    }
                    const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
                    if num_frames > 0 && !data_ptr.is_null() && (flags & AUDCLNT_BUFFERFLAGS_SILENT) == 0 {
                        let byte_len =
                            (num_frames as usize) * (cap_fmt.nBlockAlign.max(1) as usize);
                        let slice = std::slice::from_raw_parts(data_ptr, byte_len);
                        let p = convert_packet_to_stereo_s16le(
                            slice,
                            num_frames as usize,
                            &cap_fmt,
                            &mut sample_buf,
                        );
                        if p > packet_peak {
                            packet_peak = p;
                        }
                    }
                    let _ = capture.ReleaseBuffer(num_frames);
                }

                let gain = f32::from_bits(gain_bits.load(Ordering::Relaxed)).clamp(0.0, 2.0);
                if (gain - 1.0).abs() > 0.005 {
                    for s in &mut sample_buf {
                        *s = ((*s as f32) * gain).clamp(-32768.0, 32767.0) as i16;
                    }
                    packet_peak = (packet_peak * gain).clamp(0.0, 1.0);
                }

                smoothed = (smoothed * 0.72).max(packet_peak);
                peak_bits.store(smoothed.to_bits(), Ordering::Relaxed);

                if sidetone_flag.load(Ordering::Relaxed) {
                    if render_opt.is_none() {
                        render_opt = init_render_s16le_client();
                    }
                    if let Some((ref rcl, ref rcli, max_frames)) = render_opt {
                        let frames_captured = (sample_buf.len() / 2) as u32;
                        if frames_captured > 0 {
                            let padding = rcl.GetCurrentPadding().unwrap_or(0);
                            let avail = max_frames.saturating_sub(padding);
                            let to_write = frames_captured.min(avail);
                            if to_write > 0 {
                                if let Ok(dst) = rcli.GetBuffer(to_write) {
                                    let dst_slice = std::slice::from_raw_parts_mut(
                                        dst,
                                        (to_write as usize) * 4,
                                    );
                                    for (i, &smp) in
                                        sample_buf.iter().take((to_write as usize) * 2).enumerate()
                                    {
                                        let le = smp.to_le_bytes();
                                        dst_slice[i * 2] = le[0];
                                        dst_slice[i * 2 + 1] = le[1];
                                    }
                                    let _ = rcli.ReleaseBuffer(to_write, 0);
                                }
                            }
                        }
                    }
                } else if let Some((rcl, _, _)) = render_opt.take() {
                    let _ = rcl.Stop();
                }

                std::thread::sleep(Duration::from_millis(16));
            }

            if let Some((rcl, _, _)) = render_opt {
                let _ = rcl.Stop();
            }
            let _ = cap_client.Stop();
            peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            if coinit_ok {
                CoUninitialize();
            }
        });
    }
}

#[cfg(target_os = "windows")]
pub use win_wasapi_pipe::{spawn_microphone_test_worker, spawn_named_pipe_audio_worker};

#[cfg(not(target_os = "windows"))]
pub fn spawn_named_pipe_audio_worker(
    _track: AudioPipeTrack,
    _stop_flag: Arc<AtomicBool>,
    _had_audio_flag: Arc<AtomicBool>,
    _live_gains: LiveAudioGains,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| {})
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_microphone_test_worker(
    _device_id: String,
    running_flag: std::sync::Arc<AtomicBool>,
    _sidetone_flag: std::sync::Arc<AtomicBool>,
    peak_bits: std::sync::Arc<std::sync::atomic::AtomicU32>,
    _gain_bits: std::sync::Arc<std::sync::atomic::AtomicU32>,
) {
    running_flag.store(false, Ordering::SeqCst);
    peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::win_wasapi_pipe::convert_packet_to_stereo_s16le;
    use windows::Win32::Media::Audio::WAVEFORMATEX;

    #[test]
    fn test_pcm_and_extensible_conversion_to_stereo_s16le() {
        // Mono 16-bit WAVE_FORMAT_EXTENSIBLE (0xFFFE)
        let fmt_mono_ext = WAVEFORMATEX {
            wFormatTag: 0xFFFE,
            nChannels: 1,
            nSamplesPerSec: 48_000,
            nAvgBytesPerSec: 96_000,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 22,
        };
        let mono_bytes = 16384i16.to_le_bytes();
        let mut out = Vec::new();
        let peak = convert_packet_to_stereo_s16le(&mono_bytes, 1, &fmt_mono_ext, &mut out);
        assert_eq!(out, vec![16384, 16384]);
        assert!((peak - 0.5).abs() < 0.01);
        assert!(super::is_ignored_system_audio_host("svchost.exe"));
        assert!(!super::is_ignored_system_audio_host("msedge.exe"));
    }
}
