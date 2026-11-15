import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  Arrow,
  Group,
  Image as KonvaImage,
  Layer,
  Line,
  Rect,
  Stage,
  Text,
} from "react-konva";
import type Konva from "konva";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Maximize2, Minimize2, ZoomIn } from "lucide-react";
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
import type {
  AnnotationShape,
  CaptureSessionMeta,
  CropRect,
} from "../../types/ipc";

interface OverlayAppProps {
  onCloseInBrowserPreview?: () => void;
}

type ResizeCorner = "nw" | "ne" | "sw" | "se";

function dataUrlToUint8Array(dataUrl: string): Uint8Array {
  const base64Part = dataUrl.split(",")[1] || "";
  const binaryStr = window.atob(base64Part);
  const bytes = new Uint8Array(binaryStr.length);
  for (let i = 0; i < binaryStr.length; i++) {
    bytes[i] = binaryStr.charCodeAt(i);
  }
  return bytes;
}

function shiftShapeToLocalCrop(
  shape: AnnotationShape,
  crop: CropRect
): AnnotationShape {
  const dx = -crop.x;
  const dy = -crop.y;
  if (shape.tool === "arrow") {
    return {
      ...shape,
      points: [
        shape.points[0] + dx,
        shape.points[1] + dy,
        shape.points[2] + dx,
        shape.points[3] + dy,
      ],
    };
  }
  if (shape.tool === "rect" || shape.tool === "pixelate") {
    return {
      ...shape,
      x: shape.x + dx,
      y: shape.y + dy,
    };
  }
  if (shape.tool === "text") {
    return {
      ...shape,
      x: shape.x + dx,
      y: shape.y + dy,
    };
  }
  if (shape.tool === "pen") {
    const pts: number[] = [];
    for (let i = 0; i < shape.points.length; i += 2) {
      pts.push(shape.points[i] + dx, shape.points[i + 1] + dy);
    }
    return {
      ...shape,
      points: pts,
    };
  }
  return shape;
}

