import React from "react";
import {
  ArrowUpRight,
  Check,
  Crop,
  EyeOff,
  PenTool,
  Redo2,
  Square,
  Trash2,
  Type,
  Undo2,
  X,
} from "lucide-react";
import { useEditorStore } from "../../stores/useEditorStore";
import type { AnnotationTool } from "../../types/ipc";

interface FloatingToolbarProps {
  top: number;
  left: number;
  onSave: () => void;
  onCancel: () => void;
}

const COLORS = [
  "#ef4444", // Red
  "#f59e0b", // Amber
  "#10b981", // Emerald
  "#3b82f6", // Blue
  "#a855f7", // Purple
  "#ffffff", // White
];

const TOOLS: { id: AnnotationTool; label: string; icon: React.ReactNode }[] = [
  { id: "crop", label: "Select / Re-Crop Area", icon: <Crop className="w-4 h-4" /> },
  { id: "arrow", label: "Arrow", icon: <ArrowUpRight className="w-4 h-4" /> },
  { id: "rect", label: "Rectangle", icon: <Square className="w-4 h-4" /> },
  { id: "pen", label: "Freehand Pen", icon: <PenTool className="w-4 h-4" /> },
  { id: "text", label: "Text Label", icon: <Type className="w-4 h-4" /> },
  {
    id: "pixelate",
    label: "Redact / Pixelate",
    icon: <EyeOff className="w-4 h-4" />,
  },
];

export const FloatingToolbar: React.FC<FloatingToolbarProps> = ({
  top,
  left,
  onSave,
  onCancel,
}) => {
  const {
    activeTool,
    strokeColor,
    strokeWidth,
    past,
    future,
    annotations,
    setTool,
    setStrokeColor,
    setStrokeWidth,
    undo,
    redo,
    clearAnnotations,
  } = useEditorStore();

  return (
    <div
      style={{ top: Math.max(12, top), left: Math.max(12, left) }}
      className="fixed z-50 flex items-center gap-2 px-3 py-2 rounded-box bg-base-200/95 border border-base-300 shadow-2xl backdrop-blur-md text-base-content"
      onMouseDown={(e) => e.stopPropagation()}
    >
      {/* Annotation Tools Join Group */}
      <div className="join">
        {TOOLS.map((t) => (
          <div key={t.id} className="tooltip tooltip-bottom" data-tip={t.label}>
            <button
              type="button"
              onClick={() => setTool(t.id)}
              className={`btn btn-sm btn-square join-item ${
                activeTool === t.id ? "btn-primary" : "btn-ghost"
              }`}
            >
              {t.icon}
            </button>
          </div>
        ))}
      </div>

      <div className="divider divider-horizontal mx-0 my-1" />

      {/* Color Swatches */}
      <div className="flex items-center gap-1.5 px-1">
        {COLORS.map((c) => (
          <button
            key={c}
            type="button"
            onClick={() => setStrokeColor(c)}
            title={`Color ${c}`}
            style={{ backgroundColor: c }}
            className={`w-5 h-5 rounded-full transition-transform cursor-pointer ${
              strokeColor === c
                ? "scale-125 ring-2 ring-base-content ring-offset-2 ring-offset-base-200"
                : "opacity-75 hover:opacity-100"
            }`}
          />
        ))}
      </div>

      <div className="divider divider-horizontal mx-0 my-1" />

      {/* Stroke Width Join Group */}
      <div className="join">
        {[2, 4, 8].map((w) => (
          <button
            key={w}
            type="button"
            onClick={() => setStrokeWidth(w)}
            className={`btn btn-xs join-item font-mono ${
              strokeWidth === w ? "btn-neutral" : "btn-ghost"
            }`}
          >
            {w}px
          </button>
        ))}
      </div>

      <div className="divider divider-horizontal mx-0 my-1" />

      {/* Command Pattern Undo / Redo */}
      <div className="join">
        <div className="tooltip tooltip-bottom" data-tip="Undo (Ctrl+Z)">
          <button
            type="button"
            onClick={undo}
            disabled={past.length === 0}
            className="btn btn-sm btn-square btn-ghost join-item"
          >
            <Undo2 className="w-4 h-4" />
          </button>
        </div>
        <div className="tooltip tooltip-bottom" data-tip="Redo (Ctrl+Y)">
          <button
            type="button"
            onClick={redo}
            disabled={future.length === 0}
            className="btn btn-sm btn-square btn-ghost join-item"
          >
            <Redo2 className="w-4 h-4" />
          </button>
        </div>
        <div className="tooltip tooltip-bottom" data-tip="Clear Annotations">
          <button
            type="button"
            onClick={clearAnnotations}
            disabled={annotations.length === 0}
            className="btn btn-sm btn-square btn-ghost text-error join-item"
          >
            <Trash2 className="w-4 h-4" />
          </button>
        </div>
      </div>

      <div className="divider divider-horizontal mx-0 my-1" />

      {/* Cancel & Save Actions */}
      <div className="flex items-center gap-1.5">
        <button
          type="button"
          onClick={onCancel}
          className="btn btn-sm btn-ghost gap-1"
        >
          <X className="w-4 h-4" />
          <kbd className="kbd kbd-xs">Esc</kbd>
        </button>
        <button
          type="button"
          onClick={onSave}
          className="btn btn-sm btn-primary gap-1.5"
        >
          <Check className="w-4 h-4" />
          Copy & Save
          <kbd className="kbd kbd-xs text-base-content">Enter</kbd>
        </button>
      </div>
    </div>
  );
};
