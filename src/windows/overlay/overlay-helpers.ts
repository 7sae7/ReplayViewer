import { invoke } from "@tauri-apps/api/core";
import {
  getFrozenCaptureUrl,
  isTauriRuntime,
  saveAnnotatedCapture,
} from "../../lib/tauri-client";
import type { CaptureSessionMeta } from "../../types/ipc";

export { getFrozenCaptureUrl, isTauriRuntime, saveAnnotatedCapture };

export async function getLatestCaptureMetaFallback(): Promise<CaptureSessionMeta> {
  if (isTauriRuntime()) {
    try {
      const meta = await invoke<CaptureSessionMeta | null>("get_latest_capture_meta");
      if (meta) return meta;
    } catch {
      // Fall through to default viewport meta
    }
  }
  return {
    sessionId: `session-${Date.now()}`,
    monitorId: 1,
    width: window.innerWidth || 1920,
    height: window.innerHeight || 1080,
    scaleFactor: window.devicePixelRatio || 1,
    capturedAt: new Date().toISOString(),
  };
}
