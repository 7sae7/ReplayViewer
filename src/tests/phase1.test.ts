import { beforeEach, describe, expect, it } from "vitest";
import { formatBytes, normalizeCropRect, toPhysicalRect } from "../lib/dpi";
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
});
