import { create } from "zustand";
import type { AnnotationShape, AnnotationTool, CropRect } from "../types/ipc";

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
  updateAnnotation: (id: string, updater: (prev: AnnotationShape) => AnnotationShape) => void;
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
