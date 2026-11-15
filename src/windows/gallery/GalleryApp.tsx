import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ArrowDownWideNarrow,
  ArrowUpNarrowWide,
  Camera,
  CheckSquare,
  ClipboardCheck,
  Columns,
  Copy,
  Database,
  FolderInput,
  FolderOpen,
  FolderPlus,
  Grid,
  ImagePlus,
  Minus,
  Monitor,
  Palette,
  Pin,
  RefreshCw,
  Search,
  Settings,
  Sparkles,
  Square,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import { formatBytes } from "../../lib/dpi";
import {
  importExternalMedia,
  isTauriRuntime,
  openInFileExplorer,
  pickFolderDialog,
  scanUnindexedMedia,
  toggleClipboardDrawer,
  triggerFullscreenCapture,
  triggerRegionCapture,
} from "../../lib/tauri-client";
import { useGalleryStore } from "../../stores/useGalleryStore";
import { useSettingsStore } from "../../stores/useSettingsStore";
import type {
  DateRangeFilter,
  MediaType,
  SortByField,
  UnindexedScanResult,
} from "../../types/ipc";
import { MediaLightbox } from "./MediaLightbox";
import { SettingsPanel } from "./SettingsPanel";
import { VirtualizedGrid } from "./VirtualizedGrid";

interface GalleryAppProps {
  onOpenOverlayPreview?: () => void;
  onOpenClipboardPreview?: () => void;
}

const DAISY_THEMES = [
  "night",
  "dark",
  "dim",
  "dracula",
  "synthwave",
  "cyberpunk",
  "sunset",
  "business",
  "nord",
  "light",
] as const;

