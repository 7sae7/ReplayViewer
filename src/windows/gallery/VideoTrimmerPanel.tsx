import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
  ArrowLeftToLine,
  ArrowRightFromLine,
  Layers,
  Loader2,
  Pause,
  Play,
  RotateCcw,
  Scissors,
  Volume2,
} from "lucide-react";
import type { CaptureRecord, VideoProbeInfo } from "../../types/ipc";
import { probeVideoAudioTracks, trimVideoClip } from "../../lib/tauri-client";
import { formatBytes } from "../../lib/dpi";

interface VideoTrimmerPanelProps {
  capture: CaptureRecord;
  videoRef: React.RefObject<HTMLVideoElement | null>;
  currentTimeMs: number;
  durationMs: number;
  isPlaying: boolean;
  onTrimmed: (trimmed: CaptureRecord, replacedId?: string) => void;
}

const MIN_SPAN_MS = 500;

const SIZE_PRESETS: ReadonlyArray<{ label: string; value: number | null }> = [
  { label: "Lossless (Instant)", value: null },
  { label: "Discord 10 MB", value: 10 },
  { label: "25 MB", value: 25 },
  { label: "50 MB", value: 50 },
];

function formatPreciseMs(ms: number): string {
  const safe = Math.max(0, ms);
  const totalSec = safe / 1000;
  const mins = Math.floor(totalSec / 60);
  const secs = Math.floor(totalSec % 60);
  const tenths = Math.floor((safe % 1000) / 100);
  return `${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}.${tenths}`;
}

