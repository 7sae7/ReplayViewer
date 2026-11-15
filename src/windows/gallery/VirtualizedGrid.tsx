import React, { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Check,
  ClipboardCheck,
  Copy,
  Folder,
  FolderOpen,
  Maximize2,
  Pin,
  Trash2,
} from "lucide-react";
import type { CaptureRecord } from "../../types/ipc";
import { formatBytes } from "../../lib/dpi";
import {
  copyCaptureToClipboard,
  openInFileExplorer,
  resolveMediaSrc,
} from "../../lib/tauri-client";

interface VirtualizedGridProps {
  items: CaptureRecord[];
  columnSetting: number | "auto";
  selectedIds: string[];
  onSelectionChange: (ids: string[]) => void;
  onToggleSelectId: (id: string) => void;
  onSelect: (capture: CaptureRecord) => void;
  onTogglePin: (id: string, currentPinned: boolean) => void;
  onDelete: (id: string) => void;
  onCopied?: () => void;
}

interface MarqueeBox {
  left: number;
  top: number;
  width: number;
  height: number;
}

export const VirtualizedGrid: React.FC<VirtualizedGridProps> = ({
  items,
  columnSetting,
  selectedIds,
  onSelectionChange,
  onToggleSelectId,
  onSelect,
  onTogglePin,
  onDelete,
  onCopied,
}) => {
  const parentRef = useRef<HTMLDivElement | null>(null);
  const [autoColumns, setAutoColumns] = useState(4);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [marquee, setMarquee] = useState<MarqueeBox | null>(null);
  const suppressClickRef = useRef(false);

  const columns = columnSetting === "auto" ? autoColumns : columnSetting;

  // Dynamic row height based on column count so cards scale naturally when resized
  const rowHeight =
    columns === 1
      ? 380
      : columns === 2
      ? 320
      : columns <= 4
      ? 268
      : columns <= 6
      ? 230
      : 205;

  const thumbHeightClass =
    columns === 1
      ? "h-64"
      : columns === 2
      ? "h-48"
      : columns <= 4
      ? "h-36"
      : columns <= 6
      ? "h-28"
      : "h-24";

  useEffect(() => {
    const updateCols = () => {
      const w = parentRef.current?.clientWidth || window.innerWidth;
      if (w >= 1400) setAutoColumns(4);
      else if (w >= 1024) setAutoColumns(3);
      else if (w >= 640) setAutoColumns(2);
      else setAutoColumns(1);
    };
    updateCols();
    window.addEventListener("resize", updateCols);
    return () => window.removeEventListener("resize", updateCols);
  }, []);

  const rowCount = Math.ceil(items.length / columns);

  const rowVirtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => parentRef.current,
    estimateSize: () => rowHeight,
    overscan: 3,
  });

  useEffect(() => {
    rowVirtualizer.measure();
  }, [rowHeight, columns, rowVirtualizer]);

  const handleCopy = async (e: React.MouseEvent, item: CaptureRecord) => {
    e.stopPropagation();
    await copyCaptureToClipboard(item.filePath);
    setCopiedId(item.id);
    if (onCopied) onCopied();
    setTimeout(() => setCopiedId(null), 1500);
  };

  const handleOpenExplorer = async (e: React.MouseEvent, item: CaptureRecord) => {
    e.stopPropagation();
    await openInFileExplorer(item.filePath);
  };

  // Left-click hold & drag marquee selection (works even when starting on an image card!)
  const handleMouseDown = (e: React.MouseEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input, a, select, label")) {
      return;
    }

    const container = parentRef.current;
    if (!container) return;

    const rect = container.getBoundingClientRect();
    const startClientX = e.clientX;
    const startClientY = e.clientY;
    const startScrollX = e.clientX - rect.left;
    const startScrollY = e.clientY - rect.top + container.scrollTop;
    const additive = e.ctrlKey || e.metaKey || e.shiftKey;
    const baseSelection = new Set(additive ? selectedIds : []);

    let isDraggingMarquee = false;

    const updateMarqueeSelection = (clientX: number, clientY: number) => {
      const curRect = container.getBoundingClientRect();
      // Auto-scroll when dragging near top or bottom edges
      const edgeThreshold = 42;
      if (clientY < curRect.top + edgeThreshold) {
        container.scrollTop -= 14;
      } else if (clientY > curRect.bottom - edgeThreshold) {
        container.scrollTop += 14;
      }

      const curScrollX = Math.max(
        0,
        Math.min(container.clientWidth, clientX - curRect.left)
      );
      const curScrollY = Math.max(
        0,
        clientY - curRect.top + container.scrollTop
      );

      const minX = Math.min(startScrollX, curScrollX);
      const maxX = Math.max(startScrollX, curScrollX);
      const minY = Math.min(startScrollY, curScrollY);
      const maxY = Math.max(startScrollY, curScrollY);

      setMarquee({
        left: minX,
        top: minY,
        width: Math.max(2, maxX - minX),
        height: Math.max(2, maxY - minY),
      });

      // Compute intersecting grid cards in O(N) pure math across virtualized rows
      const padX = 24; // px-6
      const padY = 16; // py-4
      const gap = 16;
      const innerWidth = Math.max(100, container.clientWidth - padX * 2);
      const colWidth = (innerWidth - (columns - 1) * gap) / columns;
      const cardHeight = rowHeight - gap;

      const nextSelected = new Set(baseSelection);
      for (let i = 0; i < items.length; i++) {
        const r = Math.floor(i / columns);
        const c = i % columns;
        const cardLeft = padX + c * (colWidth + gap);
        const cardRight = cardLeft + colWidth;
        const cardTop = padY + r * rowHeight;
        const cardBottom = cardTop + cardHeight;

        const intersects =
          cardRight >= minX &&
          cardLeft <= maxX &&
          cardBottom >= minY &&
          cardTop <= maxY;

        if (intersects) {
          nextSelected.add(items[i].id);
        }
      }

      onSelectionChange(Array.from(nextSelected));
    };

    const handleWindowMouseMove = (ev: MouseEvent) => {
      const dist = Math.hypot(
        ev.clientX - startClientX,
        ev.clientY - startClientY
      );
      if (!isDraggingMarquee && dist >= 6) {
        isDraggingMarquee = true;
        suppressClickRef.current = true;
      }
      if (isDraggingMarquee) {
        updateMarqueeSelection(ev.clientX, ev.clientY);
      }
    };

    const handleWindowMouseUp = (ev: MouseEvent) => {
      window.removeEventListener("mousemove", handleWindowMouseMove);
      window.removeEventListener("mouseup", handleWindowMouseUp);

      if (isDraggingMarquee) {
        setMarquee(null);
        setTimeout(() => {
          suppressClickRef.current = false;
        }, 60);
      } else {
        // Plain click on empty background clears multi-selection
        const upTarget = ev.target as HTMLElement | null;
        if (upTarget && !upTarget.closest("[data-capture-card]")) {
          if (selectedIds.length > 0 && !additive) {
            onSelectionChange([]);
          }
        }
      }
    };

    window.addEventListener("mousemove", handleWindowMouseMove);
    window.addEventListener("mouseup", handleWindowMouseUp);
  };

  if (items.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full text-base-content/60 py-24">
        <div className="card bg-base-200 border border-base-300 shadow-lg max-w-md">
          <div className="card-body items-center text-center gap-2">
            <h3 className="card-title text-base text-base-content">
              No captures match your current filter
            </h3>
            <p className="text-xs text-base-content/70 leading-relaxed">
              Press <kbd className="kbd kbd-sm">Alt</kbd> +{" "}
              <kbd className="kbd kbd-sm">S</kbd> to capture a region or{" "}
              <kbd className="kbd kbd-sm">Alt</kbd> +{" "}
              <kbd className="kbd kbd-sm">Shift</kbd> +{" "}
              <kbd className="kbd kbd-sm">S</kbd> for instant fullscreen.
            </p>
          </div>
        </div>
      </div>
    );
  }

  const selectedSet = new Set(selectedIds);

  return (
    <div
      ref={parentRef}
      onMouseDown={handleMouseDown}
      className="relative flex-1 overflow-y-auto px-6 py-4 select-none"
    >
      {/* Rubber-Band Marquee Selection Overlay */}
      {marquee && (
        <div
          style={{
            position: "absolute",
            left: `${marquee.left}px`,
            top: `${marquee.top}px`,
            width: `${marquee.width}px`,
            height: `${marquee.height}px`,
          }}
          className="z-30 pointer-events-none rounded-lg border-2 border-primary bg-primary/20 backdrop-blur-[1px] shadow-lg"
        />
      )}

      <div
        style={{
          height: `${rowVirtualizer.getTotalSize()}px`,
          width: "100%",
          position: "relative",
        }}
      >
        {rowVirtualizer.getVirtualItems().map((virtualRow) => {
          const startIndex = virtualRow.index * columns;
          const rowItems = items.slice(startIndex, startIndex + columns);

          return (
            <div
              key={virtualRow.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: `${virtualRow.size}px`,
                transform: `translateY(${virtualRow.start}px)`,
                display: "grid",
                gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
                gap: "16px",
                paddingBottom: "16px",
              }}
            >
              {rowItems.map((item) => {
                const isSelected = selectedSet.has(item.id);
                return (
                  <div
                    key={item.id}
                    data-capture-card="true"
                    draggable={false}
                    onClick={(e) => {
                      if (suppressClickRef.current) return;
                      if (
                        e.ctrlKey ||
                        e.metaKey ||
                        e.shiftKey ||
                        selectedIds.length > 0
                      ) {
                        onToggleSelectId(item.id);
                      } else {
                        onSelect(item);
                      }
                    }}
                    onDoubleClick={() => onSelect(item)}
                    className={`card card-xs bg-base-200 border transition-all cursor-pointer overflow-hidden group ${
                      isSelected
                        ? "border-primary ring-2 ring-primary bg-primary/10 shadow-xl"
                        : "border-base-300 hover:border-primary/60 shadow-md hover:shadow-xl"
                    }`}
                  >
                    {/* Thumbnail Container */}
                    <figure
                      className={`relative ${thumbHeightClass} w-full bg-base-300 overflow-hidden`}
                    >
                      <img
                        src={resolveMediaSrc(item.thumbPath)}
                        alt={item.sourceApp || "Capture"}
                        loading="lazy"
                        draggable={false}
                        className="w-full h-full object-cover group-hover:scale-105 transition-transform duration-200 pointer-events-none"
                      />

                      {/* Top-Left Multi-Select Checkbox + Pinned / Group Badges */}
                      <div className="absolute top-2 left-2 flex items-center gap-1.5 z-10">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onToggleSelectId(item.id);
                          }}
                          title="Select item (or left-click & drag across cards)"
                          className={`btn btn-xs btn-square rounded-md border transition-opacity ${
                            isSelected
                              ? "btn-primary border-primary opacity-100"
                              : "bg-base-100/80 border-base-300 opacity-0 group-hover:opacity-100"
                          }`}
                        >
                          {isSelected && <Check className="w-3.5 h-3.5" />}
                        </button>

                        {item.isPinned && (
                          <div className="badge badge-warning badge-xs gap-1 font-bold uppercase tracking-wider shadow">
                            <Pin className="w-2.5 h-2.5" /> Pinned
                          </div>
                        )}

                        {item.groupName && (
                          <div className="badge badge-secondary badge-xs gap-1 font-semibold shadow">
                            <Folder className="w-2.5 h-2.5" /> {item.groupName}
                          </div>
                        )}
                      </div>

                      {/* Top Right Quick Actions */}
                      <div className="absolute top-2 right-2 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity z-10">
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onSelect(item);
                          }}
                          title="Open Full Lightbox View"
                          className="btn btn-xs btn-square bg-base-100/85 hover:btn-info shadow"
                        >
                          <Maximize2 className="w-3.5 h-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onTogglePin(item.id, item.isPinned);
                          }}
                          title={
                            item.isPinned
                              ? "Unpin (Allow Auto-Cleanup)"
                              : "Pin (Exempt from Auto-Cleanup)"
                          }
                          className={`btn btn-xs btn-square shadow ${
                            item.isPinned
                              ? "btn-warning"
                              : "bg-base-100/85 hover:btn-warning"
                          }`}
                        >
                          <Pin className="w-3.5 h-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={(e) => handleOpenExplorer(e, item)}
                          title="Show in File Explorer / Finder"
                          className="btn btn-xs btn-square bg-base-100/85 hover:btn-secondary shadow"
                        >
                          <FolderOpen className="w-3.5 h-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={(e) => handleCopy(e, item)}
                          title="Copy Image to Clipboard"
                          className="btn btn-xs btn-square bg-base-100/85 hover:btn-primary shadow"
                        >
                          <Copy className="w-3.5 h-3.5" />
                        </button>
                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            onDelete(item.id);
                          }}
                          title="Delete Capture"
                          className="btn btn-xs btn-square bg-base-100/85 hover:btn-error shadow"
                        >
                          <Trash2 className="w-3.5 h-3.5" />
                        </button>
                      </div>

                      {copiedId === item.id && (
                        <div className="absolute inset-0 bg-success/85 backdrop-blur-xs flex items-center justify-center text-xs font-bold text-success-content z-20">
                          Copied to Clipboard!
                        </div>
                      )}

                      {item.lastCopiedAt && (
                        <div
                          title={`Copied to clipboard at ${new Date(
                            item.lastCopiedAt
                          ).toLocaleTimeString()}`}
                          className="badge badge-accent badge-xs gap-1 absolute bottom-2 left-2 opacity-90"
                        >
                          <ClipboardCheck className="w-2.5 h-2.5" />
                          Clipboard
                        </div>
                      )}

                      <div className="badge badge-neutral badge-xs font-mono gap-1 absolute bottom-2 right-2 opacity-90">
                        <Maximize2 className="w-2.5 h-2.5" />
                        {item.width}×{item.height}
                      </div>
                    </figure>

                    {/* Metadata Footer */}
                    <div className="card-body justify-between p-3 text-xs">
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-semibold text-base-content truncate">
                          {item.sourceApp || "Screenshot"}
                        </span>
                        <span className="badge badge-ghost badge-xs font-mono shrink-0">
                          {formatBytes(item.sizeBytes)}
                        </span>
                      </div>

                      <div className="flex items-center justify-between gap-1 mt-1">
                        <div className="flex items-center gap-1 overflow-hidden">
                          {item.tags.slice(0, 3).map((tag) => (
                            <span
                              key={tag}
                              className="badge badge-soft badge-primary badge-xs font-medium truncate"
                            >
                              #{tag}
                            </span>
                          ))}
                        </div>
                        <span className="text-[10px] text-base-content/50 shrink-0 font-mono">
                          {new Date(item.createdAt).toLocaleTimeString([], {
                            hour: "2-digit",
                            minute: "2-digit",
                          })}
                        </span>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
};
