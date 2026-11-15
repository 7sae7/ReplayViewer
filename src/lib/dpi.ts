import type { CropRect } from "../types/ipc";

/**
 * Normalizes a drag selection rectangle from two arbitrary mouse coordinates
 * so width and height are always positive and clamped to the viewport bounds.
 */
export function normalizeCropRect(
  startX: number,
  startY: number,
  endX: number,
  endY: number,
  maxWidth: number,
  maxHeight: number
): CropRect {
  const clampedStartX = Math.max(0, Math.min(startX, maxWidth));
  const clampedStartY = Math.max(0, Math.min(startY, maxHeight));
  const clampedEndX = Math.max(0, Math.min(endX, maxWidth));
  const clampedEndY = Math.max(0, Math.min(endY, maxHeight));

  const x = Math.min(clampedStartX, clampedEndX);
  const y = Math.min(clampedStartY, clampedEndY);
  const width = Math.abs(clampedEndX - clampedStartX);
  const height = Math.abs(clampedEndY - clampedStartY);

  return { x, y, width, height };
}

/**
 * Converts logical WebView CSS coordinates to physical monitor pixels using
 * the monitor's high-DPI scale factor (e.g., 1.25x, 1.5x, 2.0x) so cropped
 * screenshots retain 1:1 native pixel crispness.
 */
export function toPhysicalRect(logicalRect: CropRect, scaleFactor: number): CropRect {
  const safeScale = scaleFactor > 0 ? scaleFactor : 1;
  return {
    x: Math.round(logicalRect.x * safeScale),
    y: Math.round(logicalRect.y * safeScale),
    width: Math.max(1, Math.round(logicalRect.width * safeScale)),
    height: Math.max(1, Math.round(logicalRect.height * safeScale)),
  };
}

/**
 * Formats byte counts into human-readable storage strings (KB, MB, GB).
 */
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const k = 1024;
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1);
  const value = bytes / Math.pow(k, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}
