import React, { useCallback, useEffect, useRef, useState } from "react";
import { Arrow, Group, Image as KonvaImage, Layer, Line, Rect, Stage, Text } from "react-konva";
import type Konva from "konva";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { normalizeCropRect, toPhysicalRect } from "../../lib/dpi";
import {
  getFrozenCaptureUrl,
  getLatestCaptureMetaFallback,
  isTauriRuntime,
  saveAnnotatedCapture,
} from "./overlay-helpers";
import { useEditorStore } from "../../stores/useEditorStore";
import { useSettingsStore } from "../../stores/useSettingsStore";
import { FloatingToolbar } from "./FloatingToolbar";
import type { AnnotationShape, CaptureSessionMeta } from "../../types/ipc";

interface OverlayAppProps {
  onCloseInBrowserPreview?: () => void;
}

export const OverlayApp: React.FC<OverlayAppProps> = ({
  onCloseInBrowserPreview,
}) => {
  const {
    activeTool,
    strokeColor,
    strokeWidth,
    cropRect,
    annotations,
    setCropRect,
    addAnnotation,
    updateAnnotation,
    undo,
    redo,
    resetSession,
  } = useEditorStore();

  const { settings, loadSettings } = useSettingsStore();

  const [sessionMeta, setSessionMeta] = useState<CaptureSessionMeta>({
    sessionId: "initial",
    monitorId: 1,
    width: window.innerWidth || 1920,
    height: window.innerHeight || 1080,
    scaleFactor: window.devicePixelRatio || 1,
    capturedAt: new Date().toISOString(),
  });

  const [bgImage, setBgImage] = useState<HTMLImageElement | null>(null);
  const [viewport, setViewport] = useState({
    width: window.innerWidth || 1920,
    height: window.innerHeight || 1080,
  });

  // Interaction state for dragging crop or drawing shapes
  const [isDraggingCrop, setIsDraggingCrop] = useState(false);
  const [dragStart, setDragStart] = useState<{ x: number; y: number } | null>(null);
  const [drawingId, setDrawingId] = useState<string | null>(null);
  const [textPromptPos, setTextPromptPos] = useState<{ x: number; y: number } | null>(null);
  const [textPromptValue, setTextPromptValue] = useState("");

  const stageRef = useRef<Konva.Stage | null>(null);

  // Load frozen frame image into HTMLImageElement for Konva canvas.
  // Fix #1: Starts with NO area captured (`cropRect = null` via `resetSession()`).
  const loadFrozenFrame = useCallback(
    (meta: CaptureSessionMeta) => {
      setSessionMeta(meta);
      resetSession();
      setViewport({
        width: window.innerWidth || meta.width || 1920,
        height: window.innerHeight || meta.height || 1080,
      });

      const primaryUrl = getFrozenCaptureUrl(meta.sessionId);
      const img = new window.Image();
      img.crossOrigin = "anonymous";

      img.onload = () => {
        setBgImage(img);
        // Feature #1: Start with NO area captured so user drags their desired region first
      };

      img.onerror = () => {
        if (primaryUrl.startsWith("http://capture.localhost")) {
          img.src = `capture://localhost/current?session=${encodeURIComponent(
            meta.sessionId
          )}&t=${Date.now()}`;
        }
      };

      img.src = primaryUrl;
    },
    [resetSession]
  );

  useEffect(() => {
    loadSettings();
    getLatestCaptureMetaFallback().then((meta) => {
      loadFrozenFrame(meta);
    });

    const handleResize = () => {
      setViewport({ width: window.innerWidth, height: window.innerHeight });
    };
    window.addEventListener("resize", handleResize);

    let unlisten: (() => void) | undefined;
    if (isTauriRuntime()) {
      listen<CaptureSessionMeta>("capture:ready", (event) => {
        loadFrozenFrame(event.payload);
      }).then((fn) => {
        unlisten = fn;
      });
    }

    return () => {
      window.removeEventListener("resize", handleResize);
      if (unlisten) unlisten();
    };
  }, [loadFrozenFrame, loadSettings]);

  const handleCloseOverlay = useCallback(async () => {
    resetSession();
    if (isTauriRuntime()) {
      const win = getCurrentWindow();
      await win.setFullscreen(false);
      await win.hide();
    } else if (onCloseInBrowserPreview) {
      onCloseInBrowserPreview();
    }
  }, [onCloseInBrowserPreview, resetSession]);

  const handleSaveCapture = useCallback(async () => {
    if (!stageRef.current || !cropRect || cropRect.width < 5 || cropRect.height < 5) {
      return;
    }

    const scale = sessionMeta.scaleFactor || 1;
    const dataUrl = stageRef.current.toDataURL({
      x: cropRect.x,
      y: cropRect.y,
      width: Math.max(5, cropRect.width),
      height: Math.max(5, cropRect.height),
      pixelRatio: scale,
      mimeType: "image/png",
    });

    const base64Part = dataUrl.split(",")[1] || "";
    const binaryStr = window.atob(base64Part);
    const bytes = new Uint8Array(binaryStr.length);
    for (let i = 0; i < binaryStr.length; i++) {
      bytes[i] = binaryStr.charCodeAt(i);
    }

    const physical = toPhysicalRect(cropRect, scale);
    await saveAnnotatedCapture(
      bytes,
      settings?.copyToClipboardOnSave ?? true,
      settings?.imageFormat ?? "webp",
      dataUrl,
      { width: physical.width, height: physical.height }
    );

    if (!isTauriRuntime() && onCloseInBrowserPreview) {
      onCloseInBrowserPreview();
    }
  }, [
    cropRect,
    onCloseInBrowserPreview,
    sessionMeta.scaleFactor,
    settings?.copyToClipboardOnSave,
    settings?.imageFormat,
  ]);

  // Keyboard shortcuts: Escape, Enter, Ctrl+Z, Ctrl+Y
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (textPromptPos) return;
      if (e.key === "Escape") {
        e.preventDefault();
        handleCloseOverlay();
      } else if (e.key === "Enter") {
        e.preventDefault();
        handleSaveCapture();
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) {
          redo();
        } else {
          undo();
        }
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "y") {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleCloseOverlay, handleSaveCapture, redo, textPromptPos, undo]);

  const handleStageMouseDown = (e: Konva.KonvaEventObject<MouseEvent>) => {
    const pos = e.target.getStage()?.getPointerPosition();
    if (!pos) return;

    if (activeTool === "crop" || !cropRect) {
      setIsDraggingCrop(true);
      setDragStart({ x: pos.x, y: pos.y });
      setCropRect({ x: pos.x, y: pos.y, width: 1, height: 1 });
      return;
    }

    if (activeTool === "text") {
      setTextPromptPos({ x: pos.x, y: pos.y });
      setTextPromptValue("");
      return;
    }

    const id = `ann-${Date.now()}`;
    setDrawingId(id);
    setDragStart({ x: pos.x, y: pos.y });

    if (activeTool === "arrow") {
      addAnnotation({
        id,
        tool: "arrow",
        color: strokeColor,
        strokeWidth,
        points: [pos.x, pos.y, pos.x, pos.y],
      });
    } else if (activeTool === "rect") {
      addAnnotation({
        id,
        tool: "rect",
        color: strokeColor,
        strokeWidth,
        x: pos.x,
        y: pos.y,
        width: 1,
        height: 1,
      });
    } else if (activeTool === "pen") {
      addAnnotation({
        id,
        tool: "pen",
        color: strokeColor,
        strokeWidth,
        points: [pos.x, pos.y],
      });
    } else if (activeTool === "pixelate") {
      addAnnotation({
        id,
        tool: "pixelate",
        color: "#0f172a",
        strokeWidth,
        x: pos.x,
        y: pos.y,
        width: 1,
        height: 1,
        blockSize: 12,
      });
    }
  };

  const handleStageMouseMove = (e: Konva.KonvaEventObject<MouseEvent>) => {
    const pos = e.target.getStage()?.getPointerPosition();
    if (!pos) return;

    if (isDraggingCrop && dragStart) {
      const normalized = normalizeCropRect(
        dragStart.x,
        dragStart.y,
        pos.x,
        pos.y,
        viewport.width,
        viewport.height
      );
      setCropRect(normalized);
      return;
    }

    if (!drawingId || !dragStart) return;

    updateAnnotation(drawingId, (prev): AnnotationShape => {
      if (prev.tool === "arrow") {
        return {
          ...prev,
          points: [dragStart.x, dragStart.y, pos.x, pos.y],
        };
      }
      if (prev.tool === "rect") {
        const box = normalizeCropRect(
          dragStart.x,
          dragStart.y,
          pos.x,
          pos.y,
          viewport.width,
          viewport.height
        );
        return { ...prev, ...box };
      }
      if (prev.tool === "pen") {
        return {
          ...prev,
          points: [...prev.points, pos.x, pos.y],
        };
      }
      if (prev.tool === "pixelate") {
        const box = normalizeCropRect(
          dragStart.x,
          dragStart.y,
          pos.x,
          pos.y,
          viewport.width,
          viewport.height
        );
        return { ...prev, ...box };
      }
      return prev;
    });
  };

  // Fix #2: Do NOT auto-switch off capture (`crop`) mode after selecting an area!
  const handleStageMouseUp = () => {
    if (isDraggingCrop) {
      setIsDraggingCrop(false);
    }
    setDrawingId(null);
    setDragStart(null);
  };

  const commitTextAnnotation = () => {
    if (textPromptPos && textPromptValue.trim()) {
      addAnnotation({
        id: `txt-${Date.now()}`,
        tool: "text",
        color: strokeColor,
        strokeWidth,
        x: textPromptPos.x,
        y: textPromptPos.y,
        text: textPromptValue.trim(),
        fontSize: Math.max(18, strokeWidth * 5),
      });
    }
    setTextPromptPos(null);
    setTextPromptValue("");
  };

  const hasValidCrop = Boolean(cropRect && cropRect.width > 4 && cropRect.height > 4);
  const physicalRect = hasValidCrop && cropRect
    ? toPhysicalRect(cropRect, sessionMeta.scaleFactor)
    : null;

  return (
    <div className="relative w-screen h-screen overflow-hidden cursor-crosshair select-none bg-base-300">
      {/* Top Instruction Banner using daisyUI badge & kbd */}
      <div className="fixed top-4 left-1/2 -translate-x-1/2 z-40 flex items-center gap-2.5 px-4 py-2 rounded-box bg-base-200/95 border border-base-300 text-xs text-base-content shadow-2xl pointer-events-none">
        <span className="badge badge-primary badge-sm font-semibold">Alt + S</span>
        {physicalRect ? (
          <>
            <span>
              Selected Area:{" "}
              <strong className="font-mono">
                {physicalRect.width} × {physicalRect.height} px
              </strong>{" "}
              ({sessionMeta.scaleFactor.toFixed(2)}x DPI)
            </span>
            <span className="opacity-40">|</span>
            <span>Drag again to re-select or pick an annotation tool</span>
            <span className="opacity-40">|</span>
            <span className="flex items-center gap-1 text-success font-medium">
              <kbd className="kbd kbd-xs">Enter</kbd> Copy & Save
            </span>
          </>
        ) : (
          <>
            <span className="font-medium">
              Click and drag anywhere on screen to select capture area
            </span>
            <span className="opacity-40">|</span>
            <span className="flex items-center gap-1 opacity-75">
              <kbd className="kbd kbd-xs">Esc</kbd> Cancel
            </span>
          </>
        )}
      </div>

      {/* Main Konva Stage */}
      <Stage
        ref={stageRef}
        width={viewport.width}
        height={viewport.height}
        onMouseDown={handleStageMouseDown}
        onMouseMove={handleStageMouseMove}
        onMouseUp={handleStageMouseUp}
      >
        <Layer>
          {/* Frozen Background Frame */}
          {bgImage && (
            <KonvaImage
              image={bgImage}
              x={0}
              y={0}
              width={viewport.width}
              height={viewport.height}
            />
          )}

          {/* Annotations Layer (Exported inside crop region) */}
          {annotations.map((ann) => {
            if (ann.tool === "arrow") {
              return (
                <Arrow
                  key={ann.id}
                  points={ann.points}
                  stroke={ann.color}
                  fill={ann.color}
                  strokeWidth={ann.strokeWidth}
                  pointerLength={12}
                  pointerWidth={12}
                  lineCap="round"
                  lineJoin="round"
                />
              );
            }
            if (ann.tool === "rect") {
              return (
                <Rect
                  key={ann.id}
                  x={ann.x}
                  y={ann.y}
                  width={ann.width}
                  height={ann.height}
                  stroke={ann.color}
                  strokeWidth={ann.strokeWidth}
                  cornerRadius={4}
                />
              );
            }
            if (ann.tool === "pen") {
              return (
                <Line
                  key={ann.id}
                  points={ann.points}
                  stroke={ann.color}
                  strokeWidth={ann.strokeWidth}
                  tension={0.4}
                  lineCap="round"
                  lineJoin="round"
                />
              );
            }
            if (ann.tool === "text") {
              return (
                <Text
                  key={ann.id}
                  x={ann.x}
                  y={ann.y}
                  text={ann.text}
                  fill={ann.color}
                  fontSize={ann.fontSize}
                  fontStyle="bold"
                  shadowColor="#000000"
                  shadowBlur={4}
                  shadowOpacity={0.85}
                />
              );
            }
            if (ann.tool === "pixelate") {
              const cols = Math.max(1, Math.ceil(ann.width / ann.blockSize));
              const rows = Math.max(1, Math.ceil(ann.height / ann.blockSize));
              const blocks: React.ReactNode[] = [];
              for (let r = 0; r < rows; r++) {
                for (let c = 0; c < cols; c++) {
                  const isEven = (r + c) % 2 === 0;
                  blocks.push(
                    <Rect
                      key={`${ann.id}-${r}-${c}`}
                      x={ann.x + c * ann.blockSize}
                      y={ann.y + r * ann.blockSize}
                      width={Math.min(ann.blockSize, ann.width - c * ann.blockSize)}
                      height={Math.min(ann.blockSize, ann.height - r * ann.blockSize)}
                      fill={isEven ? "#0f172a" : "#1e293b"}
                    />
                  );
                }
              }
              return <Group key={ann.id}>{blocks}</Group>;
            }
            return null;
          })}
        </Layer>
      </Stage>

      {/* Initial Full-Screen Dim Mask when no area is captured yet */}
      {!hasValidCrop && (
        <div
          style={{ backgroundColor: "rgba(2, 6, 23, 0.45)" }}
          className="fixed inset-0 pointer-events-none z-20"
        />
      )}

      {/* CSS Dimming Mask & Crisp Selection Border once user starts dragging */}
      {hasValidCrop && cropRect && physicalRect && (
        <div
          style={{
            left: cropRect.x,
            top: cropRect.y,
            width: cropRect.width,
            height: cropRect.height,
            boxShadow: "0 0 0 9999px rgba(2, 6, 23, 0.55)",
          }}
          className="fixed pointer-events-none border-2 border-primary z-30"
        >
          {/* Live Physical Pixel Dimensions Badge */}
          <div className="absolute -top-7 left-0 badge badge-primary badge-sm font-mono font-semibold shadow">
            {physicalRect.width} × {physicalRect.height}
          </div>
        </div>
      )}

      {/* Inline Text Input Modal when adding Text Annotation */}
      {textPromptPos && (
        <div
          style={{ left: textPromptPos.x, top: textPromptPos.y }}
          className="fixed z-50 join bg-base-200 border border-primary rounded-box p-1.5 shadow-2xl"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <input
            type="text"
            autoFocus
            placeholder="Type annotation text..."
            value={textPromptValue}
            onChange={(e) => setTextPromptValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commitTextAnnotation();
              if (e.key === "Escape") setTextPromptPos(null);
            }}
            className="input input-sm join-item"
          />
          <button
            type="button"
            onClick={commitTextAnnotation}
            className="btn btn-sm btn-primary join-item"
          >
            Add
          </button>
        </div>
      )}

      {/* Contextual Floating Annotation Toolbar (Shown once an area is selected) */}
      {hasValidCrop && cropRect && !isDraggingCrop && (
        <FloatingToolbar
          top={
            cropRect.y + cropRect.height + 68 < viewport.height
              ? cropRect.y + cropRect.height + 12
              : Math.max(12, cropRect.y - 62)
          }
          left={Math.min(
            Math.max(12, cropRect.x),
            Math.max(12, viewport.width - 720)
          )}
          onSave={handleSaveCapture}
          onCancel={handleCloseOverlay}
        />
      )}
    </div>
  );
};
