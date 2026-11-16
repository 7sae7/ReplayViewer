import React, { useEffect, useRef, useState } from "react";
import {
  Check,
  Clock,
  Copy,
  FileText,
  FolderOpen,
  Pin,
  Plus,
  RotateCcw,
  Scissors,
  Tag,
  Trash2,
  X,
} from "lucide-react";
import type { CaptureRecord } from "../../types/ipc";
import { VideoTrimmerPanel } from "./VideoTrimmerPanel";
import {
  formatBytes,
  formatDeletionCountdown,
  formatDurationMs,
} from "../../lib/dpi";
import {
  copyCaptureToClipboard,
  openInFileExplorer,
  resolveMediaSrc,
} from "../../lib/tauri-client";

interface MediaLightboxProps {
  capture: CaptureRecord;
  onClose: () => void;
  onTogglePin: (id: string, currentPinned: boolean) => void;
  onDelete: (id: string) => void;
  onAddTag: (captureId: string, tag: string) => Promise<void>;
  onRestoreQueued?: (id: string, alsoPin?: boolean) => void;
  onVideoTrimmed?: (trimmed: CaptureRecord, replacedId?: string) => void;
}

export const MediaLightbox: React.FC<MediaLightboxProps> = ({
  capture,
  onClose,
  onTogglePin,
  onDelete,
  onAddTag,
  onRestoreQueued,
  onVideoTrimmed,
}) => {
  const [newTag, setNewTag] = useState("");
  const [copied, setCopied] = useState(false);
  const [copiedOcr, setCopiedOcr] = useState(false);
  const [showTrimmer, setShowTrimmer] = useState(false);
  const [currentTimeMs, setCurrentTimeMs] = useState(0);
  const [videoDurationMs, setVideoDurationMs] = useState(capture.durationMs ?? 0);
  const [isPlaying, setIsPlaying] = useState(false);
  const videoRef = useRef<HTMLVideoElement | null>(null);

  useEffect(() => {
    setCurrentTimeMs(0);
    setVideoDurationMs(capture.durationMs ?? 0);
    setIsPlaying(false);
  }, [capture.id, capture.durationMs]);

  const handleAddTag = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!newTag.trim()) return;
    await onAddTag(capture.id, newTag.trim());
    setNewTag("");
  };

  const handleCopy = async () => {
    await copyCaptureToClipboard(capture.filePath);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const handleCopyOcrText = async () => {
    if (!capture.ocrText) return;
    try {
      await navigator.clipboard.writeText(capture.ocrText);
      setCopiedOcr(true);
      setTimeout(() => setCopiedOcr(false), 1500);
    } catch {
      // Ignore if clipboard API blocked
    }
  };

  const handleOpenExplorer = async () => {
    await openInFileExplorer(capture.filePath);
  };

  return (
    <dialog className="modal modal-open" onClick={onClose}>
      <div
        className="modal-box max-w-6xl p-0 bg-base-100 border border-base-300 shadow-2xl overflow-hidden flex flex-col lg:flex-row max-h-[90vh]"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Full-Resolution Image or Native HTML5 Video Player */}
        <div className="flex-1 bg-base-300 flex flex-col items-center justify-center p-4 min-h-[380px] overflow-y-auto">
          {capture.mediaType === "video" ? (
            <>
              <video
                key={capture.id}
                ref={videoRef}
                src={resolveMediaSrc(capture.filePath)}
                poster={resolveMediaSrc(capture.thumbPath)}
                controls
                autoPlay
                onLoadedMetadata={(e) => {
                  const dur = e.currentTarget.duration;
                  if (Number.isFinite(dur) && dur > 0) {
                    setVideoDurationMs(Math.round(dur * 1000));
                  }
                }}
                onTimeUpdate={(e) => {
                  setCurrentTimeMs(Math.round(e.currentTarget.currentTime * 1000));
                }}
                onPlay={() => setIsPlaying(true)}
                onPause={() => setIsPlaying(false)}
                className={`max-w-full rounded-lg shadow-lg bg-black ${
                  showTrimmer ? "max-h-[54vh]" : "max-h-[74vh]"
                }`}
              />
              {showTrimmer && (
                <VideoTrimmerPanel
                  capture={capture}
                  videoRef={videoRef}
                  currentTimeMs={currentTimeMs}
                  durationMs={videoDurationMs}
                  isPlaying={isPlaying}
                  onTrimmed={(trimmed, replacedId) => {
                    setShowTrimmer(false);
                    onVideoTrimmed?.(trimmed, replacedId);
                  }}
                />
              )}
            </>
          ) : (
            <img
              src={resolveMediaSrc(capture.filePath)}
              alt={capture.sourceApp || "Capture"}
              className="max-w-full max-h-[78vh] object-contain rounded-lg shadow-lg"
            />
          )}
        </div>

        {/* Right Details & Actions Sidebar */}
        <div className="w-full lg:w-92 border-t lg:border-t-0 lg:border-l border-base-300 p-5 flex flex-col justify-between bg-base-200 overflow-y-auto">
          <div className="space-y-4">
            <div className="flex items-start justify-between gap-2">
              <div>
                <h3 className="text-base font-bold text-base-content">
                  {capture.sourceApp || "Region Capture"}
                </h3>
                <p className="text-xs text-base-content/60 mt-0.5">
                  {new Date(capture.createdAt).toLocaleString()}
                </p>
              </div>
              <button
                type="button"
                onClick={onClose}
                className="btn btn-sm btn-circle btn-ghost"
              >
                <X className="w-4 h-4" />
              </button>
            </div>

            {/* Queued Deletion 3-Day Grace Period Banner */}
            {capture.queuedDeleteAt && (
              <div
                role="alert"
                className="alert alert-warning alert-soft border border-warning/40 p-3 flex flex-col items-start gap-2 text-xs"
              >
                <div className="flex items-center gap-1.5 font-bold text-warning">
                  <Clock className="w-4 h-4 shrink-0" />
                  <span>
                    Queued for Auto-Deletion (
                    {formatDeletionCountdown(capture.queuedDeleteAt).countdown})
                  </span>
                </div>
                <p className="text-[11px] text-base-content/80 font-mono">
                  Deletes: {new Date(capture.queuedDeleteAt).toLocaleString()}
                </p>
                {onRestoreQueued && (
                  <div className="flex items-center gap-1.5 w-full pt-1">
                    <button
                      type="button"
                      onClick={() => onRestoreQueued(capture.id, false)}
                      className="btn btn-xs btn-success flex-1 gap-1"
                    >
                      <RotateCcw className="w-3 h-3" />
                      Restore
                    </button>
                    <button
                      type="button"
                      onClick={() => onRestoreQueued(capture.id, true)}
                      className="btn btn-xs btn-warning flex-1 gap-1"
                    >
                      <Pin className="w-3 h-3" />
                      Pin &amp; Restore
                    </button>
                  </div>
                )}
              </div>
            )}

            {/* Specs Card */}
            <div className="card card-xs bg-base-100 border border-base-300">
              <div className="card-body grid grid-cols-2 gap-2 text-xs">
                <div>
                  <span className="text-base-content/50 block">Resolution</span>
                  <span className="font-mono text-base-content font-semibold">
                    {capture.width} × {capture.height}
                  </span>
                </div>
                <div>
                  <span className="text-base-content/50 block">File Size</span>
                  <span className="font-mono text-base-content font-semibold">
                    {formatBytes(capture.sizeBytes)}
                    {capture.mediaType === "video" && capture.durationMs
                      ? ` • ${formatDurationMs(capture.durationMs)}`
                      : ""}
                  </span>
                </div>
                <div className="col-span-2 pt-2 border-t border-base-300">
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-base-content/50">Disk Path</span>
                    <button
                      type="button"
                      onClick={handleOpenExplorer}
                      className="btn btn-xs btn-link p-0 h-auto min-h-0 text-primary no-underline hover:underline gap-1"
                    >
                      <FolderOpen className="w-3 h-3" />
                      Show in Explorer
                    </button>
                  </div>
                  <span className="font-mono text-[11px] text-base-content/70 break-all line-clamp-2">
                    {capture.filePath}
                  </span>
                </div>
              </div>
            </div>

            {/* Tags Section (Indexed in FTS5) */}
            <div>
              <label className="text-xs font-semibold text-base-content/80 flex items-center gap-1.5 mb-2">
                <Tag className="w-3.5 h-3.5 text-primary" />
                Searchable FTS5 Tags
              </label>
              <div className="flex flex-wrap gap-1.5 mb-2.5">
                {capture.tags.map((t) => (
                  <span
                    key={t}
                    className="badge badge-soft badge-primary badge-sm font-medium"
                  >
                    #{t}
                  </span>
                ))}
                {capture.tags.length === 0 && (
                  <span className="text-xs text-base-content/50 italic">
                    No tags yet
                  </span>
                )}
              </div>
              <form onSubmit={handleAddTag} className="join w-full">
                <input
                  type="text"
                  placeholder="Add tag (e.g. bug, valorant)..."
                  value={newTag}
                  onChange={(e) => setNewTag(e.target.value)}
                  className="input input-sm input-bordered join-item flex-1 text-xs"
                />
                <button
                  type="submit"
                  className="btn btn-sm btn-primary join-item gap-1"
                >
                  <Plus className="w-3.5 h-3.5" /> Add
                </button>
              </form>
            </div>

            {/* Detected Text (Native OS OCR) Section */}
            <div>
              <div className="flex items-center justify-between mb-1.5">
                <label className="text-xs font-semibold text-base-content/80 flex items-center gap-1.5">
                  <FileText className="w-3.5 h-3.5 text-info" />
                  Detected Text (Native OS OCR)
                </label>
                {capture.ocrText && (
                  <button
                    type="button"
                    onClick={handleCopyOcrText}
                    className={`btn btn-xs gap-1 h-6 min-h-0 ${
                      copiedOcr ? "btn-success" : "btn-ghost text-info"
                    }`}
                  >
                    {copiedOcr ? (
                      <>
                        <Check className="w-3 h-3" /> Copied Text
                      </>
                    ) : (
                      <>
                        <Copy className="w-3 h-3" /> Copy Text
                      </>
                    )}
                  </button>
                )}
              </div>

              {capture.ocrText ? (
                <div className="p-2.5 rounded-lg bg-base-100 border border-base-300 font-mono text-[11px] text-base-content/85 max-h-28 overflow-y-auto whitespace-pre-wrap select-text">
                  {capture.ocrText}
                </div>
              ) : (
                <div className="p-2 rounded-lg bg-base-100/60 border border-base-300 text-[11px] text-base-content/50 italic">
                  {capture.ocrStatus === "pending"
                    ? "Background OCR scan pending..."
                    : "No text detected in this capture."}
                </div>
              )}
            </div>
          </div>

          {/* Bottom Actions */}
          <div className="flex flex-col gap-2 pt-4 border-t border-base-300 mt-4">
            {capture.mediaType === "video" ? (
              <button
                type="button"
                onClick={() => setShowTrimmer((v) => !v)}
                className={`btn btn-sm w-full gap-2 ${
                  showTrimmer ? "btn-primary" : "btn-outline btn-primary"
                }`}
              >
                <Scissors className="w-4 h-4" />
                {showTrimmer ? "Hide Video Trimmer" : "Trim & Audio Export"}
              </button>
            ) : (
              <button
                type="button"
                onClick={handleCopy}
                className={`btn btn-sm w-full gap-2 ${
                  copied ? "btn-success" : "btn-primary"
                }`}
              >
                <Copy className="w-4 h-4" />
                {copied ? "Copied to Clipboard!" : "Copy Image to Clipboard"}
              </button>
            )}

            <button
              type="button"
              onClick={handleOpenExplorer}
              className="btn btn-sm btn-neutral w-full gap-2"
            >
              <FolderOpen className="w-4 h-4 text-secondary" />
              Open in File Explorer / Finder
            </button>

            <div className="grid grid-cols-2 gap-2">
              <button
                type="button"
                onClick={() => onTogglePin(capture.id, capture.isPinned)}
                className={`btn btn-sm gap-1.5 ${
                  capture.isPinned ? "btn-warning" : "btn-outline"
                }`}
              >
                <Pin className="w-3.5 h-3.5" />
                {capture.isPinned ? "Pinned" : "Pin Capture"}
              </button>
              <button
                type="button"
                onClick={() => {
                  onDelete(capture.id);
                  onClose();
                }}
                className="btn btn-sm btn-soft btn-error gap-1.5"
              >
                <Trash2 className="w-3.5 h-3.5" />
                Delete
              </button>
            </div>
          </div>
        </div>
      </div>
    </dialog>
  );
};