export const VideoTrimmerPanel: React.FC<VideoTrimmerPanelProps> = ({
  capture,
  videoRef,
  currentTimeMs,
  durationMs,
  isPlaying,
  onTrimmed,
}) => {
  const trackBarRef = useRef<HTMLDivElement>(null);
  const [probeInfo, setProbeInfo] = useState<VideoProbeInfo | null>(null);
  const [probing, setProbing] = useState(false);

  const effectiveDurationMs = useMemo(() => {
    const candidate = durationMs || probeInfo?.durationMs || capture.durationMs || 0;
    return Math.max(MIN_SPAN_MS, candidate);
  }, [durationMs, probeInfo?.durationMs, capture.durationMs]);

  const [startMs, setStartMs] = useState(0);
  const [endMs, setEndMs] = useState(effectiveDurationMs);
  const [selectedTracks, setSelectedTracks] = useState<number[]>([0]);
  const [mixSelectedAudio, setMixSelectedAudio] = useState(false);
  const [targetMaxMb, setTargetMaxMb] = useState<number | null>(null);
  const [replaceOriginal, setReplaceOriginal] = useState(false);
  const [trimming, setTrimming] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [draggingHandle, setDraggingHandle] = useState<"start" | "end" | null>(null);
  const [loopSelection, setLoopSelection] = useState(false);

  // Probe embedded audio streams + accurate duration when opening a capture
  useEffect(() => {
    let cancelled = false;
    setErrorMsg(null);
    setProbing(true);
    probeVideoAudioTracks(capture.id)
      .then((info) => {
        if (cancelled) return;
        setProbeInfo(info);
        if (info.audioTracks.length > 0) {
          setSelectedTracks([info.audioTracks[0].streamIndex]);
        } else {
          setSelectedTracks([]);
        }
        setMixSelectedAudio(false);
      })
      .catch(() => {
        if (cancelled) return;
        setProbeInfo(null);
        setSelectedTracks([0]);
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });

    return () => {
      cancelled = true;
    };
  }, [capture.id]);

  // Reset trim range whenever capture changes or metadata duration loads
  useEffect(() => {
    setStartMs(0);
    setEndMs(effectiveDurationMs);
    setLoopSelection(false);
  }, [capture.id, effectiveDurationMs]);

  // Keep loop-selection playback constrained between In and Out points
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !loopSelection) return;
    if (currentTimeMs >= endMs - 40) {
      video.currentTime = startMs / 1000;
      if (video.paused) {
        video.play().catch(() => undefined);
      }
    }
  }, [currentTimeMs, endMs, startMs, loopSelection, videoRef]);

  const seekVideoToMs = useCallback(
    (ms: number) => {
      const video = videoRef.current;
      if (!video) return;
      video.currentTime = Math.max(0, ms) / 1000;
    },
    [videoRef]
  );

  const clientXToMs = useCallback(
    (clientX: number): number => {
      const bar = trackBarRef.current;
      if (!bar) return 0;
      const rect = bar.getBoundingClientRect();
      if (rect.width <= 0) return 0;
      const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
      return Math.round(ratio * effectiveDurationMs);
    },
    [effectiveDurationMs]
  );

  useEffect(() => {
    if (!draggingHandle) return;
    const handlePointerMove = (e: PointerEvent) => {
      const ms = clientXToMs(e.clientX);
      if (draggingHandle === "start") {
        const clamped = Math.min(ms, endMs - MIN_SPAN_MS);
        setStartMs(Math.max(0, clamped));
        seekVideoToMs(Math.max(0, clamped));
      } else {
        const clamped = Math.max(ms, startMs + MIN_SPAN_MS);
        setEndMs(Math.min(effectiveDurationMs, clamped));
        seekVideoToMs(Math.min(effectiveDurationMs, clamped));
      }
    };
    const handlePointerUp = () => {
      setDraggingHandle(null);
    };
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
    };
  }, [draggingHandle, clientXToMs, startMs, endMs, effectiveDurationMs, seekVideoToMs]);

  const startPct = (startMs / effectiveDurationMs) * 100;
  const endPct = (endMs / effectiveDurationMs) * 100;
  const playheadPct = Math.min(100, Math.max(0, (currentTimeMs / effectiveDurationMs) * 100));
  const selectedSpanMs = Math.max(MIN_SPAN_MS, endMs - startMs);

  const estimatedSizeLabel = useMemo(() => {
    const sourceBytes = probeInfo?.sizeBytes || capture.sizeBytes;
    const proportionalBytes = Math.round(
      sourceBytes * (selectedSpanMs / Math.max(MIN_SPAN_MS, effectiveDurationMs))
    );
    if (targetMaxMb !== null) {
      const capBytes = targetMaxMb * 1024 * 1024;
      const est = Math.min(proportionalBytes, Math.round(capBytes * 0.94));
      return `<= ${targetMaxMb} MB (est. ~${formatBytes(est)})`;
    }
    return `est. ~${formatBytes(proportionalBytes)}`;
  }, [probeInfo?.sizeBytes, capture.sizeBytes, selectedSpanMs, effectiveDurationMs, targetMaxMb]);

  const toggleTrack = (streamIndex: number) => {
    setSelectedTracks((prev) =>
      prev.includes(streamIndex)
        ? prev.filter((idx) => idx !== streamIndex)
        : [...prev, streamIndex].sort((a, b) => a - b)
    );
  };

  const handleTogglePlaySelection = () => {
    const video = videoRef.current;
    if (!video) return;
    if (isPlaying && loopSelection) {
      video.pause();
      setLoopSelection(false);
      return;
    }
    if (currentTimeMs < startMs || currentTimeMs >= endMs - 80) {
      video.currentTime = startMs / 1000;
    }
    setLoopSelection(true);
    video.play().catch(() => undefined);
  };

  const handleTrimSubmit = async () => {
    setTrimming(true);
    setErrorMsg(null);
    try {
      const trimmed = await trimVideoClip({
        id: capture.id,
        startMs,
        endMs,
        selectedAudioIndices: selectedTracks,
        mixSelectedAudio: selectedTracks.length > 1 ? mixSelectedAudio : false,
        targetMaxMb,
        replaceOriginal,
      });
      onTrimmed(trimmed, replaceOriginal ? capture.id : undefined);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setTrimming(false);
    }
  };

  const tracks = probeInfo?.audioTracks ?? [];
  const audioStreamCount = selectedTracks.length;

  return (
    <div className="w-full max-w-4xl mt-3 rounded-xl bg-base-200/95 border border-base-300 p-3.5 shadow-lg space-y-3">
      {/* Header Row */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <Scissors className="w-4 h-4 text-primary" />
          <span className="text-xs font-semibold tracking-tight">
            Trim &amp; Audio Track Export
          </span>
          <span className="badge badge-xs badge-ghost font-mono">
            {formatPreciseMs(selectedSpanMs)} selected
          </span>
          <span className="text-[11px] opacity-65 font-mono">{estimatedSizeLabel}</span>
        </div>

        <div className="flex items-center gap-1.5">
          <button
            type="button"
            className="btn btn-xs btn-ghost gap-1"
            title="Set Start (In) to current playhead"
            onClick={() => {
              const next = Math.min(Math.round(currentTimeMs), endMs - MIN_SPAN_MS);
              setStartMs(Math.max(0, next));
            }}
          >
            <ArrowRightFromLine className="w-3 h-3" />
            Set In
          </button>
          <button
            type="button"
            className="btn btn-xs btn-ghost gap-1"
            title="Set End (Out) to current playhead"
            onClick={() => {
              const next = Math.max(Math.round(currentTimeMs), startMs + MIN_SPAN_MS);
              setEndMs(Math.min(effectiveDurationMs, next));
            }}
          >
            <ArrowLeftToLine className="w-3 h-3" />
            Set Out
          </button>
          <button
            type="button"
            className={`btn btn-xs gap-1 ${loopSelection && isPlaying ? "btn-primary" : "btn-outline"}`}
            onClick={handleTogglePlaySelection}
          >
            {loopSelection && isPlaying ? (
              <>
                <Pause className="w-3 h-3" /> Pause Loop
              </>
            ) : (
              <>
                <Play className="w-3 h-3" /> Play Selection
              </>
            )}
          </button>
          <button
            type="button"
            className="btn btn-xs btn-ghost"
            title="Reset trim range to full clip"
            onClick={() => {
              setStartMs(0);
              setEndMs(effectiveDurationMs);
              setLoopSelection(false);
            }}
          >
            <RotateCcw className="w-3 h-3" />
          </button>
        </div>
      </div>

      {/* Dual-Handle Timeline Range Scrubber */}
      <div className="space-y-1">
        <div
          ref={trackBarRef}
          onPointerDown={(e) => {
            if ((e.target as HTMLElement).dataset.handle) return;
            const ms = clientXToMs(e.clientX);
            seekVideoToMs(ms);
          }}
          className="relative h-8 rounded-lg bg-base-300 border border-base-content/10 cursor-pointer select-none touch-none overflow-hidden"
        >
          {/* Dimmed excluded areas */}
          <div
            className="absolute inset-y-0 left-0 bg-base-100/55 pointer-events-none"
            style={{ width: `${startPct}%` }}
          />
          <div
            className="absolute inset-y-0 right-0 bg-base-100/55 pointer-events-none"
            style={{ width: `${Math.max(0, 100 - endPct)}%` }}
          />

          {/* Selected Active Span */}
          <div
            className="absolute inset-y-0 bg-primary/20 border-y border-primary/50 pointer-events-none"
            style={{
              left: `${startPct}%`,
              width: `${Math.max(0, endPct - startPct)}%`,
            }}
          />

          {/* Playhead */}
          <div
            className="absolute inset-y-0 w-0.5 bg-warning pointer-events-none z-10"
            style={{ left: `${playheadPct}%` }}
          />

          {/* Start Handle */}
          <button
            type="button"
            aria-label="Trim start handle"
            data-handle="start"
            onPointerDown={(e) => {
              e.stopPropagation();
              setDraggingHandle("start");
            }}
            className="absolute inset-y-0 -translate-x-1/2 w-3.5 bg-primary hover:bg-primary/90 text-primary-content flex items-center justify-center cursor-ew-resize z-20 rounded-sm shadow"
            style={{ left: `${startPct}%` }}
          >
            <span className="w-0.5 h-3.5 bg-primary-content/80 rounded-full pointer-events-none" />
          </button>

          {/* End Handle */}
          <button
            type="button"
            aria-label="Trim end handle"
            data-handle="end"
            onPointerDown={(e) => {
              e.stopPropagation();
              setDraggingHandle("end");
            }}
            className="absolute inset-y-0 -translate-x-1/2 w-3.5 bg-primary hover:bg-primary/90 text-primary-content flex items-center justify-center cursor-ew-resize z-20 rounded-sm shadow"
            style={{ left: `${endPct}%` }}
          >
            <span className="w-0.5 h-3.5 bg-primary-content/80 rounded-full pointer-events-none" />
          </button>
        </div>

        <div className="flex items-center justify-between text-[11px] font-mono opacity-75 px-0.5">
          <span>In: {formatPreciseMs(startMs)}</span>
          <span>Playhead: {formatPreciseMs(currentTimeMs)}</span>
          <span>Out: {formatPreciseMs(endMs)} / {formatPreciseMs(effectiveDurationMs)}</span>
        </div>
      </div>

      {/* Audio Track Selection + File Size Presets */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1 border-t border-base-300">
        {/* Per-Track Audio Picker */}
        <div className="space-y-1.5">
          <div className="flex items-center justify-between">
            <span className="text-[11px] uppercase tracking-wider font-semibold opacity-70 flex items-center gap-1.5">
              <Volume2 className="w-3.5 h-3.5" />
              Audio Tracks ({audioStreamCount}/{tracks.length || 1} kept)
            </span>
            {tracks.length > 1 && (
              <label className="flex items-center gap-1.5 cursor-pointer text-[11px]">
                <input
                  type="checkbox"
                  className="checkbox checkbox-xs checkbox-primary"
                  checked={mixSelectedAudio}
                  disabled={audioStreamCount <= 1}
                  onChange={(e) => setMixSelectedAudio(e.target.checked)}
                />
                <span className={audioStreamCount <= 1 ? "opacity-40" : "opacity-85"}>
                  Mix into 1 Discord track
                </span>
              </label>
            )}
          </div>

          {probing ? (
            <div className="flex items-center gap-2 text-xs opacity-65 py-1">
              <Loader2 className="w-3.5 h-3.5 animate-spin" />
              Scanning embedded audio streams...
            </div>
          ) : tracks.length === 0 ? (
            <div className="text-xs opacity-60 py-1">
              No embedded audio streams detected (video-only export).
            </div>
          ) : (
            <div className="flex flex-wrap gap-1.5">
              {tracks.map((track) => {
                const checked = selectedTracks.includes(track.streamIndex);
                return (
                  <label
                    key={track.streamIndex}
                    className={`flex items-center gap-1.5 px-2 py-1 rounded-lg border text-xs cursor-pointer transition-colors ${
                      checked
                        ? "bg-primary/15 border-primary/40 text-base-content"
                        : "bg-base-300/60 border-base-300 opacity-65 hover:opacity-100"
                    }`}
                  >
                    <input
                      type="checkbox"
                      className="checkbox checkbox-xs checkbox-primary"
                      checked={checked}
                      onChange={() => toggleTrack(track.streamIndex)}
                    />
                    <span className="font-medium truncate max-w-[140px]">
                      {track.title}
                    </span>
                    <span className="text-[10px] font-mono opacity-60">
                      #{track.streamIndex + 1}
                    </span>
                  </label>
                );
              })}
            </div>
          )}

          {audioStreamCount > 1 && !mixSelectedAudio && (
            <p className="text-[11px] opacity-60 flex items-center gap-1">
              <Layers className="w-3 h-3 shrink-0" />
              Saves {audioStreamCount} discrete audio streams (best for VLC / NLEs).
            </p>
          )}
        </div>

        {/* Target Size Presets + Export Action */}
        <div className="space-y-2 flex flex-col justify-between">
          <div className="space-y-1.5">
            <span className="text-[11px] uppercase tracking-wider font-semibold opacity-70 block">
              Export Quality / Target Size
            </span>
            <div className="flex flex-wrap gap-1.5">
              {SIZE_PRESETS.map((preset) => {
                const active = targetMaxMb === preset.value;
                return (
                  <button
                    key={preset.label}
                    type="button"
                    className={`btn btn-xs ${active ? "btn-primary" : "btn-outline"}`}
                    onClick={() => setTargetMaxMb(preset.value)}
                  >
                    {preset.label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
            <label className="flex items-center gap-2 cursor-pointer text-xs">
              <input
                type="checkbox"
                className="checkbox checkbox-xs checkbox-warning"
                checked={replaceOriginal}
                onChange={(e) => setReplaceOriginal(e.target.checked)}
              />
              <span className="opacity-85">Delete original after trim</span>
            </label>

            <button
              type="button"
              className="btn btn-sm btn-primary gap-1.5"
              disabled={trimming || selectedSpanMs < MIN_SPAN_MS}
              onClick={handleTrimSubmit}
            >
              {trimming ? (
                <>
                  <span className="loading loading-spinner loading-xs" />
                  Exporting...
                </>
              ) : (
                <>
                  <Scissors className="w-3.5 h-3.5" />
                  {replaceOriginal ? "Trim & Replace Original" : "Save Trimmed Clip"}
                </>
              )}
            </button>
          </div>
        </div>
      </div>

      {errorMsg && (
        <div className="alert alert-error py-2 px-3 text-xs">
          <AlertCircle className="w-4 h-4 shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}
    </div>
  );
};
