export type MediaType = "screenshot" | "video";

export type SortByField = "created_at" | "size_bytes" | "last_copied_at";
export type SortDirection = "desc" | "asc";
export type DateRangeFilter = "all" | "today" | "7d" | "30d";

export interface CaptureRecord {
  id: string;
  mediaType: MediaType;
  filePath: string;
  thumbPath: string;
  width: number;
  height: number;
  sizeBytes: number;
  durationMs: number | null;
  sourceApp: string | null;
  ocrText: string;
  isPinned: boolean;
  groupName: string;
  lastCopiedAt: string | null;
  tags: string[];
  createdAt: string;
}

export interface GalleryFilter {
  searchQuery?: string;
  mediaType?: MediaType;
  onlyPinned: boolean;
  onlyRecentClipboard?: boolean;
  groupName?: string;
  tag?: string;
  dateRange?: DateRangeFilter;
  sortBy?: SortByField;
  sortDir?: SortDirection;
}

export interface PaginatedCaptures {
  items: CaptureRecord[];
  nextCursor: string | null;
  totalCount: number;
  totalBytes: number;
  groups: string[];
}

export interface UnindexedScanResult {
  folderPath: string;
  count: number;
  totalBytes: number;
  sampleNames: string[];
}

export interface ClipboardItem {
  id: string;
  blake3Hash: string;
  filePath: string;
  thumbPath: string;
  baseFilePath: string | null;
  annotationsJson: string;
  width: number;
  height: number;
  sizeBytes: number;
  isPinned: boolean;
  isCurrentlyCopied: boolean;
  copiedAt: string;
}

export interface CaptureSessionMeta {
  sessionId: string;
  monitorId: number;
  width: number;
  height: number;
  scaleFactor: number;
  capturedAt: string;
  sourceClipboardId?: string | null;
  initialAnnotationsJson?: string | null;
}

export interface SettingsPayload {
  hotkeyRegionCapture: string;
  hotkeyFullscreenCapture: string;
  hotkeyOpenGallery: string;
  hotkeyVisualClipboard: string;
  hotkeySaveReplay: string;
  imageFormat: "webp" | "png";
  copyToClipboardOnSave: boolean;
  maxStorageGb: number;
  autoCleanupDays: number;
  clipboardMaxItems: number;
  launchOnStartup: boolean;
  capturesDir: string;
}

export type AnnotationTool =
  | "crop"
  | "arrow"
  | "rect"
  | "pen"
  | "text"
  | "pixelate";

export interface CropRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface BaseAnnotation {
  id: string;
  tool: Exclude<AnnotationTool, "crop">;
  color: string;
  strokeWidth: number;
}

export interface ArrowAnnotation extends BaseAnnotation {
  tool: "arrow";
  points: [number, number, number, number];
}

export interface RectAnnotation extends BaseAnnotation {
  tool: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PenAnnotation extends BaseAnnotation {
  tool: "pen";
  points: number[];
}

export interface TextAnnotation extends BaseAnnotation {
  tool: "text";
  x: number;
  y: number;
  text: string;
  fontSize: number;
}

export interface PixelateAnnotation extends BaseAnnotation {
  tool: "pixelate";
  x: number;
  y: number;
  width: number;
  height: number;
  blockSize: number;
}

export type AnnotationShape =
  | ArrowAnnotation
  | RectAnnotation
  | PenAnnotation
  | TextAnnotation
  | PixelateAnnotation;
