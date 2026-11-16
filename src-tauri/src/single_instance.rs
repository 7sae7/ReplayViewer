use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Manager};

fn resolve_instance_lock_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let dir = PathBuf::from(local_app_data).join("com.replayviewer.app");
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("instance.lock");
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        let dir = PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("com.replayviewer.app");
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("instance.lock");
    }

    std::env::temp_dir().join("replay_viewer_instance.lock")
}

/// Ensures only a single instance of `replay-viewer.exe` runs at a time.
/// - If an existing instance is already listening on its loopback socket, sends `"FOCUS_MAIN\n"`
///   so the running instance shows & focuses its window, and returns `None` so the duplicate process exits immediately.
/// - Otherwise binds a fresh loopback `TcpListener`, records its address in `instance.lock`, and returns the listener.
pub fn acquire_single_instance_listener() -> Option<TcpListener> {
    let lock_path = resolve_instance_lock_path();

    if let Ok(addr_str) = std::fs::read_to_string(&lock_path) {
        if let Ok(addr) = addr_str.trim().parse::<SocketAddr>() {
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(250)) {
                let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                let _ = stream.write_all(b"FOCUS_MAIN\n");
                let _ = stream.flush();
                return None;
            }
        }
    }

    if let Ok(listener) = TcpListener::bind("127.0.0.1:0") {
        if let Ok(local_addr) = listener.local_addr() {
            let _ = std::fs::write(&lock_path, local_addr.to_string());
        }
        return Some(listener);
    }

    None
}

/// Spawns a background thread on the primary instance that listens for `"FOCUS_MAIN"` signals
/// from subsequent `.exe` launches and brings the `main` Gallery window to the foreground.
pub fn spawn_single_instance_focus_handler(listener: TcpListener, app: AppHandle) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let _ = s.set_read_timeout(Some(Duration::from_millis(250)));
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).unwrap_or(0);
            let msg = String::from_utf8_lossy(&buf[..n]);
            if msg.contains("FOCUS_MAIN") {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
        }
    });
}
