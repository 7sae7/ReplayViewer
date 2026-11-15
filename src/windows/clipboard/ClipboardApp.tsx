import React, { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  Check,
  Clipboard,
  Copy,
  FolderPlus,
  Pause,
  PenTool,
  Pin,
  PinOff,
  Play,
  RefreshCw,
  Search,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import {
  annotateClipboardItem,
  clearUnpinnedClipboardHistory,
  deleteClipboardItem,
  getClipboardAlwaysOnTop,
  getClipboardPaused,
  getSettings,
  isTauriRuntime,
  listClipboardHistory,
  pasteClipboardItem,
  promoteClipboardToGallery,
  resolveMediaSrc,
  setClipboardAlwaysOnTop,
  setClipboardPaused,
  togglePinClipboardItem,
  updateSettings,
} from "../../lib/tauri-client";
import type { ClipboardItem, SettingsPayload } from "../../types/ipc";

interface ClipboardAppProps {
  onCloseInBrowserPreview?: () => void;
  onAnnotateInBrowserPreview?: () => void;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(0)} KB`;
  return `${(kb / 1024).toFixed(2)} MB`;
}

function formatRelativeTime(iso: string): string {
  const diffSec = Math.max(
    0,
    Math.floor((Date.now() - new Date(iso).getTime()) / 1000)
  );
  if (diffSec < 15) return "Just now";
  if (diffSec < 60) return `${diffSec}s ago`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  return new Date(iso).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

export const ClipboardApp: React.FC<ClipboardAppProps> = ({
  onCloseInBrowserPreview,
  onAnnotateInBrowserPreview,
}) => {
  const [items, setItems] = useState<ClipboardItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [filterTab, setFilterTab] = useState<"all" | "pinned">("all");
  const [searchQuery, setSearchQuery] = useState("");
  const [paused, setPaused] = useState(false);
  const [windowAlwaysOnTop, setWindowAlwaysOnTop] = useState(true);
  const [settings, setSettings] = useState<SettingsPayload | null>(null);
  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  const showToast = useCallback((msg: string) => {
    setToastMsg(msg);
    setTimeout(() => setToastMsg(null), 2200);
  }, []);

  // Sync daisyUI theme with Gallery window
  useEffect(() => {
    const applySavedTheme = () => {
      const saved = localStorage.getItem("rv_daisy_theme") || "night";
      document.documentElement.setAttribute("data-theme", saved);
    };
    applySavedTheme();
    window.addEventListener("storage", applySavedTheme);
    return () => window.removeEventListener("storage", applySavedTheme);
  }, []);

  const refreshClipboard = useCallback(async () => {
    setLoading(true);
    try {
      const [history, isPaused, isOnTop, cfg] = await Promise.all([
        listClipboardHistory(),
        getClipboardPaused(),
        getClipboardAlwaysOnTop(),
        getSettings(),
      ]);
      setItems(history);
      setPaused(isPaused);
      setWindowAlwaysOnTop(isOnTop);
      setSettings(cfg);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refreshClipboard();
  }, [refreshClipboard]);

  // Listen for live clipboard updates & drawer open events in Tauri runtime
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlistenUpdated: (() => void) | undefined;
    let unlistenOpened: (() => void) | undefined;

    listen("clipboard:updated", () => {
      refreshClipboard();
    }).then((fn) => {
      unlistenUpdated = fn;
    });

    listen("clipboard:opened", () => {
      setSelectedIndex(0);
      refreshClipboard();
    }).then((fn) => {
      unlistenOpened = fn;
    });

    return () => {
      if (unlistenUpdated) unlistenUpdated();
      if (unlistenOpened) unlistenOpened();
    };
  }, [refreshClipboard]);

  const closeDrawer = useCallback(async () => {
    if (isTauriRuntime()) {
      await getCurrentWindow().hide();
    } else if (onCloseInBrowserPreview) {
      onCloseInBrowserPreview();
    }
  }, [onCloseInBrowserPreview]);

  const filteredItems = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return items.filter((item) => {
      if (filterTab === "pinned" && !item.isPinned) return false;
      if (q) {
        const hay = `${item.width}x${item.height} ${item.blake3Hash} ${formatBytes(
          item.sizeBytes
        )}`.toLowerCase();
        return hay.includes(q);
      }
      return true;
    });
  }, [items, filterTab, searchQuery]);

  // Keep selectedIndex within bounds
  useEffect(() => {
    if (selectedIndex >= filteredItems.length) {
      setSelectedIndex(Math.max(0, filteredItems.length - 1));
    }
  }, [filteredItems.length, selectedIndex]);

  const handlePasteItem = useCallback(
    async (item: ClipboardItem) => {
      setCopiedId(item.id);
      await pasteClipboardItem(item.id);
      showToast(`Copied ${item.width}×${item.height} image to OS Clipboard`);
      setTimeout(() => {
        setCopiedId(null);
        closeDrawer();
      }, 180);
    },
    [closeDrawer, showToast]
  );

  const handleAnnotateItem = useCallback(
    async (item: ClipboardItem, e?: React.MouseEvent) => {
      e?.stopPropagation();
      await annotateClipboardItem(item.id);
      if (!isTauriRuntime() && onAnnotateInBrowserPreview) {
        onAnnotateInBrowserPreview();
      }
    },
    [onAnnotateInBrowserPreview]
  );

  const handlePromoteToGallery = useCallback(
    async (item: ClipboardItem, e: React.MouseEvent) => {
      e.stopPropagation();
      await promoteClipboardToGallery(item.id);
      showToast(`Saved ${item.width}×${item.height} image to Gallery!`);
    },
    [showToast]
  );

  const handleTogglePin = useCallback(
    async (item: ClipboardItem, e: React.MouseEvent) => {
      e.stopPropagation();
      await togglePinClipboardItem(item.id, !item.isPinned);
      await refreshClipboard();
      showToast(
        !item.isPinned
          ? "Pinned to top (protected from FIFO ring eviction)"
          : "Unpinned item"
      );
    },
    [refreshClipboard, showToast]
  );

  const handleDeleteItem = useCallback(
    async (item: ClipboardItem, e: React.MouseEvent) => {
      e.stopPropagation();
      await deleteClipboardItem(item.id);
      await refreshClipboard();
    },
    [refreshClipboard]
  );

  const handleClearUnpinned = async () => {
    const removed = await clearUnpinnedClipboardHistory();
    await refreshClipboard();
    showToast(`Cleared ${removed} unpinned clipboard item(s)`);
  };

  const handleTogglePause = async () => {
    const next = await setClipboardPaused(!paused);
    setPaused(next);
    showToast(
      next
        ? "Paused OS Clipboard image history watcher"
        : "Resumed OS Clipboard image history watcher"
    );
  };

  // Fix #1: Persistent Window "Always on Top" Toggle
  const handleToggleWindowAlwaysOnTop = async () => {
    const next = await setClipboardAlwaysOnTop(!windowAlwaysOnTop);
    setWindowAlwaysOnTop(next);
    showToast(
      next
        ? "Window pinned on top (remembered for next Alt+V)"
        : "Window unpinned from top (remembered for next Alt+V)"
    );
  };

  const handleChangeMaxItems = async (nextLimit: number) => {
    if (!settings) return;
    const updated = await updateSettings({
      ...settings,
      clipboardMaxItems: nextLimit,
    });
    setSettings(updated);
    await refreshClipboard();
    showToast(`FIFO ring buffer set to ${nextLimit} unpinned images`);
  };

  // Keyboard shortcuts: 1-9 instant paste, ArrowUp/ArrowDown, Enter, A (annotate), Escape
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const inInput =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "SELECT" ||
          target.tagName === "TEXTAREA");

      if (e.key === "Escape") {
        e.preventDefault();
        closeDrawer();
        return;
      }

      if (inInput) return;

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((prev) =>
          filteredItems.length > 0 ? (prev + 1) % filteredItems.length : 0
        );
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((prev) =>
          filteredItems.length > 0
            ? (prev - 1 + filteredItems.length) % filteredItems.length
            : 0
        );
      } else if (e.key === "Enter") {
        e.preventDefault();
        const active = filteredItems[selectedIndex];
        if (active) handlePasteItem(active);
      } else if (e.key.toLowerCase() === "a") {
        e.preventDefault();
        const active = filteredItems[selectedIndex];
        if (active) handleAnnotateItem(active);
      } else if (/^[1-9]$/.test(e.key)) {
        const idx = Number(e.key) - 1;
        if (idx < filteredItems.length) {
          e.preventDefault();
          handlePasteItem(filteredItems[idx]);
        }
      }
    };

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [
    closeDrawer,
    filteredItems,
    handleAnnotateItem,
    handlePasteItem,
    selectedIndex,
  ]);

  const handleStartDrag = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, a, [data-no-drag]")) return;
    if (isTauriRuntime()) {
      getCurrentWindow().startDragging();
    }
  };

  const pinnedCount = useMemo(
    () => items.filter((i) => i.isPinned).length,
    [items]
  );

  return (
    <div className="flex flex-col h-screen w-screen bg-base-100 text-base-content select-none overflow-hidden border border-base-300 shadow-2xl">
      {/* Frameless Draggable daisyUI Header */}
      <header
        onMouseDown={handleStartDrag}
        className="flex items-center justify-between px-3 py-2 bg-base-300 border-b border-base-content/10 cursor-move shrink-0"
      >
        <div className="flex items-center gap-2">
          <div className="w-6 h-6 rounded-field bg-primary flex items-center justify-center text-primary-content shadow-xs">
            <Clipboard className="w-3.5 h-3.5" />
          </div>
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-bold tracking-tight">
              Visual Clipboard
            </span>
            <kbd className="kbd kbd-xs font-mono">Alt+V</kbd>
          </div>
        </div>

        <div className="flex items-center gap-1" data-no-drag>
          {/* Fix #1: Window Always-on-Top Pin / Unpin Toggle (Persisted across Alt+V) */}
          <button
            onClick={handleToggleWindowAlwaysOnTop}
            className={`btn btn-xs gap-1 ${
              windowAlwaysOnTop ? "btn-primary" : "btn-ghost"
            }`}
            title={
              windowAlwaysOnTop
                ? "Window is pinned Always on Top — click to unpin (remembered on next Alt+V)"
                : "Window is unpinned from top — click to keep Always on Top"
            }
          >
            {windowAlwaysOnTop ? (
              <>
                <Pin className="w-3 h-3 fill-current" /> Top
              </>
            ) : (
              <>
                <PinOff className="w-3 h-3" /> Unpinned
              </>
            )}
          </button>

          <button
            onClick={handleTogglePause}
            className={`btn btn-xs gap-1 ${
              paused ? "btn-warning" : "btn-ghost"
            }`}
            title={
              paused
                ? "Clipboard history is paused — click to resume"
                : "Pause clipboard image recording"
            }
          >
            {paused ? (
              <>
                <Play className="w-3 h-3" /> Paused
              </>
            ) : (
              <>
                <Pause className="w-3 h-3" /> Live
              </>
            )}
          </button>

          <button
            onClick={refreshClipboard}
            className="btn btn-ghost btn-xs btn-square"
            title="Refresh clipboard history"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? "animate-spin" : ""}`} />
          </button>

          <button
            onClick={closeDrawer}
            className="btn btn-ghost btn-xs btn-square hover:btn-error"
            title="Close Drawer (Esc)"
          >
            <X className="w-3.5 h-3.5" />
          </button>
        </div>
      </header>

      {/* Filter Tabs, Search & FIFO Ring Capacity Bar */}
      <div className="px-3 py-2 bg-base-200 border-b border-base-300 flex flex-col gap-2 shrink-0">
        <div className="flex items-center justify-between gap-2">
          <div role="tablist" className="tabs tabs-box tabs-xs">
            <button
              role="tab"
              onClick={() => setFilterTab("all")}
              className={`tab gap-1 ${filterTab === "all" ? "tab-active" : ""}`}
            >
              All
              <span className="badge badge-xs">{items.length}</span>
            </button>
            <button
              role="tab"
              onClick={() => setFilterTab("pinned")}
              className={`tab gap-1 ${
                filterTab === "pinned" ? "tab-active" : ""
              }`}
            >
              <Pin className="w-3 h-3" /> Pinned
              <span className="badge badge-xs">{pinnedCount}</span>
            </button>
          </div>

          <div className="flex items-center gap-1.5">
            <span className="text-[11px] text-base-content/60">FIFO Limit:</span>
            <select
              value={settings?.clipboardMaxItems ?? 10}
              onChange={(e) => handleChangeMaxItems(Number(e.target.value))}
              className="select select-xs w-20 font-mono"
              title="Maximum unpinned images kept in ring buffer (Pinned items are exempt)"
            >
              <option value={5}>5 imgs</option>
              <option value={10}>10 imgs</option>
              <option value={20}>20 imgs</option>
              <option value={50}>50 imgs</option>
            </select>
          </div>
        </div>

        <label className="input input-xs w-full flex items-center gap-2">
          <Search className="w-3.5 h-3.5 text-base-content/50" />
          <input
            type="text"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Filter by size (1920x1080) or hash..."
            className="grow text-xs"
          />
          {searchQuery && (
            <button
              onClick={() => setSearchQuery("")}
              className="btn btn-ghost btn-xs btn-circle"
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </label>
      </div>

      {/* Scrollable Clipboard Ring Buffer List */}
      <main className="flex-1 overflow-y-auto p-2.5 space-y-2">
        {filteredItems.length === 0 ? (
          <div className="h-full flex flex-col items-center justify-center text-center p-6">
            <div className="w-11 h-11 rounded-box bg-base-200 border border-base-300 flex items-center justify-center mb-3 text-base-content/40">
              <Clipboard className="w-5 h-5" />
            </div>
            <p className="text-xs font-semibold text-base-content/80">
              No clipboard images in view
            </p>
            <p className="text-[11px] text-base-content/50 mt-1 max-w-xs">
              Copy any image from a browser, Discord, design tool, or press{" "}
              <kbd className="kbd kbd-xs">Alt+S</kbd> to populate your visual
              clipboard ring.
            </p>
          </div>
        ) : (
          filteredItems.map((item, idx) => {
            const isSelected = idx === selectedIndex;
            const isCopied = copiedId === item.id;
            const shortcutNum = idx < 9 ? idx + 1 : null;
            const hasEditableHistory =
              Boolean(item.annotationsJson) && item.annotationsJson !== "[]";

            return (
              <div
                key={item.id}
                onClick={() => handlePasteItem(item)}
                onMouseEnter={() => setSelectedIndex(idx)}
                className={`card card-side card-border bg-base-200/80 hover:bg-base-200 transition-all cursor-pointer overflow-hidden ${
                  isSelected
                    ? "ring-2 ring-primary border-primary/50 shadow-md"
                    : item.isCurrentlyCopied
                    ? "border-success/60"
                    : "border-base-300"
                }`}
              >
                {/* Fix #3: Plain div container instead of <figure> so daisyUI never stretches child badges over the image! */}
                <div className="relative w-28 h-20 shrink-0 bg-base-300 overflow-hidden border-r border-base-300">
                  <img
                    src={resolveMediaSrc(item.thumbPath)}
                    alt={`Clipboard ${item.width}x${item.height}`}
                    draggable={false}
                    className="w-full h-full object-contain bg-base-300"
                  />
                </div>

                {/* Card Metadata & Quick Actions */}
                <div className="card-body p-2 justify-between min-w-0">
                  <div className="flex items-center justify-between gap-1 flex-wrap">
                    <div className="flex items-center gap-1 min-w-0 flex-wrap">
                      {shortcutNum !== null && (
                        <kbd
                          className="kbd kbd-xs font-mono font-bold"
                          title={`Press ${shortcutNum} to instant-copy`}
                        >
                          {shortcutNum}
                        </kbd>
                      )}
                      <span className="badge badge-xs badge-neutral font-mono">
                        {item.width}×{item.height}
                      </span>
                      {item.isPinned && (
                        <span
                          className="badge badge-warning badge-xs gap-0.5 font-semibold"
                          title="Pinned to top (exempt from FIFO eviction)"
                        >
                          <Pin className="w-2.5 h-2.5 fill-current" /> Pinned
                        </span>
                      )}
                      {/* Fix #5: CURRENTLY COPIED badge */}
                      {item.isCurrentlyCopied && (
                        <span className="badge badge-success badge-xs font-bold tracking-tight">
                          CURRENTLY COPIED
                        </span>
                      )}
                      {hasEditableHistory && (
                        <span
                          className="badge badge-info badge-soft badge-xs"
                          title="Has editable vector annotations (Ctrl+Z / Ctrl+Y supported)"
                        >
                          Layers
                        </span>
                      )}
                    </div>
                    <span className="text-[10px] text-base-content/50 shrink-0 font-mono">
                      {formatBytes(item.sizeBytes)} •{" "}
                      {formatRelativeTime(item.copiedAt)}
                    </span>
                  </div>

                  <div className="card-actions justify-between items-center mt-1 pt-1 border-t border-base-content/5">
                    <span className="text-[11px] font-medium text-primary flex items-center gap-1">
                      {isCopied ? (
                        <>
                          <Check className="w-3 h-3 text-success" /> Copied!
                        </>
                      ) : (
                        <>
                          <Copy className="w-3 h-3" /> Click / Enter
                        </>
                      )}
                    </span>

                    <div className="flex items-center gap-1">
                      <button
                        onClick={(e) => handleAnnotateItem(item, e)}
                        className="btn btn-xs btn-ghost btn-square"
                        title="Open in Annotation Editor at 1:1 Size (A)"
                      >
                        <PenTool className="w-3.5 h-3.5 text-info" />
                      </button>

                      <button
                        onClick={(e) => handlePromoteToGallery(item, e)}
                        className="btn btn-xs btn-ghost btn-square"
                        title="Save permanently to Gallery"
                      >
                        <FolderPlus className="w-3.5 h-3.5 text-success" />
                      </button>

                      <button
                        onClick={(e) => handleTogglePin(item, e)}
                        className={`btn btn-xs btn-ghost btn-square ${
                          item.isPinned ? "text-warning" : ""
                        }`}
                        title={
                          item.isPinned
                            ? "Unpin item"
                            : "Pin item to top (protect from FIFO eviction)"
                        }
                      >
                        <Pin
                          className={`w-3.5 h-3.5 ${
                            item.isPinned ? "fill-current" : ""
                          }`}
                        />
                      </button>

                      <button
                        onClick={(e) => handleDeleteItem(item, e)}
                        className="btn btn-xs btn-ghost btn-square hover:text-error"
                        title="Remove from clipboard history"
                      >
                        <Trash2 className="w-3.5 h-3.5" />
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            );
          })
        )}
      </main>

      {/* Footer Keyboard Legend & Clear Unpinned Action */}
      <footer className="px-3 py-2 bg-base-200 border-t border-base-300 flex items-center justify-between text-[11px] text-base-content/60 shrink-0">
        <div className="flex items-center gap-1.5 flex-wrap">
          <span>
            <kbd className="kbd kbd-xs">1-9</kbd> Paste
          </span>
          <span>•</span>
          <span>
            <kbd className="kbd kbd-xs">A</kbd> Annotate
          </span>
          <span>•</span>
          <span>
            <kbd className="kbd kbd-xs">Esc</kbd> Close
          </span>
        </div>

        <button
          onClick={handleClearUnpinned}
          disabled={items.filter((i) => !i.isPinned).length === 0}
          className="btn btn-ghost btn-xs text-error disabled:opacity-40"
          title="Delete all unpinned clipboard items"
        >
          Clear Unpinned
        </button>
      </footer>

      {/* Toast Notification */}
      {toastMsg && (
        <div className="toast toast-bottom toast-center z-50 mb-8">
          <div className="alert alert-info py-1.5 px-3 text-xs shadow-lg">
            <Sparkles className="w-3.5 h-3.5 shrink-0" />
            <span>{toastMsg}</span>
          </div>
        </div>
      )}
    </div>
  );
};
