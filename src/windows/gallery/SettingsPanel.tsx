import React, { useCallback, useEffect, useState } from "react";
import {
  Check,
  Clock,
  Cpu,
  Download,
  Eye,
  FileText,
  Film,
  Folder,
  FolderInput,
  FolderOpen,
  FolderSearch,
  FolderSync,
  HardDrive,
  ImagePlus,
  Keyboard,
  Lock,
  MoreHorizontal,
  Pin,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  ShieldCheck,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { formatBytes, formatDeletionCountdown } from "../../lib/dpi";
import {
  enforceStoragePoliciesNow,
  getDefaultCapturesDir,
  getStorageStatusPreview,
  importExternalMedia,
  migrateCapturesToFolder,
  openInFileExplorer,
  pickFfmpegDialog,
  pickFolderDialog,
  runOcrBackfillNow,
  saveReplayBuffer,
  scanUnindexedMedia,
  setReplayBufferActive,
  setShortcutRecordingMode,
} from "../../lib/tauri-client";
import { useGalleryStore } from "../../stores/useGalleryStore";
import { useSettingsStore } from "../../stores/useSettingsStore";
import { AudioMixerCard } from "./AudioMixerCard";
import type {
  SettingsPayload,
  StorageStatusSummary,
  UnindexedScanResult,
} from "../../types/ipc";

type HotkeyFieldKey =
  | "hotkeyRegionCapture"
  | "hotkeyFullscreenCapture"
  | "hotkeyOpenGallery"
  | "hotkeyVisualClipboard"
  | "hotkeySaveReplay";

const HOTKEY_ROWS: {
  key: HotkeyFieldKey;
  label: string;
  desc: string;
  badge: string;
  defaultCombo: string;
}[] = [
  {
    key: "hotkeyRegionCapture",
    label: "Region Screenshot & Annotator",
    desc: "Freezes active screen before opening the Konva crop & annotation overlay",
    badge: "v1.0",
    defaultCombo: "Alt+S",
  },
  {
    key: "hotkeyFullscreenCapture",
    label: "Instant Fullscreen Capture (Zero Focus Loss)",
    desc: "Captures full monitor & copies to clipboard silently without Alt-Tabbing Exclusive Fullscreen games",
    badge: "v1.0",
    defaultCombo: "Alt+Shift+S",
  },
  {
    key: "hotkeyOpenGallery",
    label: "Open Virtualized Gallery",
    desc: "Opens/focuses the main SQLite + FTS5 media library window from background tray",
    badge: "v1.0",
    defaultCombo: "Alt+G",
  },
  {
    key: "hotkeyVisualClipboard",
    label: "Visual Image Clipboard Drawer",
    desc: "Opens the Windows+V style image ring buffer with blake3 deduplication",
    badge: "v1.5",
    defaultCombo: "Alt+V",
  },
  {
    key: "hotkeySaveReplay",
    label: "Save Rolling Replay Clip (Alt + C)",
    desc: "Stitches hardware-encoded 30s–20m segments via FFmpeg stream copy (avoids NVIDIA Alt+R conflict)",
    badge: "v2.0",
    defaultCombo: "Alt+C",
  },
];

export const SettingsPanel: React.FC = () => {
  const {
    settings,
    isSaving,
    ffmpegStatus,
    replaySnapshot,
    isDownloadingFfmpeg,
    downloadStageMsg,
    loadSettings,
    saveSettings,
    refreshReplayStatus,
    startFfmpegDownload,
  } = useSettingsStore();
  const { fetchCaptures } = useGalleryStore();

  const [form, setForm] = useState<SettingsPayload | null>(null);
  const [recordingField, setRecordingField] = useState<HotkeyFieldKey | null>(
    null
  );
  const [savedBanner, setSavedBanner] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string | null>(null);

  // Folder Change Choice Modal ("Move existing files" vs "Switch active folder view")
  const [pendingDirChange, setPendingDirChange] = useState<{
    oldDir: string;
    newDir: string;
  } | null>(null);

  // Import Pre-Existing Images Prompt Modal
  const [unindexedPrompt, setUnindexedPrompt] =
    useState<UnindexedScanResult | null>(null);
  const [isImporting, setIsImporting] = useState(false);

  // Storage Quota & OCR Status Preview
  const [storageStatus, setStorageStatus] =
    useState<StorageStatusSummary | null>(null);
  const [showDryRunModal, setShowDryRunModal] = useState(false);
  const [isRunningPolicy, setIsRunningPolicy] = useState(false);
  const [isRunningOcr, setIsRunningOcr] = useState(false);

  // Phase 2.0.4: Excluded Apps Blocklist Input
  const [newExcludedApp, setNewExcludedApp] = useState("");

  // Phase 2.0.1 & 2.0.2: Replay Buffer & FFmpeg Engine State (Global Store — persists across tabs)
  useEffect(() => {
    loadSettings();
    refreshReplayStatus(false);

    return () => {
      setShortcutRecordingMode(false).catch(() => {});
    };
  }, [loadSettings, refreshReplayStatus]);

  useEffect(() => {
    if (settings) {
      setForm({ ...settings });
    }
  }, [settings]);

  useEffect(() => {
    if (!form) return;
    getStorageStatusPreview(form.maxStorageGb, form.autoCleanupDays)
      .then(setStorageStatus)
      .catch(() => {});
  }, [form?.maxStorageGb, form?.autoCleanupDays]);

  const startRecordingKeybind = async (field: HotkeyFieldKey) => {
    await setShortcutRecordingMode(true);
    setRecordingField(field);
  };

  const stopRecordingKeybind = useCallback(async () => {
    setRecordingField(null);
    await setShortcutRecordingMode(false);
  }, []);

  // Applies a new keybind (or Default / Remove), automatically unbinds any conflicting action,
  // and immediately persists + binds the updated shortcuts in the OS.
  const applyHotkeyUpdate = useCallback(
    async (field: HotkeyFieldKey, rawCombo: string, actionNote?: string) => {
      if (!form) return;
      const combo = rawCombo.trim();
      const nextForm: SettingsPayload = { ...form, [field]: combo };
      const overwrittenLabels: string[] = [];

      if (combo !== "") {
        for (const row of HOTKEY_ROWS) {
          if (
            row.key !== field &&
            String(nextForm[row.key] || "")
              .trim()
              .toLowerCase() === combo.toLowerCase()
          ) {
            nextForm[row.key] = "";
            overwrittenLabels.push(row.label);
          }
        }
      }

      setForm(nextForm);
      setRecordingField(null);
      await saveSettings(nextForm);
      await setShortcutRecordingMode(false);

      const targetLabel =
        HOTKEY_ROWS.find((r) => r.key === field)?.label || field;
      if (overwrittenLabels.length > 0) {
        setStatusMessage(
          `Bound "${combo}" to "${targetLabel}" and removed duplicate keybind from ${overwrittenLabels
            .map((l) => `"${l}"`)
            .join(", ")}.`
        );
      } else if (actionNote) {
        setStatusMessage(actionNote);
      } else if (combo === "") {
        setStatusMessage(`Removed keybind for "${targetLabel}".`);
      } else {
        setStatusMessage(`Bound "${combo}" to "${targetLabel}" and activated shortcut.`);
      }
    },
    [form, saveSettings]
  );

  // Window-level keydown listener while recording a keybind so OS/browser focus never drops Alt+S
  useEffect(() => {
    if (!recordingField || !form) return;

    const onWindowKeyDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        stopRecordingKeybind();
        return;
      }

      const ignoreKeys = ["Control", "Alt", "Shift", "Meta"];
      if (ignoreKeys.includes(e.key)) return;

      const mods: string[] = [];
      if (e.ctrlKey || e.metaKey) mods.push("Ctrl");
      if (e.altKey) mods.push("Alt");
      if (e.shiftKey) mods.push("Shift");

      let mainKey = e.key.length === 1 ? e.key.toUpperCase() : e.key;
      if (mainKey === " ") mainKey = "Space";
      const combo = [...mods, mainKey].join("+");
      applyHotkeyUpdate(recordingField, combo);
    };

    window.addEventListener("keydown", onWindowKeyDown, true);
    return () => window.removeEventListener("keydown", onWindowKeyDown, true);
  }, [applyHotkeyUpdate, form, recordingField, stopRecordingKeybind]);

  if (!form) {
    return (
      <div className="p-8 flex items-center gap-3 text-sm text-base-content/70">
        <span className="loading loading-spinner loading-sm text-primary" />
        Loading settings...
      </div>
    );
  }

  const normalizeDir = (p: string) =>
    p.trim().replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

  const finalizeSaveAndScan = async (nextForm: SettingsPayload, msg?: string) => {
    await saveSettings(nextForm);
    await fetchCaptures();
    setSavedBanner(true);
    if (msg) setStatusMessage(msg);
    setTimeout(() => {
      setSavedBanner(false);
    }, 2200);

    // Check if the newly active working folder has pre-existing unindexed media
    const scan = await scanUnindexedMedia();
    if (scan.count > 0) {
      setUnindexedPrompt(scan);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const oldDir = settings?.capturesDir || "";
    const newDir = form.capturesDir.trim();

    if (oldDir && newDir && normalizeDir(oldDir) !== normalizeDir(newDir)) {
      setPendingDirChange({ oldDir, newDir });
      return;
    }

    await finalizeSaveAndScan(form);
  };

  const handleConfirmMoveExisting = async () => {
    if (!pendingDirChange) return;
    const { oldDir, newDir } = pendingDirChange;
    setPendingDirChange(null);

    const movedCount = await migrateCapturesToFolder(oldDir, newDir);
    await finalizeSaveAndScan(
      form,
      `Moved ${movedCount} existing capture(s) to "${newDir}".`
    );
  };

  const handleConfirmSwitchViewOnly = async () => {
    if (!pendingDirChange) return;
    const { newDir } = pendingDirChange;
    setPendingDirChange(null);

    await finalizeSaveAndScan(
      form,
      `Switched active working folder to "${newDir}" (previous folder files kept in place).`
    );
  };

  const handleConfirmImportExternal = async () => {
    setIsImporting(true);
    try {
      const imported = await importExternalMedia();
      await fetchCaptures();
      setStatusMessage(
        `Imported ${imported} existing image(s) into the Gallery with original timestamps!`
      );
    } finally {
      setIsImporting(false);
      setUnindexedPrompt(null);
    }
  };

  const handleBrowseCapturesDir = async () => {
    const picked = await pickFolderDialog(form.capturesDir);
    if (picked) {
      setForm({ ...form, capturesDir: picked });
    }
  };

  const handleResetToDefaultCapturesDir = async () => {
    const defaultDir = await getDefaultCapturesDir();
    if (defaultDir) {
      setForm({ ...form, capturesDir: defaultDir });
      setStatusMessage(
        `Set Storage Directory to built-in folder "${defaultDir}" — click "Save Changes" to apply.`
      );
    }
  };

  const handleRunStoragePolicyNow = async () => {
    setIsRunningPolicy(true);
    try {
      await saveSettings(form);
      const summary = await enforceStoragePoliciesNow();
      setStorageStatus(summary);
      await fetchCaptures();
      setShowDryRunModal(false);
      setStatusMessage(
        `Storage policy pass complete: ${summary.queuedCount} capture(s) now in 3-day Queued Deletion buffer, ${summary.expiredPurgedCount} expired capture(s) permanently deleted.`
      );
    } finally {
      setIsRunningPolicy(false);
    }
  };

  const handleRunOcrBackfill = async () => {
    setIsRunningOcr(true);
    try {
      const processed = await runOcrBackfillNow();
      const refreshed = await getStorageStatusPreview(
        form.maxStorageGb,
        form.autoCleanupDays
      );
      setStorageStatus(refreshed);
      await fetchCaptures();
      setStatusMessage(
        processed > 0
          ? `Native OS OCR scanned ${processed} capture(s) and indexed detected text into FTS5!`
          : "All captures in this folder have already been scanned by Native OS OCR."
      );
    } finally {
      setIsRunningOcr(false);
    }
  };

  const handleAddExcludedApp = () => {
    const clean = newExcludedApp.trim().toLowerCase();
    if (!clean) return;
    const current = form.clipboardExcludedApps || [];
    if (!current.includes(clean)) {
      setForm({
        ...form,
        clipboardExcludedApps: [...current, clean],
      });
    }
    setNewExcludedApp("");
  };

  const handleRemoveExcludedApp = (target: string) => {
    const current = form.clipboardExcludedApps || [];
    setForm({
      ...form,
      clipboardExcludedApps: current.filter((a) => a !== target),
    });
  };

  const canEnableReplayBuffer = Boolean(
    ffmpegStatus?.available &&
      !isDownloadingFfmpeg &&
      !(ffmpegStatus.source === "system" && !ffmpegStatus.supportsDdagrab)
  );

  const handleDownloadFfmpeg = async () => {
    const res = await startFfmpegDownload();
    if (res.error) {
      setStatusMessage(`FFmpeg Download Error: ${res.error}`);
    } else if (res.status) {
      setStatusMessage(
        `FFmpeg Replay Engine installed! Detected encoder: ${res.status.recommendedEncoder}`
      );
    }
  };

  const handleToggleReplayRecording = async () => {
    if (!canEnableReplayBuffer) {
      setStatusMessage(
        isDownloadingFfmpeg
          ? "Wait until the FFmpeg Engine download/upgrade finishes before enabling Replay Buffer."
          : "Download or Upgrade the FFmpeg Engine first before toggling Hardware Replay Buffer on."
      );
      return;
    }
    const next = !(form.replayEnabled ?? true);
    try {
      await setReplayBufferActive(next);
      setForm({ ...form, replayEnabled: next });
      setTimeout(() => {
        refreshReplayStatus(false).catch(() => {});
      }, 250);
    } catch (err) {
      setStatusMessage(String(err));
    }
  };

  const estimatedRingMb = Math.round(
    (((form.replayBitrateMbps ?? 20) * (form.replayBufferSecs ?? 60)) / 8)
  );

  return (
    <>
      <form
        onSubmit={handleSubmit}
        className="flex-1 overflow-y-auto px-8 py-6 max-w-4xl mx-auto w-full space-y-6"
      >
        <div className="flex items-center justify-between">
          <div>
            <h2 className="text-lg font-bold text-base-content">
              Application Settings & Keybinds
            </h2>
            <p className="text-xs text-base-content/60">
              Configure global hotkeys, working storage directory, capture formats, and storage quotas.
            </p>
          </div>

          <button
            type="submit"
            disabled={isSaving}
            className={`btn btn-sm gap-2 shadow ${
              savedBanner ? "btn-success" : "btn-primary"
            }`}
          >
            {savedBanner ? (
              <>
                <Check className="w-4 h-4" /> Saved!
              </>
            ) : (
              <>
                <Save className="w-4 h-4" /> Save Changes
              </>
            )}
          </button>
        </div>

        {statusMessage && (
          <div role="alert" className="alert alert-info alert-soft text-xs py-2.5">
            <Check className="w-4 h-4 shrink-0" />
            <span>{statusMessage}</span>
          </div>
        )}

        {/* Global Keybinds Section */}
        <div className="card bg-base-200 border border-base-300 shadow-sm">
          <div className="card-body p-5 space-y-3">
            <h3 className="card-title text-sm flex items-center gap-2">
              <Keyboard className="w-4 h-4 text-primary" />
              Global Shortcuts (Click any keybind to re-record)
            </h3>

            <div className="divide-y divide-base-300">
              {HOTKEY_ROWS.map((row) => {
                const isRec = recordingField === row.key;
                const currentCombo = String(form[row.key] || "").trim();
                const isUnbound = currentCombo === "";
                return (
                  <div
                    key={row.key}
                    className="py-3 flex items-center justify-between gap-4"
                  >
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-semibold text-base-content">
                          {row.label}
                        </span>
                        <span className="badge badge-soft badge-primary badge-xs font-mono">
                          {row.badge}
                        </span>
                        {currentCombo === row.defaultCombo && (
                          <span className="badge badge-ghost badge-xs font-mono opacity-75">
                            Default
                          </span>
                        )}
                      </div>
                      <p className="text-[11px] text-base-content/60 mt-0.5">
                        {row.desc}
                      </p>
                    </div>

                    <div className="flex items-center gap-1.5 shrink-0">
                      <button
                        type="button"
                        onClick={() =>
                          isRec
                            ? stopRecordingKeybind()
                            : startRecordingKeybind(row.key)
                        }
                        className={`btn btn-sm min-w-[155px] font-mono text-xs ${
                          isRec
                            ? "btn-primary animate-pulse"
                            : isUnbound
                            ? "btn-outline border-dashed border-base-300 bg-base-100 text-base-content/45 italic"
                            : "btn-outline border-base-300 bg-base-100"
                        }`}
                      >
                        {isRec
                          ? "Press keys... (Esc)"
                          : isUnbound
                          ? "Unbound (None)"
                          : currentCombo}
                      </button>

                      {/* Meatballs Dropdown Menu (Reset to Default / Remove Keybind) */}
                      <div className="dropdown dropdown-end">
                        <button
                          type="button"
                          tabIndex={0}
                          title="Keybind Options (Default / Remove)"
                          className="btn btn-sm btn-ghost btn-square border border-base-300 bg-base-100"
                        >
                          <MoreHorizontal className="w-4 h-4" />
                        </button>
                        <ul
                          tabIndex={0}
                          className="dropdown-content menu bg-base-100 rounded-box z-30 w-52 p-1.5 shadow-xl border border-base-300 text-xs"
                        >
                          <li>
                            <button
                              type="button"
                              onClick={(e) => {
                                (e.currentTarget as HTMLElement).blur();
                                applyHotkeyUpdate(
                                  row.key,
                                  row.defaultCombo,
                                  `Reset "${row.label}" to default (${row.defaultCombo}).`
                                );
                              }}
                              className="flex items-center justify-between gap-2 py-2"
                            >
                              <span className="flex items-center gap-2 font-medium">
                                <RotateCcw className="w-3.5 h-3.5 text-primary" />
                                Default
                              </span>
                              <span className="badge badge-ghost badge-xs font-mono">
                                {row.defaultCombo}
                              </span>
                            </button>
                          </li>
                          <li>
                            <button
                              type="button"
                              onClick={(e) => {
                                (e.currentTarget as HTMLElement).blur();
                                applyHotkeyUpdate(
                                  row.key,
                                  "",
                                  `Removed keybind for "${row.label}".`
                                );
                              }}
                              className="flex items-center gap-2 py-2 text-error hover:bg-error/10"
                            >
                              <Trash2 className="w-3.5 h-3.5" />
                              Remove Keybind
                            </button>
                          </li>
                        </ul>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>

        {/* Capture Format & Storage Quota Section */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <div className="card bg-base-200 border border-base-300 shadow-sm">
            <div className="card-body p-5 space-y-4">
              <h3 className="card-title text-sm flex items-center gap-2">
                <Folder className="w-4 h-4 text-success" />
                Capture Output & Working Storage Folder
              </h3>

              <div className="space-y-3 text-xs">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-base-content/80">Default Image Format</span>
                  <select
                    value={form.imageFormat}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        imageFormat: e.target.value as "webp" | "png",
                      })
                    }
                    className="select select-sm select-bordered w-48 text-xs"
                  >
                    <option value="webp">WebP (Fast, 40% smaller)</option>
                    <option value="png">PNG (Lossless RGBA)</option>
                  </select>
                </div>

                <label className="flex items-center justify-between cursor-pointer pt-1">
                  <span className="text-base-content/80">
                    Auto-copy to OS Clipboard on Save
                  </span>
                  <input
                    type="checkbox"
                    checked={form.copyToClipboardOnSave}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        copyToClipboardOnSave: e.target.checked,
                      })
                    }
                    className="toggle toggle-primary toggle-sm"
                  />
                </label>

                <div className="pt-2 border-t border-base-300 space-y-2">
                  <div className="flex items-center justify-between">
                    <span className="text-base-content/70 font-medium">
                      Storage Directory (Working Folder)
                    </span>
                    <div className="flex items-center gap-1">
                      <button
                        type="button"
                        onClick={handleResetToDefaultCapturesDir}
                        title="Reset to built-in ./captures folder next to the .exe"
                        className="btn btn-xs btn-ghost text-base-content/70 gap-1"
                      >
                        <RotateCcw className="w-3 h-3" />
                        Reset to Built-In
                      </button>
                      <button
                        type="button"
                        onClick={handleBrowseCapturesDir}
                        className="btn btn-xs btn-soft btn-primary gap-1"
                      >
                        <FolderSearch className="w-3 h-3" />
                        Browse...
                      </button>
                      <button
                        type="button"
                        onClick={() => openInFileExplorer(form.capturesDir)}
                        className="btn btn-xs btn-ghost text-base-content/70 gap-1"
                      >
                        <FolderOpen className="w-3 h-3" />
                        Open
                      </button>
                    </div>
                  </div>
                  <input
                    type="text"
                    value={form.capturesDir}
                    onChange={(e) =>
                      setForm({ ...form, capturesDir: e.target.value })
                    }
                    placeholder="C:\Users\...\Pictures\ReplayViewer\captures"
                    className="input input-sm input-bordered w-full bg-base-100 font-mono text-[11px]"
                  />
                  <p className="text-[10px] text-base-content/50">
                    When changing folders and clicking &ldquo;Save Changes&rdquo;, you&apos;ll be asked whether to move your existing captures or switch to a fresh folder view (and whether to import any pre-existing images in the target folder).
                  </p>
                </div>
              </div>
            </div>
          </div>

          <div className="card bg-base-200 border border-base-300 shadow-sm">
            <div className="card-body p-5 space-y-4">
              <h3 className="card-title text-sm flex items-center gap-2">
                <HardDrive className="w-4 h-4 text-warning" />
                Smart Storage, 3-Day Grace Buffer &amp; Native OCR (v1.5)
              </h3>

              <div className="space-y-3.5 text-xs">
                {/* Max Library Storage Cap */}
                <div>
                  <div className="flex justify-between mb-1">
                    <span className="text-base-content/80 font-medium">
                      Max Library Storage Cap (Default: 10 GB)
                    </span>
                    <span className="badge badge-warning badge-sm font-mono">
                      {form.maxStorageGb === 0
                        ? "Unlimited"
                        : `${form.maxStorageGb} GB`}
                    </span>
                  </div>
                  <input
                    type="range"
                    min={0}
                    max={100}
                    step={1}
                    value={form.maxStorageGb}
                    onChange={(e) =>
                      setForm({ ...form, maxStorageGb: Number(e.target.value) })
                    }
                    className="range range-warning range-xs w-full"
                  />
                  <p className="text-[10px] text-base-content/55 mt-1">
                    <strong className="text-warning">Pinned items are never pruned.</strong>{" "}
                    Pruned unpinned items are held in <strong>Queued Deletion</strong> for 3 days before permanent deletion.
                  </p>
                </div>

                {/* Max Retention Age (Unpinned) */}
                <div className="flex items-center justify-between gap-2 pt-1 border-t border-base-300">
                  <div>
                    <span className="text-base-content/80 font-medium block">
                      Max Retention Age (Unpinned)
                    </span>
                    <span className="text-[10px] text-base-content/55">
                      Default: No max retention age (Keep Forever)
                    </span>
                  </div>
                  <select
                    value={form.autoCleanupDays}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        autoCleanupDays: Number(e.target.value),
                      })
                    }
                    className="select select-xs select-bordered bg-base-100 font-mono text-[11px]"
                  >
                    <option value={0}>No Max Age (Keep Forever)</option>
                    <option value={7}>Older than 7 Days</option>
                    <option value={14}>Older than 14 Days</option>
                    <option value={30}>Older than 30 Days</option>
                    <option value={60}>Older than 60 Days</option>
                    <option value={90}>Older than 90 Days</option>
                    <option value={180}>Older than 180 Days</option>
                    <option value={365}>Older than 365 Days</option>
                  </select>
                </div>

                {/* Live Storage Breakdown & 3-Day Queued Deletion Status */}
                {storageStatus && (
                  <div className="p-2.5 rounded-lg bg-base-100 border border-base-300 space-y-2">
                    <div className="grid grid-cols-3 gap-2 text-[11px]">
                      <div>
                        <span className="text-warning font-semibold flex items-center gap-1">
                          <Pin className="w-3 h-3" /> Pinned (Immune)
                        </span>
                        <span className="font-mono text-base-content/80">
                          {storageStatus.pinnedCount} ({formatBytes(storageStatus.pinnedBytes)})
                        </span>
                      </div>
                      <div>
                        <span className="text-primary font-semibold block">
                          Unpinned Active
                        </span>
                        <span className="font-mono text-base-content/80">
                          {storageStatus.unpinnedCount} ({formatBytes(storageStatus.unpinnedBytes)})
                        </span>
                      </div>
                      <div>
                        <span className="text-error font-semibold flex items-center gap-1">
                          <Clock className="w-3 h-3" /> Queued (3d Grace)
                        </span>
                        <span className="font-mono text-base-content/80">
                          {storageStatus.queuedCount} ({formatBytes(storageStatus.queuedBytes)})
                        </span>
                      </div>
                    </div>

                    {storageStatus.nextExpiryAt && (
                      <div className="text-[10px] font-mono text-warning flex items-center justify-between pt-1 border-t border-base-300">
                        <span>Next permanent auto-deletion:</span>
                        <span>
                          {formatDeletionCountdown(storageStatus.nextExpiryAt).compactLabel}
                        </span>
                      </div>
                    )}

                    <div className="flex items-center justify-between gap-2 pt-1 border-t border-base-300">
                      <button
                        type="button"
                        onClick={() => setShowDryRunModal(true)}
                        className="btn btn-xs btn-outline border-base-300 gap-1 flex-1"
                      >
                        <Eye className="w-3 h-3 text-warning" />
                        Preview Cleanup ({storageStatus.candidateCount})
                      </button>
                      <button
                        type="button"
                        disabled={isRunningOcr}
                        onClick={handleRunOcrBackfill}
                        title="Scan unindexed captures using native OS OCR (Windows.Media.Ocr / Apple Vision)"
                        className="btn btn-xs btn-soft btn-info gap-1 flex-1"
                      >
                        <FileText className="w-3 h-3" />
                        {isRunningOcr
                          ? "Scanning OCR..."
                          : `Scan OCR (${storageStatus.pendingOcrCount} pending)`}
                      </button>
                    </div>
                  </div>
                )}

                {/* Visual Clipboard Ring Slots */}
                <div className="pt-1 border-t border-base-300">
                  <div className="flex justify-between mb-1">
                    <span className="text-base-content/80">
                      Visual Clipboard Ring Slots (Alt + V)
                    </span>
                    <span className="badge badge-primary badge-sm font-mono">
                      {form.clipboardMaxItems} images
                    </span>
                  </div>
                  <input
                    type="range"
                    min={5}
                    max={50}
                    step={5}
                    value={form.clipboardMaxItems}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        clipboardMaxItems: Number(e.target.value),
                      })
                    }
                    className="range range-primary range-xs w-full"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>

        {/* Phase 2.0.1 & 2.0.2: Hardware Replay Buffer & Dynamic Multi-Track Audio Router */}
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          {/* Milestone 2.0.1: Segmented GPU Replay Buffer */}
          <div className="card bg-base-200 border border-base-300 shadow-sm">
            <div className="card-body p-5 space-y-3.5 text-xs">
              <div className="flex items-center justify-between gap-2">
                <h3 className="card-title text-sm flex items-center gap-2">
                  <Film className="w-4 h-4 text-primary" />
                  Hardware Replay Buffer (v2.0.1)
                </h3>
                <div className="flex items-center gap-2">
                  <span
                    className={`badge badge-xs font-mono ${
                      replaySnapshot?.runtime.isRecording
                        ? "badge-error"
                        : replaySnapshot?.runtime.lastError
                        ? "badge-error"
                        : !ffmpegStatus?.available
                        ? "badge-warning"
                        : form.replayEnabled
                        ? "badge-info"
                        : "badge-ghost"
                    }`}
                  >
                    {replaySnapshot?.runtime.isRecording
                      ? `REC • ${replaySnapshot.runtime.activeEncoder}`
                      : isDownloadingFfmpeg
                      ? "Downloading..."
                      : replaySnapshot?.runtime.lastError
                      ? "Error"
                      : !ffmpegStatus?.available
                      ? "FFmpeg Missing"
                      : !canEnableReplayBuffer
                      ? "Upgrade Required"
                      : form.replayEnabled
                      ? "Starting..."
                      : "Paused"}
                  </span>
                  <input
                    type="checkbox"
                    disabled={!canEnableReplayBuffer}
                    checked={Boolean((form.replayEnabled ?? true) && canEnableReplayBuffer)}
                    onChange={handleToggleReplayRecording}
                    title={
                      isDownloadingFfmpeg
                        ? "Wait until FFmpeg Engine download/upgrade completes"
                        : !canEnableReplayBuffer
                        ? "Download or Upgrade FFmpeg Engine before enabling Replay Buffer"
                        : "Auto-start Replay Buffer (Prevents forgetting to toggle before gaming)"
                    }
                    className="toggle toggle-primary toggle-sm disabled:opacity-40"
                  />
                </div>
              </div>

              {/* FFmpeg Engine Resolver / One-Click Downloader */}
              <div className="p-3 rounded-lg bg-base-100 border border-base-300 space-y-2.5">
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0 flex-1">
                    <span className="font-semibold block">
                      FFmpeg Sidecar ({ffmpegStatus?.source || "checking"})
                    </span>
                    <span className="text-[10px] text-base-content/65 font-mono block">
                      {ffmpegStatus?.available
                        ? `${ffmpegStatus.recommendedEncoder} • ${
                            ffmpegStatus.supportsDdagrab
                              ? "ddagrab GPU (Zero-Copy, Anti-Cheat Safe)"
                              : "Legacy build (gdigrab fallback — upgrade for ddagrab)"
                          }`
                        : "Not detected — Download managed LGPL build (~45 MB) or browse for ffmpeg.exe"}
                    </span>
                  </div>
                  <div className="flex items-center gap-1.5 shrink-0">
                    <button
                      type="button"
                      onClick={() => refreshReplayStatus(true)}
                      title="Re-check PATH / custom FFmpeg binary"
                      className="btn btn-xs btn-ghost btn-square"
                    >
                      <RefreshCw className="w-3 h-3" />
                    </button>
                    {(!ffmpegStatus?.available ||
                      (ffmpegStatus?.source !== "managed" &&
                        !ffmpegStatus?.supportsDdagrab)) && (
                      <button
                        type="button"
                        disabled={isDownloadingFfmpeg}
                        onClick={handleDownloadFfmpeg}
                        className="btn btn-xs btn-primary gap-1"
                      >
                        <Download className="w-3 h-3" />
                        {isDownloadingFfmpeg
                          ? "Downloading..."
                          : ffmpegStatus?.available
                          ? "Upgrade Engine"
                          : "Download Engine"}
                      </button>
                    )}
                    {canEnableReplayBuffer && (
                      <button
                        type="button"
                        onClick={async () => {
                          const saved = await saveReplayBuffer();
                          await fetchCaptures();
                          setStatusMessage(
                            `Saved ${Math.round((saved.durationMs || 0) / 1000)}s Instant Replay clip!`
                          );
                        }}
                        className="btn btn-xs btn-soft btn-success gap-1"
                      >
                        <Film className="w-3 h-3" />
                        Save Clip Now
                      </button>
                    )}
                  </div>
                </div>

                <div className="space-y-1">
                  <label className="text-[10px] font-medium text-base-content/75 block">
                    Custom ffmpeg.exe Override Path
                  </label>
                  <div className="flex items-center gap-1.5">
                    <input
                      type="text"
                      value={form.ffmpegCustomPath ?? ""}
                      onChange={(e) =>
                        setForm({ ...form, ffmpegCustomPath: e.target.value })
                      }
                      placeholder="C:\ffmpeg\bin\ffmpeg.exe"
                      className="input input-xs input-bordered flex-1 bg-base-200 font-mono text-[10px]"
                    />
                    <button
                      type="button"
                      onClick={async () => {
                        const chosen = await pickFfmpegDialog();
                        if (chosen) {
                          setForm({ ...form, ffmpegCustomPath: chosen });
                        }
                      }}
                      className="btn btn-xs btn-outline border-base-300 gap-1 shrink-0"
                    >
                      <FolderOpen className="w-3 h-3" />
                      Browse...
                    </button>
                  </div>
                  <p className="text-[10px] text-base-content/55 leading-relaxed">
                    Leave empty to use ReplayViewer&apos;s one-click managed LGPL engine or system PATH.
                    {ffmpegStatus?.binaryPath && (
                      <span className="block font-mono text-base-content/70 break-all mt-0.5">
                        Active binary: {ffmpegStatus.binaryPath}
                      </span>
                    )}
                  </p>
                </div>
                {downloadStageMsg && isDownloadingFfmpeg && (
                  <p className="text-[10px] font-mono text-primary animate-pulse">
                    {downloadStageMsg}
                  </p>
                )}
              </div>

              {/* Rolling Buffer Window Presets (30s .. 20m) */}
              <div>
                <div className="flex justify-between mb-1">
                  <span className="text-base-content/80 font-medium">
                    Rolling Buffer Window (Default: 1m / 60s)
                  </span>
                  <span className="font-mono text-[11px] text-primary">
                    ~{estimatedRingMb} MB temp SSD ring
                  </span>
                </div>
                <div className="join w-full">
                  {[
                    { secs: 30, label: "30s" },
                    { secs: 60, label: "1m" },
                    { secs: 120, label: "2m" },
                    { secs: 300, label: "5m" },
                    { secs: 600, label: "10m" },
                    { secs: 1200, label: "20m" },
                  ].map((opt) => (
                    <button
                      key={opt.secs}
                      type="button"
                      onClick={() =>
                        setForm({ ...form, replayBufferSecs: opt.secs })
                      }
                      className={`btn btn-xs join-item flex-1 font-mono ${
                        (form.replayBufferSecs ?? 60) === opt.secs
                          ? "btn-primary"
                          : "btn-ghost bg-base-100"
                      }`}
                    >
                      {opt.label}
                    </button>
                  ))}
                </div>
              </div>

              <div className="grid grid-cols-3 gap-2 pt-1">
                <div>
                  <span className="text-[11px] text-base-content/70 block mb-1">
                    GPU Encoder
                  </span>
                  <select
                    value={form.replayEncoder ?? "auto"}
                    onChange={(e) =>
                      setForm({ ...form, replayEncoder: e.target.value })
                    }
                    className="select select-xs select-bordered w-full bg-base-100 font-mono text-[11px]"
                  >
                    <option value="auto">
                      Auto ({ffmpegStatus?.recommendedEncoder || "GPU"})
                    </option>
                    {(ffmpegStatus?.availableEncoders || []).map((enc) => (
                      <option key={enc} value={enc}>
                        {enc}
                      </option>
                    ))}
                  </select>
                </div>

                <div>
                  <span className="text-[11px] text-base-content/70 block mb-1">
                    Framerate
                  </span>
                  <select
                    value={form.replayFps ?? 60}
                    onChange={(e) =>
                      setForm({ ...form, replayFps: Number(e.target.value) })
                    }
                    className="select select-xs select-bordered w-full bg-base-100 font-mono text-[11px]"
                  >
                    <option value={30}>30 FPS</option>
                    <option value={60}>60 FPS</option>
                    <option value={120}>120 FPS</option>
                  </select>
                </div>

                <div>
                  <span className="text-[11px] text-base-content/70 block mb-1">
                    Bitrate ({form.replayBitrateMbps ?? 20} Mbps)
                  </span>
                  <input
                    type="range"
                    min={8}
                    max={50}
                    step={2}
                    value={form.replayBitrateMbps ?? 20}
                    onChange={(e) =>
                      setForm({
                        ...form,
                        replayBitrateMbps: Number(e.target.value),
                      })
                    }
                    className="range range-primary range-xs w-full mt-1.5"
                  />
                </div>
              </div>

              <p className="text-[10px] text-base-content/55">
                Alt-Tabbing or focusing other windows never resets the 2-second <code>.ts</code> ring.
                {replaySnapshot?.runtime.lastError && (
                  <span className="block text-error font-mono mt-1">
                    {replaySnapshot.runtime.lastError}
                  </span>
                )}
              </p>
            </div>
          </div>

          {/* Milestone 2.0.2: Dynamic Multi-Track Audio Mixer (Isolated Component, Zero Form Lag) */}
          <AudioMixerCard form={form} onFormChange={setForm} />
        </div>

        {/* Phase 2.0.4: Password Manager & Excluded Apps Clipboard Privacy */}
        <div className="card bg-base-200 border border-base-300 shadow-sm">
          <div className="card-body p-5 space-y-3.5 text-xs">
            <div className="flex items-center justify-between">
              <h3 className="card-title text-sm flex items-center gap-2">
                <Lock className="w-4 h-4 text-success" />
                Clipboard Privacy &amp; Excluded Apps (v2.0.4)
              </h3>
              <span className="badge badge-soft badge-success badge-xs font-mono">
                OS Flags Active
              </span>
            </div>

            <p className="text-[11px] text-base-content/70 leading-relaxed">
              Automatically respects Windows privacy formats (
              <code className="font-mono text-[10px]">
                ExcludeClipboardContentFromMonitorProcessing
              </code>{" "}
              &amp;{" "}
              <code className="font-mono text-[10px]">
                CanIncludeInClipboardHistory
              </code>
              ) and skips any copy when the active window title or process matches your blocklist below:
            </p>

            <div className="flex flex-wrap gap-1.5 p-2.5 rounded-lg bg-base-100 border border-base-300 max-h-28 overflow-y-auto">
              {(form.clipboardExcludedApps || []).map((rule) => (
                <span
                  key={rule}
                  className="badge badge-soft badge-neutral badge-sm gap-1 font-mono text-[11px]"
                >
                  {rule}
                  <button
                    type="button"
                    onClick={() => handleRemoveExcludedApp(rule)}
                    title={`Remove "${rule}" from blocklist`}
                    className="hover:text-error"
                  >
                    <X className="w-3 h-3" />
                  </button>
                </span>
              ))}
            </div>

            <div className="join w-full">
              <input
                type="text"
                value={newExcludedApp}
                onChange={(e) => setNewExcludedApp(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    handleAddExcludedApp();
                  }
                }}
                placeholder="Add app .exe or window keyword (e.g. signal, bank, keepass)..."
                className="input input-xs input-bordered join-item flex-1 bg-base-100 font-mono text-[11px] h-7"
              />
              <button
                type="button"
                onClick={handleAddExcludedApp}
                className="btn btn-xs btn-primary join-item h-7 gap-1"
              >
                <Plus className="w-3 h-3" />
                Add Rule
              </button>
            </div>
            <p className="text-[10px] text-base-content/55">
              Tip: You can also pause/resume clipboard history anytime from the System Tray menu or the{" "}
              <kbd className="kbd kbd-xs">Alt+V</kbd> drawer header.
            </p>
          </div>
        </div>

        {/* Anti-Cheat & MIT-Safe On-Demand FFmpeg Architecture Alert */}
        <div
          role="alert"
          className="alert alert-soft alert-success border border-success/30"
        >
          <ShieldCheck className="w-5 h-5 shrink-0" />
          <div className="text-xs">
            <div className="font-bold">
              Anti-Cheat Safe Compositor Mode Active (Zero DLL Injection)
            </div>
            <div className="opacity-80 mt-0.5">
              Phase 2.0 Hardware Replay Buffer uses an on-demand LGPL FFmpeg sidecar (NVENC / AMF / QuickSync) to keep the base installer &lt; 6 MB and 100% MIT-licensed.
            </div>
          </div>
          <span className="badge badge-neutral font-mono text-[11px] gap-1.5 py-3 shrink-0">
            <Cpu className="w-3.5 h-3.5 text-primary" />
            Phase 2.0 Ready
          </span>
        </div>
      </form>

      {/* Modal 1: Working Directory Change Confirmation (Move vs Switch View) */}
      {pendingDirChange && (
        <dialog
          className="modal modal-open"
          onClick={() => setPendingDirChange(null)}
        >
          <div
            className="modal-box max-w-lg bg-base-200 border border-base-300 space-y-4"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="font-bold text-base flex items-center gap-2">
              <FolderSync className="w-5 h-5 text-primary" />
              Working Storage Directory Changed
            </h3>

            <div className="text-xs space-y-2 text-base-content/80">
              <p>
                You are changing the active working directory from:
              </p>
              <div className="p-2 rounded bg-base-100 border border-base-300 font-mono text-[11px] break-all">
                {pendingDirChange.oldDir}
              </div>
              <p>to:</p>
              <div className="p-2 rounded bg-base-100 border border-primary/40 font-mono text-[11px] text-primary break-all">
                {pendingDirChange.newDir}
              </div>
            </div>

            <div className="space-y-2 pt-1">
              <button
                type="button"
                onClick={handleConfirmMoveExisting}
                className="btn btn-sm btn-primary w-full justify-start gap-2.5 h-auto py-2.5 text-left"
              >
                <FolderInput className="w-4 h-4 shrink-0" />
                <div>
                  <div className="font-bold text-xs">
                    Move Existing Captures to New Folder (Recommended)
                  </div>
                  <div className="text-[10px] opacity-80 font-normal">
                    Moves all files &amp; thumbnails to the new folder and keeps all tags, pins, and groups intact.
                  </div>
                </div>
              </button>

              <button
                type="button"
                onClick={handleConfirmSwitchViewOnly}
                className="btn btn-sm btn-neutral w-full justify-start gap-2.5 h-auto py-2.5 text-left"
              >
                <Folder className="w-4 h-4 shrink-0 text-secondary" />
                <div>
                  <div className="font-bold text-xs">
                    Switch Active Folder View (Keep Old Files Where They Are)
                  </div>
                  <div className="text-[10px] opacity-80 font-normal">
                    Scopes the Gallery to only show files in the new folder. Old files stay untouched on disk (and reappear with their tags if you switch back later).
                  </div>
                </div>
              </button>
            </div>

            <div className="modal-action mt-2">
              <button
                type="button"
                onClick={() => setPendingDirChange(null)}
                className="btn btn-xs btn-ghost"
              >
                Cancel
              </button>
            </div>
          </div>
        </dialog>
      )}

      {/* Modal 2: Ask Before Importing Pre-Existing Media in Target Folder */}
      {unindexedPrompt && (
        <dialog
          className="modal modal-open"
          onClick={() => setUnindexedPrompt(null)}
        >
          <div
            className="modal-box max-w-md bg-base-200 border border-base-300 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="font-bold text-sm flex items-center gap-2">
              <ImagePlus className="w-4 h-4 text-success" />
              Found {unindexedPrompt.count} Existing Image(s) in Folder
            </h3>

            <p className="text-xs text-base-content/80 leading-relaxed">
              We found{" "}
              <span className="font-bold text-base-content">
                {unindexedPrompt.count} unindexed image(s)
              </span>{" "}
              ({formatBytes(unindexedPrompt.totalBytes)}) inside{" "}
              <span className="font-mono text-[11px]">
                {unindexedPrompt.folderPath}
              </span>
              . Would you like to import them into the Gallery?
            </p>

            {unindexedPrompt.sampleNames.length > 0 && (
              <div className="p-2.5 rounded-lg bg-base-100 border border-base-300 text-[11px] font-mono text-base-content/70 space-y-0.5">
                {unindexedPrompt.sampleNames.map((name) => (
                  <div key={name} className="truncate">
                    • {name}
                  </div>
                ))}
                {unindexedPrompt.count > unindexedPrompt.sampleNames.length && (
                  <div className="text-base-content/50 italic">
                    + {unindexedPrompt.count - unindexedPrompt.sampleNames.length}{" "}
                    more...
                  </div>
                )}
              </div>
            )}

            <p className="text-[11px] text-base-content/60">
              Original file modification dates will be preserved for accurate timeline sorting, and fast 320px WebP thumbnails will be generated.
            </p>

            <div className="modal-action">
              <button
                type="button"
                disabled={isImporting}
                onClick={() => setUnindexedPrompt(null)}
                className="btn btn-xs btn-ghost"
              >
                Skip for Now
              </button>
              <button
                type="button"
                disabled={isImporting}
                onClick={handleConfirmImportExternal}
                className="btn btn-xs btn-success gap-1.5"
              >
                {isImporting ? (
                  <>
                    <span className="loading loading-spinner loading-xs" />
                    Importing...
                  </>
                ) : (
                  <>
                    <ImagePlus className="w-3.5 h-3.5" />
                    Import {unindexedPrompt.count} Image(s)
                  </>
                )}
              </button>
            </div>
          </div>
        </dialog>
      )}

      {/* Modal 3: Dry-Run Storage Policy Preview Modal */}
      {showDryRunModal && storageStatus && (
        <dialog
          className="modal modal-open"
          onClick={() => setShowDryRunModal(false)}
        >
          <div
            className="modal-box max-w-lg bg-base-200 border border-base-300 space-y-3"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="font-bold text-sm flex items-center gap-2">
              <Clock className="w-4 h-4 text-warning" />
              Storage Policy Preview (3-Day Queued Deletion Buffer)
            </h3>

            <div className="text-xs text-base-content/80 space-y-2">
              <p>
                With a cap of{" "}
                <strong>
                  {form.maxStorageGb === 0 ? "Unlimited" : `${form.maxStorageGb} GB`}
                </strong>{" "}
                and retention age of{" "}
                <strong>
                  {form.autoCleanupDays === 0
                    ? "Keep Forever"
                    : `${form.autoCleanupDays} days`}
                </strong>
                :
              </p>

              <div className="p-3 rounded-lg bg-base-100 border border-base-300 space-y-1.5 font-mono text-[11px]">
                <div className="flex justify-between">
                  <span>Pinned Captures (100% Immune):</span>
                  <span className="text-warning font-bold">
                    {storageStatus.pinnedCount} items ({formatBytes(storageStatus.pinnedBytes)})
                  </span>
                </div>
                <div className="flex justify-between">
                  <span>Unpinned Active Captures:</span>
                  <span>
                    {storageStatus.unpinnedCount} items ({formatBytes(storageStatus.unpinnedBytes)})
                  </span>
                </div>
                <div className="flex justify-between">
                  <span>Currently in 3-Day Queued Deletion:</span>
                  <span className="text-error">
                    {storageStatus.queuedCount} items ({formatBytes(storageStatus.queuedBytes)})
                  </span>
                </div>
                <div className="flex justify-between pt-1 border-t border-base-300">
                  <span>Would Move to 3-Day Queue Now:</span>
                  <span className="text-primary font-bold">
                    {storageStatus.candidateCount} items ({formatBytes(storageStatus.candidateBytes)})
                  </span>
                </div>
              </div>

              {storageStatus.candidateSamples.length > 0 && (
                <div className="space-y-1">
                  <span className="text-[11px] font-semibold text-base-content/70">
                    Oldest unpinned captures that would enter the 3-day grace buffer:
                  </span>
                  <div className="p-2 rounded bg-base-100 border border-base-300 max-h-36 overflow-y-auto space-y-1 font-mono text-[10px]">
                    {storageStatus.candidateSamples.map((c) => (
                      <div key={c.id} className="flex items-center justify-between gap-2">
                        <span className="truncate">{c.fileName}</span>
                        <span className="text-warning shrink-0">
                          {c.reason} • {formatBytes(c.sizeBytes)}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>

            <div className="modal-action">
              <button
                type="button"
                onClick={() => setShowDryRunModal(false)}
                className="btn btn-xs btn-ghost"
              >
                Close
              </button>
              <button
                type="button"
                disabled={isRunningPolicy}
                onClick={handleRunStoragePolicyNow}
                className="btn btn-xs btn-warning gap-1.5"
              >
                <Sparkles className="w-3.5 h-3.5" />
                {isRunningPolicy
                  ? "Applying Policy..."
                  : "Apply & Run Policy Pass Now"}
              </button>
            </div>
          </div>
        </dialog>
      )}
    </>
  );
};
