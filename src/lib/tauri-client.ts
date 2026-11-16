import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type {
  AudioInputDevice,
  AudioSessionInfo,
  CaptureRecord,
  CaptureSessionMeta,
  ClipboardItem,
  FfmpegStatus,
  GalleryFilter,
  PaginatedCaptures,
  ReplayBufferSnapshot,
  SettingsPayload,
  StorageStatusSummary,
  UnindexedScanResult,
  VideoProbeInfo,
} from "../types/ipc";

export function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  );
}

// ---------------------------------------------------------------------------
// Browser Dev / Preview Fallback State (Used when running `vite` outside Tauri)
// ---------------------------------------------------------------------------
let mockCaptures: CaptureRecord[] = [];
let mockClipboardItems: ClipboardItem[] = [];
let mockClipboardPaused = false;
let mockClipboardAlwaysOnTop = true;
let mockInitialized = false;

function createMockSvgDataUrl(
  title: string,
  subtitle: string,
  accent: string,
  width = 1920,
  height = 1080
): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
    <defs>
      <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0%" stop-color="#0f172a"/>
        <stop offset="50%" stop-color="#1e1b4b"/>
        <stop offset="100%" stop-color="#090d16"/>
      </linearGradient>
    </defs>
    <rect width="100%" height="100%" fill="url(#bg)"/>
    <g stroke="rgba(148,163,184,0.08)" stroke-width="2">
      <line x1="0" y1="270" x2="${width}" y2="270"/>
      <line x1="0" y1="540" x2="${width}" y2="540"/>
      <line x1="0" y1="810" x2="${width}" y2="810"/>
      <line x1="480" y1="0" x2="480" y2="${height}"/>
      <line x1="960" y1="0" x2="960" y2="${height}"/>
      <line x1="1440" y1="0" x2="1440" y2="${height}"/>
    </g>
    <rect x="140" y="130" width="1640" height="820" rx="20" fill="#111827" stroke="${accent}" stroke-width="3" stroke-opacity="0.6"/>
    <circle cx="195" cy="180" r="10" fill="#ef4444"/>
    <circle cx="230" cy="180" r="10" fill="#f59e0b"/>
    <circle cx="265" cy="180" r="10" fill="#10b981"/>
    <text x="310" y="188" fill="#94a3b8" font-family="monospace" font-size="24">${title}</text>
    <rect x="190" y="230" width="740" height="360" rx="12" fill="#0b0f17" stroke="#1f2937" stroke-width="2"/>
    <text x="230" y="300" fill="${accent}" font-family="monospace" font-size="30" font-weight="bold">${subtitle}</text>
    <text x="230" y="365" fill="#cbd5e1" font-family="monospace" font-size="22">API_SECRET_TOKEN = "sk_live_98a7bc65d4e3f210"</text>
    <text x="230" y="415" fill="#94a3b8" font-family="monospace" font-size="22">STATUS: 60.0 FPS | VRAM: 41.2 MB | SQLite WAL</text>
    <text x="230" y="465" fill="#64748b" font-family="monospace" font-size="20">Tip: Use Pixelate tool to redact sensitive tokens</text>
    <rect x="970" y="230" width="760" height="660" rx="12" fill="#0b0f17" stroke="#1f2937" stroke-width="2"/>
    <circle cx="1350" cy="540" r="170" fill="none" stroke="${accent}" stroke-width="18" stroke-dasharray="620 220"/>
    <text x="1350" y="555" text-anchor="middle" fill="#f8fafc" font-family="sans-serif" font-size="52" font-weight="bold">144 FPS</text>
  </svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function ensureMockSeed() {
  if (mockInitialized) return;
  mockInitialized = true;

  const presets = [
    {
      title: "Valorant_Ascent_Clutch.webp",
      app: "Valorant",
      group: "Esports",
      tags: ["valorant", "clutch", "highlight"],
      ocr: "MATCH MVP 14-12 SCOREBOARD",
      accent: "#3b82f6",
      pinned: true,
      copied: true,
    },
    {
      title: "Production_Server_Error_500.webp",
      app: "Chrome DevTools",
      group: "Bug Reports",
      tags: ["bug", "backend", "urgent"],
      ocr: "HTTP 500 Internal Server Error SQLite FTS5 trace",
      accent: "#ef4444",
      pinned: true,
      copied: true,
    },
    {
      title: "Architecture_Diagram_Review.webp",
      app: "VS Code",
      group: "Architecture",
      tags: ["architecture", "rust", "tauri"],
      ocr: "ScreenCaptureProvider trait xcap zero base64",
      accent: "#10b981",
      pinned: false,
      copied: false,
    },
    {
      title: "Cyberpunk_Benchmark_4K.webp",
      app: "Cyberpunk 2077",
      group: "Benchmarks",
      tags: ["benchmark", "hdr", "gaming"],
      ocr: "3840x2160 DLSS Raytracing 98 FPS",
      accent: "#f59e0b",
      pinned: false,
      copied: false,
    },
    {
      title: "AWS_Invoice_Receipt_Nov.webp",
      app: "Firefox",
      group: "",
      tags: ["receipt", "finance"],
      ocr: "Invoice #94810 Total $24.50 Paid",
      accent: "#8b5cf6",
      pinned: false,
      copied: false,
    },
    {
      title: "Elden_Ring_Boss_Hitbox.webp",
      app: "Elden Ring",
      group: "Esports",
      tags: ["gaming", "boss"],
      ocr: "YOU DIED Stamina Bar Frame Data",
      accent: "#ec4899",
      pinned: false,
      copied: false,
    },
  ];

  const now = Date.now();
  mockCaptures = presets.map((p, idx) => {
    const imgUrl = createMockSvgDataUrl(p.title, p.ocr, p.accent, 1920, 1080);
    const createdIso = new Date(now - idx * 3600_000).toISOString();
    return {
      id: `seed-${idx + 1}`,
      mediaType: "screenshot",
      filePath: imgUrl,
      thumbPath: imgUrl,
      width: 1920,
      height: 1080,
      sizeBytes: 245_000 + idx * 38_400,
      durationMs: null,
      sourceApp: p.app,
      ocrText: p.ocr,
      ocrStatus: "done",
      isPinned: p.pinned,
      groupName: p.group,
      lastCopiedAt: p.copied ? createdIso : null,
      queuedDeleteAt: null,
      tags: p.tags,
      createdAt: createdIso,
    };
  });

  mockClipboardItems = mockCaptures.slice(0, 4).map((c, idx) => ({
    id: `clip-seed-${idx + 1}`,
    blake3Hash: `mock-hash-${idx + 1}`,
    filePath: c.filePath,
    thumbPath: c.thumbPath,
    baseFilePath: c.filePath,
    annotationsJson: "[]",
    width: c.width,
    height: c.height,
    sizeBytes: c.sizeBytes,
    isPinned: idx === 0,
    isCurrentlyCopied: idx === 0,
    copiedAt: new Date(now - idx * 600_000).toISOString(),
  }));
}

