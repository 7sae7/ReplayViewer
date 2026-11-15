import { create } from "zustand";
import type { AnnotationShape, AnnotationTool, CropRect } from "../types/ipc";

function transformShapeToNewBounds(
  shape: AnnotationShape,
  oldRect: CropRect,
  newRect: CropRect
): AnnotationShape {
  const sx = newRect.width / Math.max(1, oldRect.width);
  const sy = newRect.height / Math.max(1, oldRect.height);
  const scaleAvg = (sx + sy) / 2;

  const mapX = (x: number) => newRect.x + (x - oldRect.x) * sx;
  const mapY = (y: number) => newRect.y + (y - oldRect.y) * sy;

  if (shape.tool === "arrow") {
    return {
      ...shape,
      strokeWidth: Math.max(2, Math.round(shape.strokeWidth * scaleAvg)),
      points: [
        mapX(shape.points[0]),
        mapY(shape.points[1]),
        mapX(shape.points[2]),
        mapY(shape.points[3]),
      ],
    };
  }
  if (shape.tool === "rect") {
    return {
      ...shape,
      strokeWidth: Math.max(2, Math.round(shape.strokeWidth * scaleAvg)),
      x: mapX(shape.x),
      y: mapY(shape.y),
      width: shape.width * sx,
      height: shape.height * sy,
    };
  }
  if (shape.tool === "pen") {
    const nextPts: number[] = [];
    for (let i = 0; i < shape.points.length; i += 2) {
      nextPts.push(mapX(shape.points[i]), mapY(shape.points[i + 1]));
    }
    return {
      ...shape,
      strokeWidth: Math.max(2, Math.round(shape.strokeWidth * scaleAvg)),
      points: nextPts,
    };
  }
  if (shape.tool === "text") {
    return {
      ...shape,
      x: mapX(shape.x),
      y: mapY(shape.y),
      fontSize: Math.max(12, Math.round(shape.fontSize * scaleAvg)),
    };
  }
  if (shape.tool === "pixelate") {
    return {
      ...shape,
      x: mapX(shape.x),
      y: mapY(shape.y),
      width: shape.width * sx,
      height: shape.height * sy,
    };
  }
  return shape;
}

interface EditorState {
  activeTool: AnnotationTool;
  strokeColor: string;
  strokeWidth: number;
  cropRect: CropRect | null;

  // Command Pattern Undo / Redo History Stacks
  past: AnnotationShape[][];
  annotations: AnnotationShape[];
  future: AnnotationShape[][];

  // Actions
  setTool: (tool: AnnotationTool) => void;
  setStrokeColor: (color: string) => void;
  setStrokeWidth: (width: number) => void;
  setCropRect: (rect: CropRect | null) => void;

  addAnnotation: (shape: AnnotationShape) => void;
  updateAnnotation: (
    id: string,
    updater: (prev: AnnotationShape) => AnnotationShape
  ) => void;
  hydrateAnnotations: (
    shapes: AnnotationShape[],
    initialTool?: AnnotationTool
  ) => void;
  transformAnnotationsToBounds: (oldRect: CropRect, newRect: CropRect) => void;
  undo: () => void;
  redo: () => void;
  clearAnnotations: () => void;
  resetSession: () => void;
}

export const useEditorStore = create<EditorState>((set) => ({
  activeTool: "crop",
  strokeColor: "#ef4444",
  strokeWidth: 4,
  cropRect: null,

  past: [],
  annotations: [],
  future: [],

  setTool: (activeTool) => set({ activeTool }),
  setStrokeColor: (strokeColor) => set({ strokeColor }),
  setStrokeWidth: (strokeWidth) => set({ strokeWidth }),
  setCropRect: (cropRect) => set({ cropRect }),

  addAnnotation: (shape) =>
    set((state) => ({
      past: [...state.past, state.annotations],
      annotations: [...state.annotations, shape],
      future: [],
    })),

  updateAnnotation: (id, updater) =>
    set((state) => ({
      annotations: state.annotations.map((item) =>
        item.id === id ? updater(item) : item
      ),
    })),

  // Hydrates saved annotations from SQLite into the Undo stack so Ctrl+Z / Ctrl+Y
  // can immediately step backward and forward through previous session annotations!
  hydrateAnnotations: (shapes, initialTool = "arrow") =>
    set({
      activeTool: initialTool,
      past: shapes.map((_, idx) => shapes.slice(0, idx)),
      annotations: shapes,
      future: [],
    }),

  transformAnnotationsToBounds: (oldRect, newRect) =>
    set((state) => ({
      past: state.past.map((step) =>
        step.map((s) => transformShapeToNewBounds(s, oldRect, newRect))
      ),
      annotations: state.annotations.map((s) =>
        transformShapeToNewBounds(s, oldRect, newRect)
      ),
      future: state.future.map((step) =>
        step.map((s) => transformShapeToNewBounds(s, oldRect, newRect))
      ),
    })),

  undo: () =>
    set((state) => {
      if (state.past.length === 0) return state;
      const previous = state.past[state.past.length - 1];
      const newPast = state.past.slice(0, state.past.length - 1);
      return {
        past: newPast,
        annotations: previous,
        future: [state.annotations, ...state.future],
      };
    }),

  redo: () =>
    set((state) => {
      if (state.future.length === 0) return state;
      const next = state.future[0];
      const newFuture = state.future.slice(1);
      return {
        past: [...state.past, state.annotations],
        annotations: next,
        future: newFuture,
      };
    }),

  clearAnnotations: () =>
    set((state) => {
      if (state.annotations.length === 0) return state;
      return {
        past: [...state.past, state.annotations],
        annotations: [],
        future: [],
      };
    }),

  resetSession: () =>
    set({
      activeTool: "crop",
      cropRect: null,
      past: [],
      annotations: [],
      future: [],
    }),
}));