export const GalleryApp: React.FC<GalleryAppProps> = ({
  onOpenOverlayPreview,
  onOpenClipboardPreview,
}) => {
  const {
    items,
    groups,
    totalCount,
    totalBytes,
    searchQuery,
    mediaTypeFilter,
    onlyPinned,
    onlyRecentClipboard,
    activeGroup,
    dateRange,
    sortBy,
    sortDir,
    columnCount,
    selectedIds,
    selectedCapture,
    lastReconciledCount,
    fetchCaptures,
    setSearchQuery,
    setMediaTypeFilter,
    setOnlyPinned,
    setOnlyRecentClipboard,
    setActiveGroup,
    setDateRange,
    setSortBy,
    toggleSortDir,
    setColumnCount,
    setSelectedIds,
    toggleSelectId,
    selectAllVisible,
    clearSelection,
    batchTagSelected,
    batchGroupSelected,
    batchMoveSelectedToFolder,
    setSelectedCapture,
    togglePin,
    removeCaptures,
    addTag,
    reconcileOrphans,
    runBenchmarkSeed,
  } = useGalleryStore();

  const { settings, loadSettings } = useSettingsStore();

  const [activeTab, setActiveTab] = useState<"gallery" | "settings">("gallery");
  const [statusToast, setStatusToast] = useState<string | null>(null);
  const [isMaximized, setIsMaximized] = useState(false);

  // Batch Action Popover / Modal states
  const [batchModal, setBatchModal] = useState<
    "tag" | "group" | "move" | null
  >(null);
  const [batchInputText, setBatchInputText] = useState("");

  // Unindexed External Media Prompt (when clicking Sync DB or switching folders)
  const [unindexedPrompt, setUnindexedPrompt] =
    useState<UnindexedScanResult | null>(null);
  const [isImportingExternal, setIsImportingExternal] = useState(false);

  const [theme, setTheme] = useState<string>(() => {
    return localStorage.getItem("rv_daisy_theme") || "night";
  });

  const handleThemeChange = (newTheme: string) => {
    setTheme(newTheme);
    localStorage.setItem("rv_daisy_theme", newTheme);
    document.documentElement.setAttribute("data-theme", newTheme);
  };

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  const showToast = (msg: string) => {
    setStatusToast(msg);
    setTimeout(() => setStatusToast(null), 3000);
  };

  useEffect(() => {
    fetchCaptures();
    loadSettings();

    let unlistenGallery: (() => void) | undefined;
    let unlistenToast: (() => void) | undefined;

    if (isTauriRuntime()) {
      const win = getCurrentWindow();
      win.isMaximized().then(setIsMaximized).catch(() => {});

      listen("gallery:updated", () => {
        fetchCaptures();
        showToast("New capture saved to library & copied to clipboard!");
      }).then((fn) => {
        unlistenGallery = fn;
      });

      listen<string>("shortcut:toast", (event) => {
        showToast(event.payload);
      }).then((fn) => {
        unlistenToast = fn;
      });
    }
    return () => {
      if (unlistenGallery) unlistenGallery();
      if (unlistenToast) unlistenToast();
    };
  }, [fetchCaptures, loadSettings]);

  // Escape key clears active multi-selection
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && selectedIds.length > 0 && !selectedCapture) {
        clearSelection();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedIds.length, selectedCapture, clearSelection]);

  // Custom Frameless daisyUI Window Controls
  const handleTitlebarMouseDown = async (e: React.MouseEvent) => {
    if (e.button !== 0 || !isTauriRuntime()) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, label, a, [role='tablist']")) {
      return;
    }
    const win = getCurrentWindow();
    if (e.detail === 2) {
      await win.toggleMaximize();
      const max = await win.isMaximized();
      setIsMaximized(max);
    } else {
      await win.startDragging();
    }
  };

  const handleMinimize = async () => {
    if (isTauriRuntime()) {
      await getCurrentWindow().minimize();
    }
  };

  const handleToggleMaximize = async () => {
    if (isTauriRuntime()) {
      const win = getCurrentWindow();
      await win.toggleMaximize();
      const max = await win.isMaximized();
      setIsMaximized(max);
    }
  };

  const handleCloseWindow = async () => {
    if (isTauriRuntime()) {
      await getCurrentWindow().close();
    }
  };

  const handleRegionCapture = async () => {
    if (isTauriRuntime()) {
      const win = getCurrentWindow();
      await win.hide();
      await new Promise((r) => setTimeout(r, 120));
      await triggerRegionCapture();
    } else {
      await triggerRegionCapture();
      if (onOpenOverlayPreview) onOpenOverlayPreview();
    }
  };

  const handleFullscreenCapture = async () => {
    await triggerFullscreenCapture();
    await fetchCaptures();
    showToast("Instant Fullscreen Capture saved & copied (Zero Focus Loss)!");
  };

  const handleOpenClipboardDrawer = async () => {
    if (isTauriRuntime()) {
      await toggleClipboardDrawer();
    } else if (onOpenClipboardPreview) {
      onOpenClipboardPreview();
    }
  };

  const handleReconcile = async () => {
    const pruned = await reconcileOrphans();
    const scan = await scanUnindexedMedia();
    if (scan.count > 0) {
      setUnindexedPrompt(scan);
    } else {
      showToast(
        pruned > 0
          ? `Pruned ${pruned} orphaned database row(s) missing from disk.`
          : "Folder synced: 0 orphaned rows, 0 unindexed files."
      );
    }
  };

  const handleConfirmImportExternal = async () => {
    setIsImportingExternal(true);
    try {
      const imported = await importExternalMedia();
      await fetchCaptures();
      showToast(
        `Imported ${imported} existing image(s) into the Gallery with original dates!`
      );
    } finally {
      setIsImportingExternal(false);
      setUnindexedPrompt(null);
    }
  };

  const handleSeedBenchmark = async () => {
    await runBenchmarkSeed(500);
    showToast("Inserted 500 benchmark captures into SQLite — scroll & sort at 60 FPS!");
  };

  const handleOpenCapturesFolder = async () => {
    if (settings?.capturesDir) {
      await openInFileExplorer(settings.capturesDir);
    }
  };

  // Batch Multi-Select Handlers
  const handleBatchDelete = async () => {
    const count = selectedIds.length;
    if (count === 0) return;
    await removeCaptures(selectedIds);
    showToast(`Deleted ${count} selected capture(s) from disk & SQLite.`);
  };

  const handleBatchSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!batchModal) return;

    if (batchModal === "tag") {
      const tags = batchInputText
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
      if (tags.length === 0) return;
      const updated = await batchTagSelected(tags);
      showToast(`Added tag(s) to ${updated} selected capture(s).`);
    } else if (batchModal === "group") {
      const updated = await batchGroupSelected(batchInputText.trim());
      showToast(
        batchInputText.trim()
          ? `Grouped ${updated} capture(s) into "${batchInputText.trim()}".`
          : `Removed group from ${updated} capture(s).`
      );
    } else if (batchModal === "move") {
      const target = batchInputText.trim();
      if (!target) return;
      const moved = await batchMoveSelectedToFolder(target);
      showToast(
        `Moved ${moved} capture(s) to "${target}" (removed from working gallery).`
      );
    }

    setBatchModal(null);
    setBatchInputText("");
  };

  const handleOpenMoveModal = async () => {
    const picked = await pickFolderDialog(settings?.capturesDir);
    if (picked) {
      setBatchInputText(picked);
    } else {
      setBatchInputText("");
    }
    setBatchModal("move");
  };

  return (
    <div className="flex flex-col w-screen h-screen bg-base-100 text-base-content overflow-hidden select-none border border-base-300">
      {/* Custom Frameless daisyUI Titlebar + Command Bar */}
      <header
        data-tauri-drag-region
        onMouseDown={handleTitlebarMouseDown}
        className="navbar bg-base-200 border-b border-base-300 px-4 min-h-13 gap-2 flex-wrap cursor-default"
      >
        {/* Brand & Stats (Draggable) */}
        <div className="flex items-center gap-2.5 pointer-events-none">
          <div className="w-8 h-8 rounded-lg bg-primary text-primary-content flex items-center justify-center shadow-sm">
            <Camera className="w-4 h-4" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h1 className="text-xs font-bold tracking-tight">
                ReplayViewer
              </h1>
              <span className="badge badge-soft badge-primary badge-xs font-mono font-semibold">
                v1.5.1
              </span>
            </div>
            <p className="text-[10px] text-base-content/60 font-mono">
              {totalCount.toLocaleString()} items • {formatBytes(totalBytes)} • SQLite WAL + FTS5
            </p>
          </div>
        </div>

        {/* Instant FTS5 Search Input */}
        {activeTab === "gallery" && (
          <div className="flex-1 max-w-md min-w-[200px] mx-auto">
            <label className="input input-xs input-bordered flex items-center gap-2 w-full bg-base-100 h-8">
              <Search className="w-3.5 h-3.5 opacity-60" />
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder="Instant FTS5 search by app, #tag, group, or OCR text..."
                className="grow text-xs"
              />
              {searchQuery && (
                <button
                  type="button"
                  onClick={() => setSearchQuery("")}
                  className="btn btn-ghost btn-xs btn-circle h-5 w-5 min-h-0"
                >
                  <X className="w-3 h-3" />
                </button>
              )}
            </label>
          </div>
        )}

        {/* Quick Capture, Navigation Tabs, Theme Selector & Custom Window Controls */}
        <div className="flex items-center gap-1.5 ml-auto">
          <button
            type="button"
            onClick={handleRegionCapture}
            title="Freeze Screen & Open Region Annotator (Alt + S)"
            className="btn btn-xs btn-primary h-7 px-2.5 gap-1"
          >
            <Camera className="w-3.5 h-3.5" />
            Region (Alt+S)
          </button>

          <button
            type="button"
            onClick={handleFullscreenCapture}
            title="Instant Fullscreen Capture without Alt-Tabbing games (Alt + Shift + S)"
            className="btn btn-xs btn-neutral h-7 px-2.5 gap-1"
          >
            <Monitor className="w-3.5 h-3.5 text-success" />
            Fullscreen
          </button>

          <button
            type="button"
            onClick={handleOpenClipboardDrawer}
            title="Open Visual Image Clipboard Drawer (Alt + V)"
            className="btn btn-xs btn-neutral h-7 px-2.5 gap-1"
          >
            <ClipboardCheck className="w-3.5 h-3.5 text-info" />
            Clipboard (Alt+V)
          </button>

          {/* daisyUI Boxed Tabs */}
          <div role="tablist" className="tabs tabs-box tabs-xs bg-base-300 p-0.5 ml-1">
            <button
              type="button"
              role="tab"
              onClick={() => setActiveTab("gallery")}
              className={`tab gap-1 font-semibold ${
                activeTab === "gallery" ? "tab-active" : ""
              }`}
            >
              <Grid className="w-3 h-3" />
              Gallery
            </button>
            <button
              type="button"
              role="tab"
              onClick={() => setActiveTab("settings")}
              className={`tab gap-1 font-semibold ${
                activeTab === "settings" ? "tab-active" : ""
              }`}
            >
              <Settings className="w-3 h-3" />
              Settings
            </button>
          </div>

          {/* daisyUI Theme Switcher */}
          <div className="flex items-center gap-1" title="Switch daisyUI Theme">
            <Palette className="w-3.5 h-3.5 text-primary ml-1" />
            <select
              value={theme}
              onChange={(e) => handleThemeChange(e.target.value)}
              className="select select-xs select-bordered bg-base-100 font-mono text-[11px] capitalize h-7 min-h-0"
            >
              {DAISY_THEMES.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </select>
          </div>

          <div className="divider divider-horizontal mx-0.5 my-1.5" />

          {/* Custom daisyUI Window Controls (Minimize, Maximize/Restore, Close) */}
          <div className="join">
            <button
              type="button"
              onClick={handleMinimize}
              title="Minimize Window"
              className="btn btn-xs btn-ghost join-item h-7 w-8 p-0"
            >
              <Minus className="w-3.5 h-3.5" />
            </button>
            <button
              type="button"
              onClick={handleToggleMaximize}
              title={isMaximized ? "Restore Down" : "Maximize Window"}
              className="btn btn-xs btn-ghost join-item h-7 w-8 p-0"
            >
              {isMaximized ? (
                <Copy className="w-3 h-3 rotate-180" />
              ) : (
                <Square className="w-3 h-3" />
              )}
            </button>
            <button
              type="button"
              onClick={handleCloseWindow}
              title="Close to System Tray (Alt + G to reopen)"
              className="btn btn-xs btn-ghost hover:btn-error hover:text-error-content join-item h-7 w-8 p-0"
            >
              <X className="w-3.5 h-3.5" />
            </button>
          </div>
        </div>
      </header>

      {/* Secondary Filter, B-Tree Sort, Column Resizer & Maintenance Toolbar */}
      {activeTab === "gallery" && (
        <div className="flex flex-wrap items-center justify-between gap-2 px-5 py-2 bg-base-200/60 border-b border-base-300 text-xs">
          {/* Left: Filter Pills + Group + Media Type + Date Range */}
          <div className="flex flex-wrap items-center gap-1.5">
            <div className="join">
              <button
                type="button"
                onClick={() => {
                  setOnlyPinned(false);
                  setOnlyRecentClipboard(false);
                }}
                className={`btn btn-xs join-item ${
                  !onlyPinned && !onlyRecentClipboard
                    ? "btn-primary"
                    : "btn-ghost bg-base-100"
                }`}
              >
                All ({totalCount.toLocaleString()})
              </button>
              <button
                type="button"
                onClick={() => setOnlyPinned(!onlyPinned)}
                className={`btn btn-xs join-item gap-1 ${
                  onlyPinned ? "btn-warning" : "btn-ghost bg-base-100"
                }`}
              >
                <Pin className="w-3 h-3" />
                Pinned
              </button>
              <button
                type="button"
                onClick={() => setOnlyRecentClipboard(!onlyRecentClipboard)}
                title="Show items recently copied or saved to the clipboard"
                className={`btn btn-xs join-item gap-1 ${
                  onlyRecentClipboard ? "btn-accent" : "btn-ghost bg-base-100"
                }`}
              >
                <ClipboardCheck className="w-3 h-3" />
                Recently Copied
              </button>
            </div>

            {/* Media Type Filter */}
            <select
              value={mediaTypeFilter || "all"}
              onChange={(e) =>
                setMediaTypeFilter(
                  e.target.value === "all"
                    ? undefined
                    : (e.target.value as MediaType)
                )
              }
              className="select select-xs select-bordered bg-base-100 text-[11px]"
              title="Filter by Media Type"
            >
              <option value="all">All Media</option>
              <option value="screenshot">Screenshots</option>
              <option value="video">Replay Clips</option>
            </select>

            {/* Group / Collection Filter */}
            <select
              value={activeGroup || ""}
              onChange={(e) =>
                setActiveGroup(e.target.value ? e.target.value : undefined)
              }
              className="select select-xs select-bordered bg-base-100 text-[11px]"
              title="Filter by Group"
            >
              <option value="">All Groups ({groups.length})</option>
              {groups.map((g) => (
                <option key={g} value={g}>
                  📁 {g}
                </option>
              ))}
            </select>

            {/* Date Range Filter */}
            <select
              value={dateRange}
              onChange={(e) => setDateRange(e.target.value as DateRangeFilter)}
              className="select select-xs select-bordered bg-base-100 text-[11px]"
              title="Filter by Date Captured"
            >
              <option value="all">All Time</option>
              <option value="today">Last 24 Hours</option>
              <option value="7d">Last 7 Days</option>
              <option value="30d">Last 30 Days</option>
            </select>
          </div>

          {/* Right: B-Tree Sort Controls, Column Resizer & Folder/Benchmark Tools */}
          <div className="flex flex-wrap items-center gap-1.5">
            {/* Sort By + Reverse Direction Button */}
            <div className="join">
              <select
                value={sortBy}
                onChange={(e) => setSortBy(e.target.value as SortByField)}
                className="select select-xs select-bordered join-item bg-base-100 text-[11px]"
                title="Sort Field (Indexed in SQLite)"
              >
                <option value="created_at">Sort: Date</option>
                <option value="size_bytes">Sort: File Size</option>
                <option value="last_copied_at">Sort: Copied Time</option>
              </select>
              <button
                type="button"
                onClick={toggleSortDir}
                title={
                  sortDir === "desc"
                    ? "Newest / Largest First (Click to reverse to Oldest First)"
                    : "Oldest / Smallest First (Click to reverse to Newest First)"
                }
                className="btn btn-xs btn-outline border-base-300 bg-base-100 join-item gap-1 font-mono"
              >
                {sortDir === "desc" ? (
                  <>
                    <ArrowDownWideNarrow className="w-3.5 h-3.5 text-primary" />
                    DESC ↓
                  </>
                ) : (
                  <>
                    <ArrowUpNarrowWide className="w-3.5 h-3.5 text-secondary" />
                    ASC ↑
                  </>
                )}
              </button>
            </div>

            {/* Card Columns Resizer */}
            <div
              className="flex items-center gap-1.5 px-2 py-0.5 rounded-lg bg-base-100 border border-base-300"
              title="Resize Cards by changing grid columns (1 = Large Preview, 8 = Compact Grid)"
            >
              <Columns className="w-3.5 h-3.5 text-primary" />
              <button
                type="button"
                onClick={() => setColumnCount("auto")}
                className={`badge badge-xs cursor-pointer font-mono ${
                  columnCount === "auto" ? "badge-primary" : "badge-ghost"
                }`}
              >
                Auto
              </button>
              <input
                type="range"
                min={1}
                max={8}
                step={1}
                value={columnCount === "auto" ? 4 : columnCount}
                onChange={(e) => setColumnCount(Number(e.target.value))}
                className="range range-primary range-xs w-16"
              />
              <span className="font-mono text-[10px] w-6 text-right">
                {columnCount === "auto" ? "Auto" : `${columnCount}c`}
              </span>
            </div>

            <button
              type="button"
              onClick={handleOpenCapturesFolder}
              title="Open Working Captures Folder in Windows File Explorer / Finder"
              className="btn btn-xs btn-outline border-base-300 bg-base-100 gap-1"
            >
              <FolderOpen className="w-3 h-3 text-success" />
              Folder
            </button>

            <button
              type="button"
              onClick={handleReconcile}
              title="Prune orphaned SQLite rows & scan folder for any newly added external images"
              className="btn btn-xs btn-outline border-base-300 bg-base-100 gap-1"
            >
              <RefreshCw className="w-3 h-3 text-info" />
              Sync DB
              {lastReconciledCount !== null && (
                <span className="badge badge-xs badge-ghost font-mono">
                  {lastReconciledCount}
                </span>
              )}
            </button>

            <button
              type="button"
              onClick={handleSeedBenchmark}
              title="Populate 500 synthetic captures into SQLite to stress-test 60 FPS virtualized scrolling & sorting"
              className="btn btn-xs btn-soft btn-secondary gap-1"
            >
              <Sparkles className="w-3 h-3" />
              +500 Benchmark
            </button>
          </div>
        </div>
      )}

      {/* Multi-Select Batch Action Toolbar */}
      {activeTab === "gallery" && selectedIds.length > 0 && (
        <div className="flex flex-wrap items-center justify-between gap-2 px-6 py-2 bg-primary/15 border-b border-primary/40 text-xs animate-fade-in">
          <div className="flex items-center gap-2">
            <span className="badge badge-primary badge-sm font-bold gap-1">
              <CheckSquare className="w-3.5 h-3.5" />
              {selectedIds.length} Selected
            </span>
            <button
              type="button"
              onClick={selectAllVisible}
              className="btn btn-xs btn-ghost"
            >
              Select All ({items.length})
            </button>
            <button
              type="button"
              onClick={clearSelection}
              className="btn btn-xs btn-ghost"
            >
              Clear (Esc)
            </button>
          </div>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => {
                setBatchInputText("");
                setBatchModal("group");
              }}
              className="btn btn-xs btn-secondary gap-1.5"
            >
              <FolderPlus className="w-3.5 h-3.5" />
              Group Selected
            </button>

            <button
              type="button"
              onClick={() => {
                setBatchInputText("");
                setBatchModal("tag");
              }}
              className="btn btn-xs btn-info gap-1.5"
            >
              <Tag className="w-3.5 h-3.5" />
              Add Tags
            </button>

            <button
              type="button"
              onClick={handleOpenMoveModal}
              title="Move selected files to another folder on disk (removes them from this working gallery)"
              className="btn btn-xs btn-warning gap-1.5"
            >
              <FolderInput className="w-3.5 h-3.5" />
              Move to New Folder...
            </button>

            <button
              type="button"
              onClick={handleBatchDelete}
              className="btn btn-xs btn-error gap-1.5"
            >
              <Trash2 className="w-3.5 h-3.5" />
              Delete ({selectedIds.length})
            </button>
          </div>
        </div>
      )}

      {/* Batch Action Modal (Group / Add Tags / Move to External Folder) */}
      {batchModal && (
        <dialog
          className="modal modal-open"
          onClick={() => setBatchModal(null)}
        >
          <div
            className="modal-box max-w-md bg-base-200 border border-base-300"
            onClick={(e) => e.stopPropagation()}
          >
            <h3 className="font-bold text-sm flex items-center gap-2">
              {batchModal === "group" && (
                <>
                  <FolderPlus className="w-4 h-4 text-secondary" />
                  Group {selectedIds.length} Selected Capture(s)
                </>
              )}
              {batchModal === "tag" && (
                <>
                  <Tag className="w-4 h-4 text-info" />
                  Add Tags to {selectedIds.length} Selected Capture(s)
                </>
              )}
              {batchModal === "move" && (
                <>
                  <FolderInput className="w-4 h-4 text-warning" />
                  Move {selectedIds.length} Capture(s) to Another Folder
                </>
              )}
            </h3>

            <form onSubmit={handleBatchSubmit} className="mt-4 space-y-3">
              {batchModal === "group" && (
                <div className="space-y-2">
                  <input
                    type="text"
                    autoFocus
                    value={batchInputText}
                    onChange={(e) => setBatchInputText(e.target.value)}
                    placeholder="Enter group name (e.g. Valorant Clips, Bug Reports)..."
                    className="input input-sm input-bordered w-full bg-base-100 text-xs"
                  />
                  {groups.length > 0 && (
                    <div className="flex flex-wrap gap-1 pt-1">
                      <span className="text-[11px] text-base-content/60 mr-1">
                        Existing groups:
                      </span>
                      {groups.map((g) => (
                        <button
                          key={g}
                          type="button"
                          onClick={() => setBatchInputText(g)}
                          className="badge badge-soft badge-secondary badge-xs cursor-pointer"
                        >
                          {g}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {batchModal === "tag" && (
                <input
                  type="text"
                  autoFocus
                  value={batchInputText}
                  onChange={(e) => setBatchInputText(e.target.value)}
                  placeholder="Comma-separated tags (e.g. clutch, finals, urgent)..."
                  className="input input-sm input-bordered w-full bg-base-100 text-xs"
                />
              )}

              {batchModal === "move" && (
                <div className="space-y-2">
                  <div className="join w-full">
                    <input
                      type="text"
                      autoFocus
                      value={batchInputText}
                      onChange={(e) => setBatchInputText(e.target.value)}
                      placeholder="Destination folder path..."
                      className="input input-sm input-bordered join-item flex-1 bg-base-100 font-mono text-xs"
                    />
                    <button
                      type="button"
                      onClick={async () => {
                        const p = await pickFolderDialog(
                          batchInputText || settings?.capturesDir
                        );
                        if (p) setBatchInputText(p);
                      }}
                      className="btn btn-sm btn-neutral join-item"
                    >
                      Browse...
                    </button>
                  </div>
                  <p className="text-[11px] text-warning/90">
                    Moving files to an external folder relocates them on disk and removes them from this working gallery.
                  </p>
                </div>
              )}

              <div className="modal-action mt-4">
                <button
                  type="button"
                  onClick={() => setBatchModal(null)}
                  className="btn btn-xs btn-ghost"
                >
                  Cancel
                </button>
                <button type="submit" className="btn btn-xs btn-primary">
                  Apply to {selectedIds.length} Item(s)
                </button>
              </div>
            </form>
          </div>
        </dialog>
      )}

      {/* Ask Before Importing Unindexed Media Modal */}
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
              . Would you like to import them into your Gallery?
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
              Original file modification dates will be preserved, and fast 320px WebP thumbnails will be generated.
            </p>

            <div className="modal-action">
              <button
                type="button"
                disabled={isImportingExternal}
                onClick={() => setUnindexedPrompt(null)}
                className="btn btn-xs btn-ghost"
              >
                Skip for Now
              </button>
              <button
                type="button"
                disabled={isImportingExternal}
                onClick={handleConfirmImportExternal}
                className="btn btn-xs btn-success gap-1.5"
              >
                {isImportingExternal ? (
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

      {/* daisyUI Toast Notification */}
      {statusToast && (
        <div className="toast toast-end toast-bottom z-50">
          <div className="alert alert-info shadow-xl text-xs py-2.5 px-4">
            <Database className="w-4 h-4 shrink-0" />
            <span>{statusToast}</span>
          </div>
        </div>
      )}

      {/* Main Content Area */}
      {activeTab === "gallery" ? (
        <VirtualizedGrid
          items={items}
          columnSetting={columnCount}
          selectedIds={selectedIds}
          onSelectionChange={setSelectedIds}
          onToggleSelectId={toggleSelectId}
          onSelect={(cap) => setSelectedCapture(cap)}
          onTogglePin={togglePin}
          onDelete={(id) => removeCaptures([id])}
          onCopied={() => fetchCaptures()}
        />
      ) : (
        <SettingsPanel />
      )}

      {/* Full-Resolution Lightbox Modal */}
      {selectedCapture && (
        <MediaLightbox
          capture={selectedCapture}
          onClose={() => setSelectedCapture(null)}
          onTogglePin={togglePin}
          onDelete={(id) => removeCaptures([id])}
          onAddTag={addTag}
        />
      )}
    </div>
  );
};