let mockSettings: SettingsPayload = {
  hotkeyRegionCapture: "Alt+S",
  hotkeyFullscreenCapture: "Alt+Shift+S",
  hotkeyOpenGallery: "Alt+G",
  hotkeyVisualClipboard: "Alt+V",
  hotkeySaveReplay: "Alt+C",
  imageFormat: "webp",
  copyToClipboardOnSave: true,
  maxStorageGb: 10,
  autoCleanupDays: 0,
  clipboardMaxItems: 10,
  clipboardExcludedApps: [
    "1password",
    "bitwarden",
    "keepass",
    "keepassxc",
    "dashlane",
    "lastpass",
    "nordpass",
    "protonpass",
    "enpass",
    "inprivate",
    "incognito",
    "private browsing",
    "tor browser",
  ],
  replayEnabled: true,
  replayBufferSecs: 60,
  replayFps: 60,
  replayBitrateMbps: 20,
  replayEncoder: "auto",
  ffmpegCustomPath: "",
  audioRecordDesktop: true,
  audioRecordMic: true,
  audioSingleMix: false,
  audioMutedApps: [],
  audioIsolatedApps: [],
  launchOnStartup: false,
  capturesDir: "C:\\Users\\shawp\\Desktop\\ReplayViewer\\captures",
};

// ---------------------------------------------------------------------------
// Unified Client API
// ---------------------------------------------------------------------------

export function resolveMediaSrc(pathOrUrl: string): string {
  if (!pathOrUrl) return "";
  if (
    pathOrUrl.startsWith("data:") ||
    pathOrUrl.startsWith("blob:") ||
    pathOrUrl.startsWith("http") ||
    pathOrUrl.startsWith("capture://")
  ) {
    return pathOrUrl;
  }
  if (isTauriRuntime()) {
    return convertFileSrc(pathOrUrl);
  }
  return pathOrUrl;
}

export function getFrozenCaptureUrl(sessionId: string): string {
  if (isTauriRuntime()) {
    const isWindows = navigator.userAgent.toLowerCase().includes("windows");
    const base = isWindows
      ? "http://capture.localhost/current"
      : "capture://localhost/current";
    return `${base}?session=${encodeURIComponent(sessionId)}&t=${Date.now()}`;
  }
  return createMockSvgDataUrl(
    "Active_Monitor_Freeze.raw (1920x1080 @ 1.0x)",
    "Frozen Frame Ready for Region Crop & Redaction",
    "#3b82f6",
    1920,
    1080
  );
}

export async function triggerRegionCapture(): Promise<CaptureSessionMeta> {
  if (isTauriRuntime()) {
    return invoke<CaptureSessionMeta>("trigger_region_capture");
  }
  return {
    sessionId: `browser-${Date.now()}`,
    monitorId: 1,
    width: 1920,
    height: 1080,
    scaleFactor: window.devicePixelRatio || 1,
    capturedAt: new Date().toISOString(),
  };
}

export async function triggerFullscreenCapture(): Promise<CaptureRecord> {
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("trigger_fullscreen_capture");
  }
  ensureMockSeed();
  const url = createMockSvgDataUrl(
    `Fullscreen_${new Date().toLocaleTimeString()}`,
    "Instant Fullscreen Capture (Alt+Shift+S - Zero Focus Loss)",
    "#10b981",
    1920,
    1080
  );
  const nowIso = new Date().toISOString();
  const record: CaptureRecord = {
    id: `full-${Date.now()}`,
    mediaType: "screenshot",
    filePath: url,
    thumbPath: url,
    width: 1920,
    height: 1080,
    sizeBytes: 312_400,
    durationMs: null,
    sourceApp: "Fullscreen Capture",
    ocrText: "Instant Fullscreen Capture",
    ocrStatus: "done",
    isPinned: false,
    groupName: "",
    lastCopiedAt: nowIso,
    queuedDeleteAt: null,
    tags: ["fullscreen"],
    createdAt: nowIso,
  };
  mockCaptures = [record, ...mockCaptures];
  return record;
}

