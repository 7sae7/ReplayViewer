import { beforeEach, describe, expect, it } from "vitest";
import { formatBytes, normalizeCropRect, toPhysicalRect } from "../lib/dpi";
import {
  clearUnpinnedClipboardHistory,
  getClipboardAlwaysOnTop,
  listClipboardHistory,
  pasteClipboardItem,
  promoteClipboardToGallery,
  setClipboardAlwaysOnTop,
  togglePinClipboardItem,
} from "../lib/tauri-client";
import { useEditorStore } from "../stores/useEditorStore";
import type { ArrowAnnotation, PixelateAnnotation } from "../types/ipc";

describe("High-DPI Coordinate Normalization & Storage Math", () => {
  it("normalizes inverted drag coordinates and clamps to monitor bounds", () => {
    // Dragging from bottom-right (1500, 900) up to top-left (300, 200)
    const rect = normalizeCropRect(1500, 900, 300, 200, 1920, 1080);
    expect(rect).toEqual({
      x: 300,
      y: 200,
      width: 1200,
      height: 700,
    });

    // Out-of-bounds drag should clamp cleanly to [0..1920, 0..1080]
    const clamped = normalizeCropRect(-50, -20, 2500, 1400, 1920, 1080);
    expect(clamped).toEqual({
      x: 0,
      y: 0,
      width: 1920,
      height: 1080,
    });
  });

  it("scales logical WebView CSS pixels to physical monitor pixels on 150% DPI displays", () => {
    const logical = { x: 100, y: 200, width: 1280, height: 720 };
    const physical = toPhysicalRect(logical, 1.5);
    expect(physical).toEqual({
      x: 150,
      y: 300,
      width: 1920,
      height: 1080,
    });
  });

  it("formats storage byte sizes accurately", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024 * 500)).toBe("500.0 KB");
    expect(formatBytes(1024 * 1024 * 42.5)).toBe("42.5 MB");
  });
});

describe("Command Pattern Undo / Redo Stack (useEditorStore)", () => {
  beforeEach(() => {
    useEditorStore.getState().resetSession();
  });

  it("tracks annotation additions, supports multi-step Undo/Redo, and clears future on new branch", () => {
    const arrow1: ArrowAnnotation = {
      id: "a1",
      tool: "arrow",
      color: "#ef4444",
      strokeWidth: 4,
      points: [10, 10, 100, 100],
    };

    const redactBox: PixelateAnnotation = {
      id: "p1",
      tool: "pixelate",
      color: "#0f172a",
      strokeWidth: 4,
      x: 50,
      y: 50,
      width: 200,
      height: 40,
      blockSize: 12,
    };

    const store = useEditorStore.getState();
    store.addAnnotation(arrow1);
    store.addAnnotation(redactBox);

    expect(useEditorStore.getState().annotations).toHaveLength(2);
    expect(useEditorStore.getState().past).toHaveLength(2);

    // Undo once -> removes redactBox
    useEditorStore.getState().undo();
    expect(useEditorStore.getState().annotations).toHaveLength(1);
    expect(useEditorStore.getState().annotations[0].id).toBe("a1");
    expect(useEditorStore.getState().future).toHaveLength(1);

    // Redo -> restores redactBox
    useEditorStore.getState().redo();
    expect(useEditorStore.getState().annotations).toHaveLength(2);
    expect(useEditorStore.getState().annotations[1].id).toBe("p1");

    // Undo again and add a new annotation -> future branch should clear
    useEditorStore.getState().undo();
    store.addAnnotation({
      ...arrow1,
      id: "a2",
    });
    expect(useEditorStore.getState().future).toHaveLength(0);
    expect(useEditorStore.getState().annotations.map((a) => a.id)).toEqual([
      "a1",
      "a2",
    ]);
  });

  it("hydrates cross-session annotations from SQLite so Ctrl+Z / Ctrl+Y works immediately", () => {
    const savedShapes: ArrowAnnotation[] = [
      {
        id: "saved-1",
        tool: "arrow",
        color: "#3b82f6",
        strokeWidth: 4,
        points: [0, 0, 50, 50],
      },
      {
        id: "saved-2",
        tool: "arrow",
        color: "#10b981",
        strokeWidth: 4,
        points: [50, 50, 120, 120],
      },
    ];

    useEditorStore.getState().hydrateAnnotations(savedShapes, "arrow");
    expect(useEditorStore.getState().annotations).toHaveLength(2);
    expect(useEditorStore.getState().past).toHaveLength(2);

    // Pressing Ctrl+Z immediately undoes saved-2 from the previous session!
    useEditorStore.getState().undo();
    expect(useEditorStore.getState().annotations.map((a) => a.id)).toEqual([
      "saved-1",
    ]);

    // Pressing Ctrl+Y restores saved-2!
    useEditorStore.getState().redo();
    expect(useEditorStore.getState().annotations.map((a) => a.id)).toEqual([
      "saved-1",
      "saved-2",
    ]);
  });
});

describe("Phase 1.5.1 Visual Clipboard Drawer (Alt + V) Operations", () => {
  it("keeps pinned items on top, marks CURRENTLY COPIED item, remembers Always-on-Top, and promotes to gallery", async () => {
    // 1. Verify persistent Always-on-Top toggle
    expect(await getClipboardAlwaysOnTop()).toBe(true);
    await setClipboardAlwaysOnTop(false);
    expect(await getClipboardAlwaysOnTop()).toBe(false);
    await setClipboardAlwaysOnTop(true);

    const initial = await listClipboardHistory();
    expect(initial.length).toBeGreaterThanOrEqual(3);
    // Item #0 is pinned in seed data and stays on top
    expect(initial[0].isPinned).toBe(true);

    // 2. Paste an unpinned item; pinned item stays at index 0, while pasted unpinned item is marked CURRENTLY COPIED
    const unpinned = initial[initial.length - 1];
    await pasteClipboardItem(unpinned.id);
    const afterPaste = await listClipboardHistory();
    expect(afterPaste[0].isPinned).toBe(true);
    const currentlyCopied = afterPaste.find((i) => i.isCurrentlyCopied);
    expect(currentlyCopied?.id).toBe(unpinned.id);

    // 3. Pin the newly pasted item too and clear unpinned items
    await togglePinClipboardItem(unpinned.id, true);
    const clearedCount = await clearUnpinnedClipboardHistory();
    expect(clearedCount).toBeGreaterThanOrEqual(1);

    const remaining = await listClipboardHistory();
    expect(remaining.every((item) => item.isPinned)).toBe(true);

    // 4. Promote pinned clipboard item permanently to Gallery
    const promoted = await promoteClipboardToGallery(remaining[0].id);
    expect(promoted.tags).toContain("clipboard");
    expect(promoted.sourceApp).toBe("Visual Clipboard (Alt + V)");
  });
});