function shiftShapeFromLocalToBounds(
  shape: AnnotationShape,
  bounds: CropRect
): AnnotationShape {
  return shiftShapeToLocalCrop(shape, {
    x: -bounds.x,
    y: -bounds.y,
    width: bounds.width,
    height: bounds.height,
  });
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
    hydrateAnnotations,
    transformAnnotationsToBounds,
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
    sourceClipboardId: null,
    initialAnnotationsJson: null,
  });

  const [bgImage, setBgImage] = useState<HTMLImageElement | null>(null);
  const [viewport, setViewport] = useState({
    width: window.innerWidth || 1920,
    height: window.innerHeight || 1080,
  });

  // For Clipboard Image Edit Mode (Alt+V -> Annotate): tracks centered image bounds at 1:1 or scaled size
  const [imageBounds, setImageBounds] = useState<CropRect | null>(null);
  const [resizingCorner, setResizingCorner] = useState<ResizeCorner | null>(
    null
  );

  // Interaction state for dragging crop or drawing shapes
  const [isDraggingCrop, setIsDraggingCrop] = useState(false);
  const [dragStart, setDragStart] = useState<{ x: number; y: number } | null>(
    null
  );
  const [drawingId, setDrawingId] = useState<string | null>(null);
  const [textPromptPos, setTextPromptPos] = useState<{
    x: number;
    y: number;
  } | null>(null);
  const [textPromptValue, setTextPromptValue] = useState("");

  const stageRef = useRef<Konva.Stage | null>(null);
  const nonDestructiveGroupRef = useRef<Konva.Group | null>(null);

  const isClipboardEditMode = Boolean(sessionMeta.sourceClipboardId);

  const computeCenteredBounds = useCallback(
    (
      imgW: number,
      imgH: number,
      viewW: number,
      viewH: number,
      scaleMultiplier = 1
    ): CropRect => {
      const maxW = Math.max(100, viewW * 0.84);
      const maxH = Math.max(100, viewH * 0.78);
      let targetW = Math.max(1, Math.round(imgW * scaleMultiplier));
      let targetH = Math.max(1, Math.round(imgH * scaleMultiplier));

      if (targetW > maxW || targetH > maxH) {
        const fitRatio = Math.min(maxW / targetW, maxH / targetH);
        targetW = Math.max(1, Math.round(targetW * fitRatio));
        targetH = Math.max(1, Math.round(targetH * fitRatio));
      }

      const x = Math.round((viewW - targetW) / 2);
      const y = Math.round((viewH - targetH) / 2);
      return { x, y, width: targetW, height: targetH };
    },
    []
  );

  // Load frozen frame image into HTMLImageElement for Konva canvas.
  const loadFrozenFrame = useCallback(
    (meta: CaptureSessionMeta) => {
      setSessionMeta(meta);
      resetSession();

      const viewW = window.innerWidth || 1920;
      const viewH = window.innerHeight || 1080;
      setViewport({ width: viewW, height: viewH });

      const isClipEdit = Boolean(meta.sourceClipboardId);
      if (isClipEdit) {
        // Fix #6: Center clipboard image at 1:1 actual pixel dimensions & auto-select cropRect
        // so saving works immediately and tiny images don't stretch to fill the whole screen!
        const bounds = computeCenteredBounds(
          meta.width || 400,
          meta.height || 300,
          viewW,
          viewH,
          1
        );
        setImageBounds(bounds);
        setCropRect(bounds);

        // Hydrate cross-session Ctrl+Z / Ctrl+Y vector history if present
        let restored: AnnotationShape[] = [];
        if (meta.initialAnnotationsJson) {
          try {
            const parsed = JSON.parse(meta.initialAnnotationsJson);
            if (Array.isArray(parsed)) {
              restored = (parsed as AnnotationShape[]).map((s) =>
                shiftShapeFromLocalToBounds(s, bounds)
              );
            }
          } catch {
            restored = [];
          }
        }
        hydrateAnnotations(restored, "arrow");
      } else {
        // Alt + S Fullscreen Region Capture: starts with NO area selected (cropRect = null)
        setImageBounds(null);
      }

      const primaryUrl = getFrozenCaptureUrl(meta.sessionId);
      const img = new window.Image();
      img.crossOrigin = "anonymous";

      img.onload = () => {
        setBgImage(img);
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
    [computeCenteredBounds, hydrateAnnotations, resetSession, setCropRect]
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

  // Scale / Expand helper for Clipboard Edit Mode (1:1, 2x, 4x, Fit to Screen)
  const applyClipboardImageScale = useCallback(
    (mode: "1x" | "2x" | "4x" | "fit") => {
      if (!imageBounds) return;
      const origW = Math.max(1, sessionMeta.width);
      const origH = Math.max(1, sessionMeta.height);

      let nextBounds: CropRect;
      if (mode === "fit") {
        const maxW = Math.round(viewport.width * 0.78);
        const maxH = Math.round(viewport.height * 0.72);
        const ratio = Math.min(maxW / origW, maxH / origH);
        const w = Math.max(24, Math.round(origW * ratio));
        const h = Math.max(24, Math.round(origH * ratio));
        nextBounds = {
          x: Math.round((viewport.width - w) / 2),
          y: Math.round((viewport.height - h) / 2),
          width: w,
          height: h,
        };
      } else {
        const mult = mode === "2x" ? 2 : mode === "4x" ? 4 : 1;
        nextBounds = computeCenteredBounds(
          origW,
          origH,
          viewport.width,
          viewport.height,
          mult
        );
      }

      transformAnnotationsToBounds(imageBounds, nextBounds);
      setImageBounds(nextBounds);
      setCropRect(nextBounds);
    },
    [
      computeCenteredBounds,
      imageBounds,
      sessionMeta.height,
      sessionMeta.width,
      setCropRect,
      transformAnnotationsToBounds,
      viewport.height,
      viewport.width,
    ]
  );

  // Corner handle dragging to expand/resize clipboard image bounds interactively
  useEffect(() => {
    if (!resizingCorner || !imageBounds) return;

    const onMouseMove = (e: MouseEvent) => {
      const minSize = 16;
      let nextX = imageBounds.x;
      let nextY = imageBounds.y;
      let nextW = imageBounds.width;
      let nextH = imageBounds.height;

      if (resizingCorner === "se") {
        nextW = Math.max(minSize, e.clientX - imageBounds.x);
        nextH = Math.max(minSize, e.clientY - imageBounds.y);
      } else if (resizingCorner === "sw") {
        const right = imageBounds.x + imageBounds.width;
        nextX = Math.min(right - minSize, e.clientX);
        nextW = right - nextX;
        nextH = Math.max(minSize, e.clientY - imageBounds.y);
      } else if (resizingCorner === "ne") {
        const bottom = imageBounds.y + imageBounds.height;
        nextY = Math.min(bottom - minSize, e.clientY);
        nextW = Math.max(minSize, e.clientX - imageBounds.x);
        nextH = bottom - nextY;
      } else if (resizingCorner === "nw") {
        const right = imageBounds.x + imageBounds.width;
        const bottom = imageBounds.y + imageBounds.height;
        nextX = Math.min(right - minSize, e.clientX);
        nextY = Math.min(bottom - minSize, e.clientY);
        nextW = right - nextX;
        nextH = bottom - nextY;
      }

      const nextBounds: CropRect = {
        x: Math.round(nextX),
        y: Math.round(nextY),
        width: Math.round(nextW),
        height: Math.round(nextH),
      };
      transformAnnotationsToBounds(imageBounds, nextBounds);
      setImageBounds(nextBounds);
      setCropRect(nextBounds);
    };

    const onMouseUp = () => {
      setResizingCorner(null);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };
  }, [imageBounds, resizingCorner, setCropRect, transformAnnotationsToBounds]);

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
    const activeRect = cropRect || imageBounds;
    if (
      !stageRef.current ||
      !activeRect ||
      activeRect.width < 1 ||
      activeRect.height < 1
    ) {
      return;
    }

    const scale = sessionMeta.scaleFactor || 1;
    const exportConfig = {
      x: activeRect.x,
      y: activeRect.y,
      width: Math.max(1, activeRect.width),
      height: Math.max(1, activeRect.height),
      pixelRatio: scale,
      mimeType: "image/png",
    };

    // 1. Export flattened image (with all annotations visible)
    const flattenedDataUrl = stageRef.current.toDataURL(exportConfig);
    const flattenedBytes = dataUrlToUint8Array(flattenedDataUrl);

    // 2. Export clean base image (with only destructive Pixelate redactions visible)
    //    + serialize non-destructive vector annotations for cross-session Ctrl+Z / Ctrl+Y!
    const nonDestructiveShapes = annotations
      .filter((a) => a.tool !== "pixelate")
      .map((a) => shiftShapeToLocalCrop(a, activeRect));
    const annotationsJson = JSON.stringify(nonDestructiveShapes);

    let baseImageBytes: Uint8Array | undefined;
    if (nonDestructiveGroupRef.current) {
      nonDestructiveGroupRef.current.visible(false);
      stageRef.current.batchDraw();
      const baseDataUrl = stageRef.current.toDataURL(exportConfig);
      nonDestructiveGroupRef.current.visible(true);
      stageRef.current.batchDraw();
      baseImageBytes = dataUrlToUint8Array(baseDataUrl);
    }

    const physical = toPhysicalRect(activeRect, scale);
    await saveAnnotatedCapture(
      flattenedBytes,
      settings?.copyToClipboardOnSave ?? true,
      settings?.imageFormat ?? "webp",
      flattenedDataUrl,
      {
        width: Math.max(1, physical.width),
        height: Math.max(1, physical.height),
      },
      {
        baseImageBytes,
        annotationsJson,
        sourceClipboardId: sessionMeta.sourceClipboardId,
      }
    );

    if (!isTauriRuntime() && onCloseInBrowserPreview) {
      onCloseInBrowserPreview();
    }
  }, [
    annotations,
    cropRect,
    imageBounds,
    onCloseInBrowserPreview,
    sessionMeta.scaleFactor,
    sessionMeta.sourceClipboardId,
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

    if (activeTool === "crop" || (!cropRect && !imageBounds)) {
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

  const effectiveRect = cropRect || imageBounds;
  const hasValidCrop = Boolean(
    effectiveRect && effectiveRect.width >= 1 && effectiveRect.height >= 1
  );
  const physicalRect =
    hasValidCrop && effectiveRect
      ? toPhysicalRect(effectiveRect, sessionMeta.scaleFactor)
      : null;

  const pixelateAnnotations = annotations.filter((a) => a.tool === "pixelate");
  const vectorAnnotations = annotations.filter((a) => a.tool !== "pixelate");

  return (
    <div className="relative w-screen h-screen overflow-hidden cursor-crosshair select-none bg-base-300">
      {/* Top Instruction & Scale Banner */}
      <div className="fixed top-4 left-1/2 -translate-x-1/2 z-40 flex items-center gap-2.5 px-4 py-2 rounded-box bg-base-200/95 border border-base-300 text-xs text-base-content shadow-2xl">
        <span className="badge badge-primary badge-sm font-semibold">
          {isClipboardEditMode ? "Alt + V Annotate" : "Alt + S"}
        </span>
        {physicalRect ? (
          <>
            <span>
              Size:{" "}
              <strong className="font-mono">
                {Math.max(1, physicalRect.width)} ×{" "}
                {Math.max(1, physicalRect.height)} px
              </strong>
            </span>
            {isClipboardEditMode && (
              <>
                <span className="opacity-40">|</span>
                <div className="join">
                  <button
                    type="button"
                    onClick={() => applyClipboardImageScale("1x")}
                    className="btn btn-xs btn-ghost join-item font-mono"
                    title="Reset to 1:1 actual pixel dimensions"
                  >
                    <Minimize2 className="w-3 h-3 mr-1" /> 1:1
                  </button>
                  <button
                    type="button"
                    onClick={() => applyClipboardImageScale("2x")}
                    className="btn btn-xs btn-ghost join-item font-mono"
                    title="Expand image 200%"
                  >
                    <ZoomIn className="w-3 h-3 mr-1" /> 2×
                  </button>
                  <button
                    type="button"
                    onClick={() => applyClipboardImageScale("4x")}
                    className="btn btn-xs btn-ghost join-item font-mono"
                    title="Expand small image 400%"
                  >
                    4×
                  </button>
                  <button
                    type="button"
                    onClick={() => applyClipboardImageScale("fit")}
                    className="btn btn-xs btn-ghost join-item"
                    title="Expand to fit screen"
                  >
                    <Maximize2 className="w-3 h-3 mr-1" /> Fit Screen
                  </button>
                </div>
                <span className="text-[11px] opacity-70">
                  (Or drag corner handles)
                </span>
              </>
            )}
            <span className="opacity-40">|</span>
            <span className="flex items-center gap-1 text-success font-medium">
              <kbd className="kbd kbd-xs">Enter</kbd> Save & Copy
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
          {/* Dark Checker/Canvas backdrop when editing a centered clipboard image */}
          {isClipboardEditMode && (
            <Rect
              x={0}
              y={0}
              width={viewport.width}
              height={viewport.height}
              fill="#090d16"
            />
          )}

          {/* Frozen Background Frame: 1:1 Centered for Clipboard Edit, or Fullscreen for Alt+S */}
          {bgImage && (
            <KonvaImage
              image={bgImage}
              x={imageBounds ? imageBounds.x : 0}
              y={imageBounds ? imageBounds.y : 0}
              width={imageBounds ? imageBounds.width : viewport.width}
              height={imageBounds ? imageBounds.height : viewport.height}
            />
          )}

          {/* Destructive Pixelate Redactions Layer (Always baked into base image for security) */}
          <Group>
            {pixelateAnnotations.map((ann) => {
              if (ann.tool !== "pixelate") return null;
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
                      width={Math.min(
                        ann.blockSize,
                        ann.width - c * ann.blockSize
                      )}
                      height={Math.min(
                        ann.blockSize,
                        ann.height - r * ann.blockSize
                      )}
                      fill={isEven ? "#0f172a" : "#1e293b"}
                    />
                  );
                }
              }
              return <Group key={ann.id}>{blocks}</Group>;
            })}
          </Group>

          {/* Non-Destructive Vector Annotations Layer (Preserved for Cross-Session Ctrl+Z / Ctrl+Y) */}
          <Group ref={nonDestructiveGroupRef}>
            {vectorAnnotations.map((ann) => {
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
              return null;
            })}
          </Group>
        </Layer>
      </Stage>

      {/* Initial Full-Screen Dim Mask when no area is captured yet */}
      {!hasValidCrop && (
        <div
          style={{ backgroundColor: "rgba(2, 6, 23, 0.45)" }}
          className="fixed inset-0 pointer-events-none z-20"
        />
      )}

      {/* CSS Dimming Mask, Crisp Selection Border & Draggable Corner Handles */}
      {hasValidCrop && effectiveRect && physicalRect && (
        <div
          style={{
            left: effectiveRect.x,
            top: effectiveRect.y,
            width: effectiveRect.width,
            height: effectiveRect.height,
            boxShadow: "0 0 0 9999px rgba(2, 6, 23, 0.55)",
          }}
          className="fixed pointer-events-none border-2 border-primary z-30"
        >
          {/* Live Physical Pixel Dimensions Badge */}
          <div className="absolute -top-7 left-0 badge badge-primary badge-sm font-mono font-semibold shadow whitespace-nowrap">
            {Math.max(1, physicalRect.width)} ×{" "}
            {Math.max(1, physicalRect.height)}
          </div>

          {/* 4 Interactive Corner Resize Handles when editing a Clipboard Image */}
          {isClipboardEditMode && (
            <>
              <div
                onMouseDown={(e) => {
                  e.stopPropagation();
                  setResizingCorner("nw");
                }}
                title="Drag corner to expand/resize image"
                className="pointer-events-auto absolute -top-2 -left-2 w-4 h-4 rounded-full bg-primary border-2 border-base-100 cursor-nwse-resize shadow"
              />
              <div
                onMouseDown={(e) => {
                  e.stopPropagation();
                  setResizingCorner("ne");
                }}
                title="Drag corner to expand/resize image"
                className="pointer-events-auto absolute -top-2 -right-2 w-4 h-4 rounded-full bg-primary border-2 border-base-100 cursor-nesw-resize shadow"
              />
              <div
                onMouseDown={(e) => {
                  e.stopPropagation();
                  setResizingCorner("sw");
                }}
                title="Drag corner to expand/resize image"
                className="pointer-events-auto absolute -bottom-2 -left-2 w-4 h-4 rounded-full bg-primary border-2 border-base-100 cursor-nesw-resize shadow"
              />
              <div
                onMouseDown={(e) => {
                  e.stopPropagation();
                  setResizingCorner("se");
                }}
                title="Drag corner to expand/resize image"
                className="pointer-events-auto absolute -bottom-2 -right-2 w-4 h-4 rounded-full bg-primary border-2 border-base-100 cursor-nwse-resize shadow"
              />
            </>
          )}
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

      {/* Contextual Floating Annotation Toolbar (Shown once an area is selected or in Clipboard Edit Mode) */}
      {hasValidCrop && effectiveRect && !isDraggingCrop && (
        <FloatingToolbar
          top={
            effectiveRect.y + effectiveRect.height + 68 < viewport.height
              ? effectiveRect.y + effectiveRect.height + 12
              : Math.max(12, effectiveRect.y - 62)
          }
          left={Math.min(
            Math.max(12, effectiveRect.x),
            Math.max(12, viewport.width - 720)
          )}
          onSave={handleSaveCapture}
          onCancel={handleCloseOverlay}
        />
      )}
    </div>
  );
};