export async function saveAnnotatedCapture(
  imageBytes: Uint8Array,
  copyToClipboard: boolean,
  imageFormat: "webp" | "png",
  dataUrlFallback?: string,
  dimensions?: { width: number; height: number },
  options?: {
    saveToDisk?: boolean;
    baseImageBytes?: Uint8Array;
    annotationsJson?: string;
    sourceClipboardId?: string | null;
  }
): Promise<CaptureRecord> {
  const shouldSaveToDisk = options?.saveToDisk ?? true;
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("save_annotated_capture", {
      imageBytes: Array.from(imageBytes),
      copyToClipboard,
      saveToDisk: shouldSaveToDisk,
      imageFormat,
      sourceApp: options?.sourceClipboardId
        ? "Clipboard Annotation (Alt + V)"
        : "Region Capture",
      baseImageBytes: options?.baseImageBytes
        ? Array.from(options.baseImageBytes)
        : null,
      annotationsJson: options?.annotationsJson ?? null,
      sourceClipboardId: options?.sourceClipboardId ?? null,
    });
  }

  ensureMockSeed();
  const url =
    dataUrlFallback ||
    createMockSvgDataUrl("Annotated_Region.webp", "Region Capture", "#3b82f6");
  const nowIso = new Date().toISOString();
  const record: CaptureRecord = {
    id: shouldSaveToDisk ? `cap-${Date.now()}` : `copied-only-${Date.now()}`,
    mediaType: "screenshot",
    filePath: shouldSaveToDisk ? url : "",
    thumbPath: shouldSaveToDisk ? url : "",
    width: dimensions?.width ?? 1280,
    height: dimensions?.height ?? 720,
    sizeBytes: imageBytes.byteLength || 195_000,
    durationMs: null,
    sourceApp: options?.sourceClipboardId
      ? "Clipboard Annotation (Alt + V)"
      : "Region Capture",
    ocrText: "Annotated Region Screenshot",
    ocrStatus: "done",
    isPinned: false,
    groupName: "",
    lastCopiedAt: copyToClipboard || !shouldSaveToDisk ? nowIso : null,
    queuedDeleteAt: null,
    tags: options?.sourceClipboardId
      ? ["annotated", "clipboard"]
      : ["annotated"],
    createdAt: nowIso,
  };

  if (shouldSaveToDisk) {
    mockCaptures = [record, ...mockCaptures];
  }

  if (copyToClipboard || !shouldSaveToDisk || options?.sourceClipboardId) {
    if (options?.sourceClipboardId) {
      mockClipboardItems = mockClipboardItems.map((c) =>
        c.id === options.sourceClipboardId
          ? {
              ...c,
              filePath: url,
              thumbPath: url,
              annotationsJson: options.annotationsJson ?? "[]",
              width: record.width,
              height: record.height,
              copiedAt: nowIso,
              isCurrentlyCopied: true,
            }
          : { ...c, isCurrentlyCopied: false }
      );
    } else {
      const newClip: ClipboardItem = {
        id: `clip-${Date.now()}`,
        blake3Hash: `hash-${Date.now()}`,
        filePath: url,
        thumbPath: url,
        baseFilePath: url,
        annotationsJson: options?.annotationsJson ?? "[]",
        width: record.width,
        height: record.height,
        sizeBytes: record.sizeBytes,
        isPinned: false,
        isCurrentlyCopied: true,
        copiedAt: nowIso,
      };
      mockClipboardItems = [
        newClip,
        ...mockClipboardItems.map((c) => ({ ...c, isCurrentlyCopied: false })),
      ];
    }
  }
  return record;
}

export async function listCaptures(
  cursor: string | null,
  limit: number,
  filter: GalleryFilter
): Promise<PaginatedCaptures> {
  if (isTauriRuntime()) {
    return invoke<PaginatedCaptures>("list_captures", {
      cursor,
      limit,
      filter,
    });
  }

  ensureMockSeed();
  const q = (filter.searchQuery || "").trim().toLowerCase();
  const filtered = mockCaptures.filter((c) => {
    if (filter.onlyQueuedDeletion) {
      if (!c.queuedDeleteAt || c.isPinned) return false;
    } else {
      if (c.queuedDeleteAt) return false;
    }
    if (filter.onlyPinned && !c.isPinned) return false;
    if (filter.onlyRecentClipboard && !c.lastCopiedAt) return false;
    if (filter.mediaType && c.mediaType !== filter.mediaType) return false;
    if (filter.groupName && c.groupName !== filter.groupName) return false;
    if (filter.tag && !c.tags.includes(filter.tag)) return false;
    if (q) {
      const hay = `${c.sourceApp || ""} ${c.ocrText} ${c.tags.join(" ")} ${c.groupName} ${c.filePath}`.toLowerCase();
      return hay.includes(q);
    }
    return true;
  });

  const dir = filter.sortDir === "asc" ? 1 : -1;
  filtered.sort((a, b) => {
    if (filter.onlyQueuedDeletion) {
      return (a.queuedDeleteAt || "").localeCompare(b.queuedDeleteAt || "");
    }
    if (filter.sortBy === "size_bytes") {
      return (a.sizeBytes - b.sizeBytes) * dir;
    }
    if (filter.sortBy === "last_copied_at" || filter.onlyRecentClipboard) {
      return (a.lastCopiedAt || "").localeCompare(b.lastCopiedAt || "") * dir;
    }
    return a.createdAt.localeCompare(b.createdAt) * dir;
  });

  const groups = Array.from(
    new Set(mockCaptures.map((c) => c.groupName).filter(Boolean))
  ).sort();

  const activeItems = mockCaptures.filter((c) => !c.queuedDeleteAt);
  const queuedItems = mockCaptures.filter((c) => Boolean(c.queuedDeleteAt) && !c.isPinned);
  const totalBytes = activeItems.reduce((acc, item) => acc + item.sizeBytes, 0);
  const queuedDeletionBytes = queuedItems.reduce((acc, item) => acc + item.sizeBytes, 0);
  return {
    items: filtered.slice(0, limit),
    nextCursor: null,
    totalCount: activeItems.length,
    totalBytes,
    queuedDeletionCount: queuedItems.length,
    queuedDeletionBytes,
    groups,
  };
}

