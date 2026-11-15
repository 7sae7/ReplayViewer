import React, { useEffect, useState } from "react";
import {
  Check,
  Cpu,
  Folder,
  FolderInput,
  FolderOpen,
  FolderSearch,
  FolderSync,
  HardDrive,
  ImagePlus,
  Keyboard,
  Save,
  ShieldCheck,
} from "lucide-react";
import { formatBytes } from "../../lib/dpi";
import {
  importExternalMedia,
  migrateCapturesToFolder,
  openInFileExplorer,
  pickFolderDialog,
  scanUnindexedMedia,
} from "../../lib/tauri-client";
import { useGalleryStore } from "../../stores/useGalleryStore";
import { useSettingsStore } from "../../stores/useSettingsStore";
import type { SettingsPayload, UnindexedScanResult } from "../../types/ipc";

export const SettingsPanel: React.FC = () => {
  const { settings, isSaving, loadSettings, saveSettings } = useSettingsStore();
  const { fetchCaptures } = useGalleryStore();

  const [form, setForm] = useState<SettingsPayload | null>(null);
  const [recordingField, setRecordingField] = useState<keyof SettingsPayload | null>(null);
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

  useEffect(() => {
    loadSettings();
  }, [loadSettings]);

  useEffect(() => {
    if (settings) {
      setForm({ ...settings });
    }
  }, [settings]);

  const handleKeyDownRecorder = (
    e: React.KeyboardEvent,
    field: keyof SettingsPayload
  ) => {
    if (recordingField !== field || !form) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      setRecordingField(null);
      return;
    }

    const mods: string[] = [];
    if (e.ctrlKey || e.metaKey) mods.push("Ctrl");
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");

    const ignoreKeys = ["Control", "Alt", "Shift", "Meta"];
    if (ignoreKeys.includes(e.key)) return;

    const mainKey = e.key.length === 1 ? e.key.toUpperCase() : e.key;
    const combo = [...mods, mainKey].join("+");
    setForm({ ...form, [field]: combo });
    setRecordingField(null);
  };

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

  const hotkeyRows: {
    key: keyof SettingsPayload;
    label: string;
    desc: string;
    badge: string;
  }[] = [
    {
      key: "hotkeyRegionCapture",
      label: "Region Screenshot & Annotator",
      desc: "Freezes active screen before opening the Konva crop & annotation overlay",
      badge: "v1.0",
    },
    {
      key: "hotkeyFullscreenCapture",
      label: "Instant Fullscreen Capture (Zero Focus Loss)",
      desc: "Captures full monitor & copies to clipboard silently without Alt-Tabbing Exclusive Fullscreen games",
      badge: "v1.0",
    },
    {
      key: "hotkeyOpenGallery",
      label: "Open Virtualized Gallery",
      desc: "Opens/focuses the main SQLite + FTS5 media library window from background tray",
      badge: "v1.0",
    },
    {
      key: "hotkeyVisualClipboard",
      label: "Visual Image Clipboard Drawer",
      desc: "Opens the Windows+V style image ring buffer with blake3 deduplication",
      badge: "v1.5",
    },
    {
      key: "hotkeySaveReplay",
      label: "Save Rolling Replay Clip (Alt + C)",
      desc: "Stitches hardware-encoded 30s–20m segments via FFmpeg stream copy (avoids NVIDIA Alt+R conflict)",
      badge: "v2.0",
    },
  ];

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
              {hotkeyRows.map((row) => {
                const isRec = recordingField === row.key;
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
                      </div>
                      <p className="text-[11px] text-base-content/60 mt-0.5">
                        {row.desc}
                      </p>
                    </div>

                    <button
                      type="button"
                      onClick={() => setRecordingField(row.key)}
                      onKeyDown={(e) => handleKeyDownRecorder(e, row.key)}
                      className={`btn btn-sm min-w-[140px] font-mono text-xs ${
                        isRec
                          ? "btn-primary animate-pulse"
                          : "btn-outline border-base-300 bg-base-100"
                      }`}
                    >
                      {isRec ? "Press keys..." : String(form[row.key])}
                    </button>
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
                Smart Storage & Clipboard Ring (v1.5)
              </h3>

              <div className="space-y-4 text-xs">
                <div>
                  <div className="flex justify-between mb-1.5">
                    <span className="text-base-content/80">
                      Max Library Storage Cap
                    </span>
                    <span className="badge badge-warning badge-sm font-mono">
                      {form.maxStorageGb} GB
                    </span>
                  </div>
                  <input
                    type="range"
                    min={5}
                    max={100}
                    step={5}
                    value={form.maxStorageGb}
                    onChange={(e) =>
                      setForm({ ...form, maxStorageGb: Number(e.target.value) })
                    }
                    className="range range-warning range-xs w-full"
                  />
                </div>

                <div>
                  <div className="flex justify-between mb-1.5">
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
    </>
  );
};