export async function scanUnindexedMedia(): Promise<UnindexedScanResult> {
  if (isTauriRuntime()) {
    return invoke<UnindexedScanResult>("scan_unindexed_media");
  }
  return {
    folderPath: mockSettings.capturesDir,
    count: 0,
    totalBytes: 0,
    sampleNames: [],
  };
}

export async function importExternalMedia(): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("import_external_media");
  }
  return 0;
}

export async function migrateCapturesToFolder(
  oldDir: string,
  newDir: string
): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("migrate_captures_to_folder", { oldDir, newDir });
  }
  return mockCaptures.length;
}

export async function togglePinCapture(id: string, pinned: boolean): Promise<void> {
  if (isTauriRuntime()) {
    return invoke<void>("toggle_pin_capture", { id, pinned });
  }
  ensureMockSeed();
  mockCaptures = mockCaptures.map((c) =>
    c.id === id
      ? { ...c, isPinned: pinned, queuedDeleteAt: pinned ? null : c.queuedDeleteAt }
      : c
  );
}

export async function restoreQueuedCaptures(
  ids: string[],
  alsoPin = false
): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("restore_queued_captures", { ids, alsoPin });
  }
  ensureMockSeed();
  let restored = 0;
  mockCaptures = mockCaptures.map((c) => {
    if (!ids.includes(c.id)) return c;
    restored++;
    return {
      ...c,
      queuedDeleteAt: null,
      isPinned: alsoPin ? true : c.isPinned,
    };
  });
  return restored;
}

export async function getStorageStatusPreview(
  maxStorageGb?: number,
  autoCleanupDays?: number
): Promise<StorageStatusSummary> {
  if (isTauriRuntime()) {
    return invoke<StorageStatusSummary>("get_storage_status_preview", {
      maxStorageGb: maxStorageGb ?? null,
      autoCleanupDays: autoCleanupDays ?? null,
    });
  }
  ensureMockSeed();
  const active = mockCaptures.filter((c) => !c.queuedDeleteAt);
  const pinned = active.filter((c) => c.isPinned);
  const unpinned = active.filter((c) => !c.isPinned);
  const queued = mockCaptures.filter((c) => Boolean(c.queuedDeleteAt) && !c.isPinned);
  return {
    activeCount: active.length,
    activeBytes: active.reduce((acc, c) => acc + c.sizeBytes, 0),
    pinnedCount: pinned.length,
    pinnedBytes: pinned.reduce((acc, c) => acc + c.sizeBytes, 0),
    unpinnedCount: unpinned.length,
    unpinnedBytes: unpinned.reduce((acc, c) => acc + c.sizeBytes, 0),
    queuedCount: queued.length,
    queuedBytes: queued.reduce((acc, c) => acc + c.sizeBytes, 0),
    nextExpiryAt: queued[0]?.queuedDeleteAt ?? null,
    candidateCount: 0,
    candidateBytes: 0,
    candidateSamples: [],
    expiredPurgedCount: 0,
    pendingOcrCount: mockCaptures.filter((c) => c.ocrStatus === "pending").length,
  };
}

export async function enforceStoragePoliciesNow(): Promise<StorageStatusSummary> {
  if (isTauriRuntime()) {
    return invoke<StorageStatusSummary>("enforce_storage_policies_now");
  }
  return getStorageStatusPreview();
}

export async function runOcrBackfillNow(): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("run_ocr_backfill_now");
  }
  return 0;
}

export async function setShortcutRecordingMode(
  recording: boolean
): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("set_shortcut_recording_mode", { recording });
  }
}

// ---------------------------------------------------------------------------
// Phase 2.0.1 & 2.0.2: Hardware Replay Buffer & Dynamic Audio Tracking API
// ---------------------------------------------------------------------------

export async function getFfmpegStatus(
  forceRefresh = false
): Promise<FfmpegStatus> {
  if (isTauriRuntime()) {
    return invoke<FfmpegStatus>("get_ffmpeg_status", { forceRefresh });
  }
  return {
    available: true,
    binaryPath: "ffmpeg",
    source: "system",
    version: "ffmpeg version 7.1 (dev preview)",
    supportsDdagrab: true,
    availableEncoders: ["h264_nvenc", "h264_amf", "h264_qsv", "libx264"],
    recommendedEncoder: "h264_nvenc",
  };
}

export async function downloadFfmpegEngine(): Promise<FfmpegStatus> {
  if (isTauriRuntime()) {
    return invoke<FfmpegStatus>("download_ffmpeg_engine");
  }
  return getFfmpegStatus();
}

export async function getReplayBufferSnapshot(): Promise<ReplayBufferSnapshot> {
  if (isTauriRuntime()) {
    return invoke<ReplayBufferSnapshot>("get_replay_buffer_snapshot");
  }
  return {
    runtime: {
      isRecording: mockSettings.replayEnabled ?? true,
      activeEncoder: "h264_nvenc",
      captureMethod: "DXGI Desktop Duplication (ddagrab)",
      segmentCount: Math.round((mockSettings.replayBufferSecs ?? 60) / 2),
      ringBytes: 150 * 1024 * 1024,
      activeTracks: ["Desktop Audio", "Microphone"],
      lastError: null,
    },
    bufferSecs: mockSettings.replayBufferSecs ?? 60,
    fps: mockSettings.replayFps ?? 60,
    bitrateMbps: mockSettings.replayBitrateMbps ?? 20,
    configuredEncoder: mockSettings.replayEncoder ?? "auto",
    ffmpegAvailable: true,
    ffmpegSource: "system",
    availableEncoders: ["h264_nvenc", "h264_amf", "h264_qsv", "libx264"],
    recommendedEncoder: "h264_nvenc",
  };
}

export async function setReplayBufferActive(active: boolean): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("set_replay_buffer_active", { active });
  }
  mockSettings = { ...mockSettings, replayEnabled: active };
  return active;
}

export async function saveReplayBuffer(): Promise<CaptureRecord> {
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("save_replay_buffer");
  }
  ensureMockSeed();
  const secs = mockSettings.replayBufferSecs ?? 60;
  const nowIso = new Date().toISOString();
  const imgUrl = createMockSvgDataUrl(
    `Replay_${secs}s.mp4`,
    `Instant ${secs}s Replay Clip (${mockSettings.replayFps ?? 60} FPS)`,
    "#10b981",
    1920,
    1080
  );
  const record: CaptureRecord = {
    id: `replay-${Date.now()}`,
    mediaType: "video",
    filePath: imgUrl,
    thumbPath: imgUrl,
    width: 1920,
    height: 1080,
    sizeBytes: secs * 2_500_000,
    durationMs: secs * 1000,
    sourceApp: `Instant Replay (${secs}s)`,
    ocrText: "",
    ocrStatus: "done",
    isPinned: false,
    groupName: "Replays",
    lastCopiedAt: null,
    queuedDeleteAt: null,
    tags: ["replay", `${secs}s`],
    createdAt: nowIso,
  };
  mockCaptures = [record, ...mockCaptures];
  return record;
}

let mockAudioSessions: AudioSessionInfo[] = [
  {
    pid: 14020,
    exeName: "Valorant-Win64-Shipping.exe",
    displayName: "VALORANT",
    state: "active",
    peakLevel: 0.72,
    isEmittingAudio: true,
    isMuted: false,
    isIsolated: false,
  },
  {
    pid: 8824,
    exeName: "Discord.exe",
    displayName: "Discord",
    state: "active",
    peakLevel: 0.44,
    isEmittingAudio: true,
    isMuted: false,
    isIsolated: true,
  },
  {
    pid: 19400,
    exeName: "chrome.exe",
    displayName: "Google Chrome",
    state: "active",
    peakLevel: 0.31,
    isEmittingAudio: true,
    isMuted: false,
    isIsolated: false,
  },
  {
    pid: 5112,
    exeName: "Spotify.exe",
    displayName: "Spotify",
    state: "inactive",
    peakLevel: 0.0,
    isEmittingAudio: false,
    isMuted: true,
    isIsolated: false,
  },
];

export async function listActiveAudioSessions(
  includeIdle = false
): Promise<AudioSessionInfo[]> {
  if (isTauriRuntime()) {
    return invoke<AudioSessionInfo[]>("list_active_audio_sessions", {
      includeIdle,
    });
  }
  const muted = new Set((mockSettings.audioMutedApps || []).map((s) => s.toLowerCase()));
  const isolated = new Set((mockSettings.audioIsolatedApps || []).map((s) => s.toLowerCase()));
  return mockAudioSessions
    .filter((s) => includeIdle || s.isEmittingAudio || muted.has(s.exeName.toLowerCase()))
    .map((s) => ({
      ...s,
      isMuted: muted.has(s.exeName.toLowerCase()) || s.isMuted,
      isIsolated: isolated.has(s.exeName.toLowerCase()) || s.isIsolated,
    }));
}

export async function listAudioInputDevices(): Promise<AudioInputDevice[]> {
  if (isTauriRuntime()) {
    return invoke<AudioInputDevice[]>("list_audio_input_devices");
  }
  return [
    {
      id: "default-array",
      name: "Microphone Array (Built-in Laptop Mic)",
      isDefault: true,
    },
    {
      id: "headset-mic",
      name: "Headset Microphone (HyperX / Bluetooth)",
      isDefault: false,
    },
  ];
}

export async function pickFfmpegDialog(): Promise<string | null> {
  if (isTauriRuntime()) {
    return invoke<string | null>("pick_ffmpeg_dialog");
  }
  return null;
}

export async function startMicTest(
  deviceId?: string,
  sidetone = false
): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("start_mic_test", {
      deviceId: deviceId || null,
      sidetone,
    });
  }
  return true;
}

export async function stopMicTest(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("stop_mic_test");
  }
  return false;
}

export async function setMicTestSidetone(enabled: boolean): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("set_mic_test_sidetone", { enabled });
  }
  return enabled;
}

export async function getMicTestLevel(): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("get_mic_test_level");
  }
  return 0.15 + Math.random() * 0.45;
}

export async function getLiveAudioPeaks(): Promise<Record<string, number>> {
  if (isTauriRuntime()) {
    return invoke<Record<string, number>>("get_live_audio_peaks");
  }
  const out: Record<string, number> = {};
  for (const s of mockAudioSessions) {
    if (s.isEmittingAudio) {
      out[s.exeName.toLowerCase()] = Math.max(
        0.04,
        Math.min(0.95, s.peakLevel + (Math.random() - 0.5) * 0.25)
      );
    }
  }
  return out;
}

export async function setMicTestGain(gain: number): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("set_mic_test_gain", { gain });
  }
  return gain;
}

export async function setAudioTrackVolumes(options: {
  desktopVolume?: number;
  micVolume?: number;
  appExeName?: string;
  appVolume?: number;
}): Promise<void> {
  if (isTauriRuntime()) {
    return invoke<void>("set_audio_track_volumes", {
      desktopVolume: options.desktopVolume ?? null,
      micVolume: options.micVolume ?? null,
      appExeName: options.appExeName ?? null,
      appVolume: options.appVolume ?? null,
    });
  }
  if (options.desktopVolume !== undefined) {
    mockSettings = { ...mockSettings, audioDesktopVolume: options.desktopVolume };
  }
  if (options.micVolume !== undefined) {
    mockSettings = { ...mockSettings, audioMicVolume: options.micVolume };
  }
  if (options.appExeName && options.appVolume !== undefined) {
    mockSettings = {
      ...mockSettings,
      audioAppVolumes: {
        ...(mockSettings.audioAppVolumes || {}),
        [options.appExeName.toLowerCase()]: options.appVolume,
      },
    };
  }
}

export async function updateAppAudioRule(
  exeName: string,
  options: { muted?: boolean; isolated?: boolean }
): Promise<void> {
  if (isTauriRuntime()) {
    return invoke<void>("update_app_audio_rule", {
      exeName,
      muted: options.muted ?? null,
      isolated: options.isolated ?? null,
    });
  }
  const clean = exeName.trim().toLowerCase();
  let mutedList = [...(mockSettings.audioMutedApps || [])];
  let isoList = [...(mockSettings.audioIsolatedApps || [])];
  if (options.muted !== undefined) {
    mutedList = mutedList.filter((a) => a.toLowerCase() !== clean);
    if (options.muted) {
      mutedList.push(exeName);
      isoList = isoList.filter((a) => a.toLowerCase() !== clean);
    }
  }
  if (options.isolated !== undefined) {
    isoList = isoList.filter((a) => a.toLowerCase() !== clean);
    if (options.isolated) {
      isoList.push(exeName);
      mutedList = mutedList.filter((a) => a.toLowerCase() !== clean);
    }
  }
  mockSettings = {
    ...mockSettings,
    audioMutedApps: mutedList,
    audioIsolatedApps: isoList,
  };
  mockAudioSessions = mockAudioSessions.map((s) =>
    s.exeName.toLowerCase() === clean
      ? {
          ...s,
          isMuted: options.muted ?? s.isMuted,
          isIsolated: options.isolated ?? s.isIsolated,
        }
      : s
  );
}

export interface TrimVideoClipOptions {
  id: string;
  startMs: number;
  endMs: number;
  selectedAudioIndices?: number[];
  mixSelectedAudio?: boolean;
  targetMaxMb?: number | null;
  replaceOriginal?: boolean;
}

export async function probeVideoAudioTracks(id: string): Promise<VideoProbeInfo> {
  if (isTauriRuntime()) {
    return invoke<VideoProbeInfo>("probe_video_audio_tracks", { id });
  }
  ensureMockSeed();
  const target = mockCaptures.find((c) => c.id === id);
  return {
    durationMs: target?.durationMs ?? 30_000,
    sizeBytes: target?.sizeBytes ?? 28 * 1024 * 1024,
    width: target?.width || 1920,
    height: target?.height || 1080,
    audioTracks: [
      { streamIndex: 0, title: "Master Mix", channels: 2, codec: "aac" },
      { streamIndex: 1, title: "Desktop Audio", channels: 2, codec: "aac" },
      { streamIndex: 2, title: "Spotify.exe", channels: 2, codec: "aac" },
      { streamIndex: 3, title: "Microphone", channels: 2, codec: "aac" },
    ],
  };
}

export async function trimVideoClip(
  options: TrimVideoClipOptions
): Promise<CaptureRecord> {
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("trim_video_clip_cmd", {
      id: options.id,
      startMs: Math.max(0, Math.round(options.startMs)),
      endMs: Math.max(0, Math.round(options.endMs)),
      selectedAudioIndices: options.selectedAudioIndices ?? null,
      mixSelectedAudio: options.mixSelectedAudio ?? false,
      targetMaxMb: options.targetMaxMb ?? null,
      replaceOriginal: options.replaceOriginal ?? false,
    });
  }
  ensureMockSeed();
  const source = mockCaptures.find((c) => c.id === options.id);
  if (!source) {
    throw new Error(`Capture not found: ${options.id}`);
  }
  const clippedDurationMs = Math.max(
    500,
    Math.round(options.endMs) - Math.round(options.startMs)
  );
  const fullDurationMs = Math.max(clippedDurationMs, source.durationMs ?? 30_000);
  const proportionalBytes = Math.max(
    64 * 1024,
    Math.round(source.sizeBytes * (clippedDurationMs / fullDurationMs))
  );
  const estimatedBytes = options.targetMaxMb
    ? Math.min(proportionalBytes, Math.round(options.targetMaxMb * 1024 * 1024 * 0.92))
    : proportionalBytes;
  const trimmedId = `trim-${Date.now()}-${Math.floor(Math.random() * 1000)}`;
  const trimmed: CaptureRecord = {
    ...source,
    id: trimmedId,
    filePath: source.filePath.replace(/\.mp4$/i, "") + `_trim_${trimmedId.slice(-4)}.mp4`,
    sizeBytes: estimatedBytes,
    durationMs: clippedDurationMs,
    createdAt: new Date().toISOString(),
    queuedDeleteAt: null,
  };
  if (options.replaceOriginal) {
    mockCaptures = mockCaptures.filter((c) => c.id !== source.id);
  }
  mockCaptures = [trimmed, ...mockCaptures];
  return trimmed;
}

export async function deleteCaptures(ids: string[]): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("delete_captures", { ids });
  }
  ensureMockSeed();
  const before = mockCaptures.length;
  mockCaptures = mockCaptures.filter((c) => !ids.includes(c.id));
  return before - mockCaptures.length;
}

export async function batchAddTags(
  captureIds: string[],
  tagNames: string[]
): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("batch_add_tags", { captureIds, tagNames });
  }
  ensureMockSeed();
  const cleaned = tagNames
    .map((t) => t.trim().replace(/^#/, "").toLowerCase())
    .filter(Boolean);
  mockCaptures = mockCaptures.map((c) => {
    if (!captureIds.includes(c.id)) return c;
    const next = Array.from(new Set([...c.tags, ...cleaned]));
    return { ...c, tags: next };
  });
  return captureIds.length;
}

export async function batchSetGroup(
  captureIds: string[],
  groupName: string
): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("batch_set_group", { captureIds, groupName });
  }
  ensureMockSeed();
  const clean = groupName.trim();
  mockCaptures = mockCaptures.map((c) =>
    captureIds.includes(c.id) ? { ...c, groupName: clean } : c
  );
  return captureIds.length;
}

export async function moveCapturesToFolder(
  captureIds: string[],
  targetDir: string
): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("move_captures_to_folder", { captureIds, targetDir });
  }
  ensureMockSeed();
  const before = mockCaptures.length;
  mockCaptures = mockCaptures.filter((c) => !captureIds.includes(c.id));
  return before - mockCaptures.length;
}

export async function pickFolderDialog(
  defaultPath?: string
): Promise<string | null> {
  if (isTauriRuntime()) {
    return invoke<string | null>("pick_folder_dialog", { defaultPath });
  }
  return defaultPath || "C:\\Users\\shawp\\Pictures\\ReplayViewer\\Exported";
}

export async function reconcileOrphanedCaptures(): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("reconcile_orphaned_captures");
  }
  return 0;
}

export async function openInFileExplorer(path: string): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("open_in_file_explorer", { path });
  }
}

export async function addTagToCapture(
  captureId: string,
  tagName: string
): Promise<CaptureRecord> {
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("add_tag_to_capture", { captureId, tagName });
  }
  ensureMockSeed();
  const clean = tagName.trim().toLowerCase();
  let updatedRecord = mockCaptures[0];
  mockCaptures = mockCaptures.map((c) => {
    if (c.id !== captureId) return c;
    const nextTags = c.tags.includes(clean) ? c.tags : [...c.tags, clean];
    updatedRecord = { ...c, tags: nextTags };
    return updatedRecord;
  });
  return updatedRecord;
}

export async function copyCaptureToClipboard(filePath: string): Promise<void> {
  if (isTauriRuntime()) {
    return invoke<void>("copy_capture_to_clipboard", { filePath });
  }
  ensureMockSeed();
  const nowIso = new Date().toISOString();
  mockCaptures = mockCaptures.map((c) =>
    c.filePath === filePath ? { ...c, lastCopiedAt: nowIso } : c
  );
  if (navigator.clipboard && filePath.startsWith("data:")) {
    try {
      const res = await fetch(filePath);
      const blob = await res.blob();
      await navigator.clipboard.write([
        new ClipboardItem({ [blob.type]: blob }),
      ]);
    } catch {
      // Ignore browser clipboard permission errors in non-HTTPS dev preview
    }
  }
}

export async function getSettings(): Promise<SettingsPayload> {
  if (isTauriRuntime()) {
    return invoke<SettingsPayload>("get_settings");
  }
  return { ...mockSettings };
}

export async function updateSettings(
  payload: SettingsPayload
): Promise<SettingsPayload> {
  if (isTauriRuntime()) {
    return invoke<SettingsPayload>("update_settings", { payload });
  }
  mockSettings = { ...payload };
  return { ...mockSettings };
}

export async function getDefaultCapturesDir(): Promise<string> {
  if (isTauriRuntime()) {
    return invoke<string>("get_default_captures_dir");
  }
  return "C:\\Users\\shawp\\Desktop\\ReplayViewer\\captures";
}

// ---------------------------------------------------------------------------
// Phase 1.5.1: Visual Image Clipboard (Alt + V) Client API
// ---------------------------------------------------------------------------

export async function toggleClipboardDrawer(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("toggle_clipboard_drawer");
  }
  return true;
}

export async function getClipboardAlwaysOnTop(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("get_clipboard_always_on_top");
  }
  return mockClipboardAlwaysOnTop;
}

export async function setClipboardAlwaysOnTop(
  alwaysOnTop: boolean
): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("set_clipboard_always_on_top", { alwaysOnTop });
  }
  mockClipboardAlwaysOnTop = alwaysOnTop;
  return mockClipboardAlwaysOnTop;
}

export async function listClipboardHistory(): Promise<ClipboardItem[]> {
  if (isTauriRuntime()) {
    return invoke<ClipboardItem[]>("list_clipboard_history");
  }
  ensureMockSeed();
  const hasExplicitCurrent = mockClipboardItems.some((i) => i.isCurrentlyCopied);
  let newestAt = "";
  if (!hasExplicitCurrent) {
    for (const item of mockClipboardItems) {
      if (item.copiedAt > newestAt) newestAt = item.copiedAt;
    }
  }
  return [...mockClipboardItems]
    .map((item) => ({
      ...item,
      isCurrentlyCopied: hasExplicitCurrent
        ? item.isCurrentlyCopied
        : item.copiedAt === newestAt,
    }))
    .sort((a, b) => {
      if (a.isPinned !== b.isPinned) return a.isPinned ? -1 : 1;
      return b.copiedAt.localeCompare(a.copiedAt);
    });
}

export async function pasteClipboardItem(id: string): Promise<ClipboardItem> {
  if (isTauriRuntime()) {
    return invoke<ClipboardItem>("paste_clipboard_item", { id });
  }
  ensureMockSeed();
  const nowIso = new Date().toISOString();
  let target = mockClipboardItems[0];
  mockClipboardItems = mockClipboardItems.map((item) => {
    if (item.id === id) {
      target = { ...item, copiedAt: nowIso, isCurrentlyCopied: true };
      return target;
    }
    return { ...item, isCurrentlyCopied: false };
  });
  return target;
}

export async function annotateClipboardItem(
  id: string
): Promise<CaptureSessionMeta> {
  if (isTauriRuntime()) {
    return invoke<CaptureSessionMeta>("annotate_clipboard_item", { id });
  }
  ensureMockSeed();
  const found = mockClipboardItems.find((i) => i.id === id);
  return {
    sessionId: `clip-annotate-${Date.now()}`,
    monitorId: 0,
    width: found?.width ?? 1920,
    height: found?.height ?? 1080,
    scaleFactor: 1.0,
    capturedAt: new Date().toISOString(),
    sourceClipboardId: found?.id ?? id,
    initialAnnotationsJson: found?.annotationsJson ?? "[]",
  };
}

export async function promoteClipboardToGallery(
  id: string
): Promise<CaptureRecord> {
  if (isTauriRuntime()) {
    return invoke<CaptureRecord>("promote_clipboard_to_gallery", { id });
  }
  ensureMockSeed();
  const item =
    mockClipboardItems.find((i) => i.id === id) || mockClipboardItems[0];
  const nowIso = new Date().toISOString();
  const record: CaptureRecord = {
    id: `promoted-${Date.now()}`,
    mediaType: "screenshot",
    filePath: item.filePath,
    thumbPath: item.thumbPath,
    width: item.width,
    height: item.height,
    sizeBytes: item.sizeBytes,
    durationMs: null,
    sourceApp: "Visual Clipboard (Alt + V)",
    ocrText: "",
    ocrStatus: "pending",
    isPinned: item.isPinned,
    groupName: "",
    lastCopiedAt: item.copiedAt,
    queuedDeleteAt: null,
    tags: ["clipboard"],
    createdAt: nowIso,
  };
  mockCaptures = [record, ...mockCaptures];
  return record;
}

export async function togglePinClipboardItem(
  id: string,
  pinned: boolean
): Promise<void> {
  if (isTauriRuntime()) {
    return invoke<void>("toggle_pin_clipboard_item", { id, pinned });
  }
  ensureMockSeed();
  mockClipboardItems = mockClipboardItems.map((i) =>
    i.id === id ? { ...i, isPinned: pinned } : i
  );
}

export async function deleteClipboardItem(id: string): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("delete_clipboard_item", { id });
  }
  ensureMockSeed();
  const before = mockClipboardItems.length;
  mockClipboardItems = mockClipboardItems.filter((i) => i.id !== id);
  return mockClipboardItems.length < before;
}

export async function clearUnpinnedClipboardHistory(): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("clear_unpinned_clipboard_history");
  }
  ensureMockSeed();
  const before = mockClipboardItems.length;
  mockClipboardItems = mockClipboardItems.filter((i) => i.isPinned);
  return before - mockClipboardItems.length;
}

export async function getClipboardPaused(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("get_clipboard_paused");
  }
  return mockClipboardPaused;
}

export async function setClipboardPaused(paused: boolean): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("set_clipboard_paused", { paused });
  }
  mockClipboardPaused = paused;
  return mockClipboardPaused;
}

export async function seedSyntheticBenchmarkItems(count = 500): Promise<number> {
  if (isTauriRuntime()) {
    return invoke<number>("seed_benchmark_captures", { count });
  }

  ensureMockSeed();
  const accents = ["#3b82f6", "#10b981", "#f59e0b", "#ef4444", "#8b5cf6"];
  const apps = ["Valorant", "Apex Legends", "VS Code", "Chrome", "Blender"];
  const groups = ["Esports", "Bug Reports", "Architecture", "Benchmarks", ""];
  const now = Date.now();
  const batch: CaptureRecord[] = [];

  for (let i = 0; i < count; i++) {
    const app = apps[i % apps.length];
    const accent = accents[i % accents.length];
    const url = mockCaptures[i % mockCaptures.length].thumbPath;
    const createdIso = new Date(now - (i + 10) * 60_000).toISOString();
    batch.push({
      id: `bench-${now}-${i}`,
      mediaType: "screenshot",
      filePath: url,
      thumbPath: url,
      width: 1920,
      height: 1080,
      sizeBytes: 210_000 + (i % 90) * 1024,
      durationMs: null,
      sourceApp: `${app} #${i + 1}`,
      ocrText: `Synthetic Benchmark Capture ${i + 1} (${accent})`,
      ocrStatus: "done",
      isPinned: i % 25 === 0,
      groupName: groups[i % groups.length],
      lastCopiedAt: i % 20 === 0 ? createdIso : null,
      queuedDeleteAt: null,
      tags: [app.toLowerCase().replace(/\s+/g, "-"), "benchmark"],
      createdAt: createdIso,
    });
  }

  mockCaptures = [...mockCaptures, ...batch];
  return mockCaptures.length;
}
