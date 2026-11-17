import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
  ArrowLeftToLine,
  ArrowRightFromLine,
  Check,
  Headphones,
  Layers,
  Loader2,
  Pause,
  Play,
  RefreshCw,
  RotateCcw,
  Scissors,
  Volume2,
  VolumeX,
} from "lucide-react";
import type { CaptureRecord, VideoAudioTrackPreview, VideoProbeInfo } from "../../types/ipc";
import {
  prepareVideoTrackPreviews,
  probeVideoAudioTracks,
  resolveMediaSrc,
  trimVideoClip,
} from "../../lib/tauri-client";
import { formatBytes } from "../../lib/dpi";

interface VideoTrimmerPanelProps {
  capture: CaptureRecord;
  videoRef: React.RefObject<HTMLVideoElement | null>;
  currentTimeMs: number;
  durationMs: number;
  isPlaying: boolean;
  onTrimmingChange?: (trimming: boolean) => void;
  onTrimmed: (trimmed: CaptureRecord, replacedId?: string) => void;
  onOpenClip?: (clip: CaptureRecord) => void;
}

const MIN_SPAN_MS = 500;
const WAVEFORM_BUCKETS = 160;

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
  onTrimmingChange,
  onTrimmed,
  onOpenClip,
}) => {
  const trackBarRef = useRef<HTMLDivElement>(null);
  const audioRefs = useRef<Record<number, HTMLAudioElement | null>>({});
  // Preserves the user's pre-monitor video.muted flag without ever mutating video.volume
  const preMonitorVideoMutedRef = useRef<boolean | null>(null);

  const [probeInfo, setProbeInfo] = useState<VideoProbeInfo | null>(null);
  const [previews, setPreviews] = useState<VideoAudioTrackPreview[]>([]);
  const [probing, setProbing] = useState(false);
  const [loadingWaveforms, setLoadingWaveforms] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);
  const [failedTracks, setFailedTracks] = useState<Record<number, string>>({});
  const [previewNonce, setPreviewNonce] = useState(0);

  const effectiveDurationMs = useMemo(() => {
    const candidate = durationMs || probeInfo?.durationMs || capture.durationMs || 0;
    return Math.max(MIN_SPAN_MS, candidate);
  }, [durationMs, probeInfo?.durationMs, capture.durationMs]);

  const [startMs, setStartMs] = useState(0);
  const [endMs, setEndMs] = useState(effectiveDurationMs);
  const [exportTracks, setExportTracks] = useState<number[]>([0]);

  // Auditioning state:
  // - `soloTrackIdx === null && customAuditionTracks === null` => Native <video> audio plays untouched.
  // - Clicking `S` solos a track via synchronized WAV <audio> monitor.
  // - Clicking Mute/Speaker toggles custom multi-track monitoring cleanly.
  const [soloTrackIdx, setSoloTrackIdx] = useState<number | null>(null);
  const [customAuditionTracks, setCustomAuditionTracks] = useState<number[] | null>(null);

  const [mixSelectedAudio, setMixSelectedAudio] = useState(false);
  const [targetMaxMb, setTargetMaxMb] = useState<number | null>(null);
  const [replaceOriginal, setReplaceOriginal] = useState(false);
  const [trimming, setTrimming] = useState(false);
  const [trimElapsedMs, setTrimElapsedMs] = useState(0);
  const [lastSavedClip, setLastSavedClip] = useState<CaptureRecord | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [draggingHandle, setDraggingHandle] = useState<"start" | "end" | null>(null);
  const [loopSelection, setLoopSelection] = useState(false);

  // Elapsed export timer so long size-capped re-encodes give continuous feedback
  useEffect(() => {
    if (!trimming) {
      setTrimElapsedMs(0);
      return;
    }
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setTrimElapsedMs(Date.now() - startedAt);
    }, 100);
    return () => window.clearInterval(timer);
  }, [trimming]);

  // Probe embedded tracks once per capture
  useEffect(() => {
    let cancelled = false;
    setErrorMsg(null);
    setProbeError(null);
    setLastSavedClip(null);
    setProbing(true);
    setSoloTrackIdx(null);
    setCustomAuditionTracks(null);
    setPreviewNonce(0);

    probeVideoAudioTracks(capture.id)
      .then((info) => {
        if (cancelled) return;
        setProbeInfo(info);
        if (info.audioTracks.length > 0) {
          setExportTracks([info.audioTracks[0].streamIndex]);
        } else {
          setExportTracks([]);
        }
        setMixSelectedAudio(false);
      })
      .catch((err) => {
        if (cancelled) return;
        setProbeInfo(null);
        setProbeError(err instanceof Error ? err.message : String(err));
        setExportTracks([0]);
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });

    return () => {
      cancelled = true;
    };
  }, [capture.id]);

  // Extract WAV track previews + waveform peaks (supports Retry via previewNonce)
  useEffect(() => {
    let cancelled = false;
    setFailedTracks({});
    setLoadingWaveforms(true);
    setPreviews([]);

    prepareVideoTrackPreviews(capture.id, WAVEFORM_BUCKETS, previewNonce > 0)
      .then((items) => {
        if (!cancelled) setPreviews(items);
      })
      .catch((err) => {
        if (!cancelled) {
          setPreviews([]);
          setProbeError((prev) => prev ?? (err instanceof Error ? err.message : String(err)));
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingWaveforms(false);
      });

    return () => {
      cancelled = true;
    };
  }, [capture.id, previewNonce]);

  // Reset trim range whenever capture changes or metadata duration loads
  useEffect(() => {
    setStartMs(0);
    setEndMs(effectiveDurationMs);
    setLoopSelection(false);
  }, [capture.id, effectiveDurationMs]);

  const playablePreviews = useMemo(
    () =>
      previews.filter(
        (p) => Boolean(p.audioPath) && failedTracks[p.streamIndex] === undefined
      ),
    [previews, failedTracks]
  );

  const hasLiveAudioStreams = playablePreviews.length > 0;
  const monitorRequested = soloTrackIdx !== null || customAuditionTracks !== null;

  const isTrackAudibleInPreview = useCallback(
    (streamIndex: number): boolean => {
      if (soloTrackIdx !== null) {
        return streamIndex === soloTrackIdx;
      }
      if (customAuditionTracks !== null) {
        return customAuditionTracks.includes(streamIndex);
      }
      return streamIndex === (probeInfo?.audioTracks[0]?.streamIndex ?? 0);
    },
    [soloTrackIdx, customAuditionTracks, probeInfo]
  );

  const anyAudibleTargetPlayable = useMemo(
    () => playablePreviews.some((p) => isTrackAudibleInPreview(p.streamIndex)),
    [playablePreviews, isTrackAudibleInPreview]
  );

  // Only mute <video> while monitoring is requested AND at least one monitor target is decodable
  // (or user explicitly muted all monitor tracks cleanly via customAuditionTracks = []).
  const monitorActive =
    monitorRequested &&
    hasLiveAudioStreams &&
    (anyAudibleTargetPlayable || (customAuditionTracks !== null && customAuditionTracks.length === 0));

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;

    if (monitorActive) {
      if (preMonitorVideoMutedRef.current === null) {
        preMonitorVideoMutedRef.current = video.muted;
      }
      video.muted = true;
    } else if (preMonitorVideoMutedRef.current !== null) {
      video.muted = preMonitorVideoMutedRef.current;
      preMonitorVideoMutedRef.current = null;
    }
  }, [videoRef, monitorActive]);

  // Always restore pre-monitor video.muted state when Trimmer closes/unmounts
  useEffect(() => {
    const video = videoRef.current;
    return () => {
      if (video && preMonitorVideoMutedRef.current !== null) {
        video.muted = preMonitorVideoMutedRef.current;
        preMonitorVideoMutedRef.current = null;
      }
    };
  }, [videoRef]);

  const syncSingleAudio = useCallback(
    (
      el: HTMLAudioElement,
      streamIndex: number,
      activeOverride?: boolean,
      soloOverride?: number | null,
      customOverride?: number[] | null
    ) => {
      const video = videoRef.current;
      if (!video) return;

      const isActive = activeOverride ?? monitorActive;
      const audible =
        soloOverride !== undefined
          ? soloOverride !== null
            ? streamIndex === soloOverride
            : customOverride !== undefined && customOverride !== null
            ? customOverride.includes(streamIndex)
            : false
          : isActive && isTrackAudibleInPreview(streamIndex);

      el.muted = !audible;
      el.volume = video.volume;
      el.playbackRate = video.playbackRate || 1;
      if (el.readyState >= 1 && Math.abs(el.currentTime - video.currentTime) > 0.08) {
        try {
          el.currentTime = video.currentTime;
        } catch {
          // Ignore seek before metadata ready
        }
      }

      if (isActive && !video.paused && audible && el.readyState >= 2) {
        el.play().catch((err) => {
          setFailedTracks((prev) => ({
            ...prev,
            [streamIndex]: err instanceof Error ? err.message : String(err),
          }));
        });
      } else {
        el.pause();
      }
    },
    [videoRef, monitorActive, isTrackAudibleInPreview]
  );

  // Keep <audio> monitor elements synchronized with <video> transport & volume (one-way)
  useEffect(() => {
    for (const preview of previews) {
      const el = audioRefs.current[preview.streamIndex];
      if (!el) continue;
      syncSingleAudio(el, preview.streamIndex);
    }
  }, [previews, syncSingleAudio]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !monitorActive) return;

    const syncAll = () => {
      for (const preview of previews) {
        const el = audioRefs.current[preview.streamIndex];
        if (!el) continue;
        syncSingleAudio(el, preview.streamIndex);
      }
    };

    const handleTimeUpdate = () => {
      if (video.paused) return;
      const targetTime = video.currentTime;
      for (const preview of previews) {
        const el = audioRefs.current[preview.streamIndex];
        if (!el || !isTrackAudibleInPreview(preview.streamIndex)) continue;
        if (el.readyState >= 2 && Math.abs(el.currentTime - targetTime) > 0.14) {
          try {
            el.currentTime = targetTime;
          } catch {
            // Ignore seek before metadata loaded
          }
        }
      }
    };

    syncAll();

    video.addEventListener("play", syncAll);
    video.addEventListener("pause", syncAll);
    video.addEventListener("seeking", syncAll);
    video.addEventListener("seeked", syncAll);
    video.addEventListener("ratechange", syncAll);
    video.addEventListener("volumechange", syncAll);
    video.addEventListener("timeupdate", handleTimeUpdate);

    return () => {
      video.removeEventListener("play", syncAll);
      video.removeEventListener("pause", syncAll);
      video.removeEventListener("seeking", syncAll);
      video.removeEventListener("seeked", syncAll);
      video.removeEventListener("ratechange", syncAll);
      video.removeEventListener("volumechange", syncAll);
      video.removeEventListener("timeupdate", handleTimeUpdate);
      for (const el of Object.values(audioRefs.current)) {
        el?.pause();
      }
    };
  }, [videoRef, monitorActive, previews, syncSingleAudio, isTrackAudibleInPreview]);

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
      const clampedSec = Math.max(0, ms) / 1000;
      const video = videoRef.current;
      if (video) {
        video.currentTime = clampedSec;
      }
      for (const el of Object.values(audioRefs.current)) {
        if (!el || el.readyState < 1) continue;
        try {
          el.currentTime = clampedSec;
        } catch {
          // Ignore seek before metadata ready
        }
      }
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

  const tracks = probeInfo?.audioTracks ?? [];

  const toggleExportTrack = (streamIndex: number) => {
    setExportTracks((prev) =>
      prev.includes(streamIndex)
        ? prev.filter((idx) => idx !== streamIndex)
        : [...prev, streamIndex].sort((a, b) => a - b)
    );
  };

  // Trigger audio element playback synchronously on user click so WebView2 autoplay policies allow it cleanly
  const triggerMonitorPlayOnGesture = (
    nextSolo: number | null,
    nextCustom: number[] | null
  ) => {
    const active = nextSolo !== null || nextCustom !== null;
    for (const preview of previews) {
      const el = audioRefs.current[preview.streamIndex];
      if (!el) continue;
      syncSingleAudio(el, preview.streamIndex, active, nextSolo, nextCustom);
    }
  };

  const toggleSoloTrack = (streamIndex: number) => {
    const nextSolo = soloTrackIdx === streamIndex ? null : streamIndex;
    setSoloTrackIdx(nextSolo);
    if (nextSolo !== null) {
      setCustomAuditionTracks(null);
    }
    triggerMonitorPlayOnGesture(nextSolo, nextSolo !== null ? null : customAuditionTracks);
  };

  const toggleAuditionTrack = (streamIndex: number) => {
    if (soloTrackIdx !== null) {
      const nextCustom = streamIndex === soloTrackIdx ? null : [streamIndex];
      setSoloTrackIdx(null);
      setCustomAuditionTracks(nextCustom);
      triggerMonitorPlayOnGesture(null, nextCustom);
      return;
    }

    if (customAuditionTracks === null) {
      const firstTrackIdx = tracks[0]?.streamIndex ?? 0;
      const nextCustom =
        streamIndex === firstTrackIdx
          ? tracks.filter((t) => t.streamIndex !== firstTrackIdx).map((t) => t.streamIndex)
          : [streamIndex];
      setCustomAuditionTracks(nextCustom);
      triggerMonitorPlayOnGesture(null, nextCustom);
      return;
    }

    const exists = customAuditionTracks.includes(streamIndex);
    const nextCustom = exists
      ? customAuditionTracks.filter((idx) => idx !== streamIndex)
      : [...customAuditionTracks, streamIndex].sort((a, b) => a - b);
    setCustomAuditionTracks(nextCustom);
    triggerMonitorPlayOnGesture(null, nextCustom);
  };

  const resetMonitorToDefault = () => {
    setSoloTrackIdx(null);
    setCustomAuditionTracks(null);
    triggerMonitorPlayOnGesture(null, null);
  };

  const handleRetryPreviews = () => {
    resetMonitorToDefault();
    setProbeError(null);
    setFailedTracks({});
    setPreviewNonce((n) => n + 1);
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
      seekVideoToMs(startMs);
    }
    setLoopSelection(true);
    video.play().catch(() => undefined);
  };

  const handleTrimSubmit = async () => {
    setTrimming(true);
    onTrimmingChange?.(true);
    setErrorMsg(null);
    setLastSavedClip(null);
    try {
      const trimmed = await trimVideoClip({
        id: capture.id,
        startMs,
        endMs,
        selectedAudioIndices: exportTracks,
        mixSelectedAudio: exportTracks.length > 1 ? mixSelectedAudio : false,
        targetMaxMb,
        replaceOriginal,
      });
      if (!replaceOriginal) {
        setLastSavedClip(trimmed);
      }
      onTrimmed(trimmed, replaceOriginal ? capture.id : undefined);
    } catch (err) {
      setErrorMsg(err instanceof Error ? err.message : String(err));
    } finally {
      setTrimming(false);
      onTrimmingChange?.(false);
    }
  };

  const previewMap = useMemo(() => {
    const map = new Map<number, VideoAudioTrackPreview>();
    for (const p of previews) {
      map.set(p.streamIndex, p);
    }
    return map;
  }, [previews]);

  const masterAndExtraSelected =
    exportTracks.includes(0) &&
    exportTracks.length > 1 &&
    tracks.some((t) => t.streamIndex === 0 && t.title.toLowerCase().includes("master"));

  const failedEntries = Object.entries(failedTracks);

  return (
    <div className="w-full max-w-5xl mt-3 rounded-xl bg-base-200/95 border border-base-300 p-4 shadow-lg space-y-3">
      {/* Hidden synchronized <audio> elements for per-track Solo/Mute auditioning */}
      {previews
        .filter((p) => Boolean(p.audioPath))
        .map((p) => (
          <audio
            key={`${capture.id}-audio-${previewNonce}-${p.streamIndex}`}
            ref={(el) => {
              audioRefs.current[p.streamIndex] = el;
            }}
            src={resolveMediaSrc(p.audioPath)}
            preload="auto"
            onCanPlay={(e) => syncSingleAudio(e.currentTarget, p.streamIndex)}
            onError={() => {
              setFailedTracks((prev) => ({
                ...prev,
                [p.streamIndex]: p.title,
              }));
            }}
          />
        ))}

      {/* Header Row */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Scissors className="w-4 h-4 text-primary" />
          <span className="text-sm font-semibold tracking-tight">
            Trim &amp; Multi-Track Audio Mixer
          </span>
          <span className="badge badge-sm badge-ghost font-mono text-xs">
            {formatPreciseMs(selectedSpanMs)} selected
          </span>
          <span className="text-xs opacity-70 font-mono">{estimatedSizeLabel}</span>
          {monitorActive && (
            <button
              type="button"
              onClick={resetMonitorToDefault}
              title="Return player audio back to default Master Mix"
              className="badge badge-sm badge-warning gap-1 cursor-pointer hover:opacity-90 text-xs"
            >
              <Headphones className="w-3 h-3" />
              {soloTrackIdx !== null
                ? `Solo: ${tracks.find((t) => t.streamIndex === soloTrackIdx)?.title ?? `Track ${soloTrackIdx + 1}`}`
                : `Monitoring ${customAuditionTracks?.length ?? 0} track(s)`}
              {" • Reset"}
            </button>
          )}
        </div>

        <div className="flex items-center gap-1.5">
          {(failedEntries.length > 0 || probeError) && (
            <button
              type="button"
              disabled={loadingWaveforms}
              onClick={handleRetryPreviews}
              className="btn btn-xs btn-outline btn-warning gap-1 text-xs"
              title="Regenerate track audio previews & waveforms"
            >
              <RefreshCw className={`w-3.5 h-3.5 ${loadingWaveforms ? "animate-spin" : ""}`} />
              Retry Previews
            </button>
          )}
          <button
            type="button"
            className="btn btn-xs btn-ghost gap-1 text-xs"
            title="Set Start (In) to current playhead"
            onClick={() => {
              const next = Math.min(Math.round(currentTimeMs), endMs - MIN_SPAN_MS);
              setStartMs(Math.max(0, next));
            }}
          >
            <ArrowRightFromLine className="w-3.5 h-3.5" />
            Set In
          </button>
          <button
            type="button"
            className="btn btn-xs btn-ghost gap-1 text-xs"
            title="Set End (Out) to current playhead"
            onClick={() => {
              const next = Math.max(Math.round(currentTimeMs), startMs + MIN_SPAN_MS);
              setEndMs(Math.min(effectiveDurationMs, next));
            }}
          >
            <ArrowLeftToLine className="w-3.5 h-3.5" />
            Set Out
          </button>
          <button
            type="button"
            className={`btn btn-xs gap-1 text-xs ${
              loopSelection && isPlaying ? "btn-primary" : "btn-outline"
            }`}
            onClick={handleTogglePlaySelection}
          >
            {loopSelection && isPlaying ? (
              <>
                <Pause className="w-3.5 h-3.5" /> Pause Loop
              </>
            ) : (
              <>
                <Play className="w-3.5 h-3.5" /> Play Selection
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
            <RotateCcw className="w-3.5 h-3.5" />
          </button>
        </div>
      </div>

      {/* Unified Multi-Lane Timeline (Aligned Playhead Across Video + Audio Waveforms) */}
      <div className="rounded-lg bg-base-300/60 border border-base-300 p-2.5 space-y-2">
        {/* Video Scrubber Lane */}
        <div className="flex items-center gap-3">
          <div className="w-56 sm:w-64 shrink-0 flex items-center justify-between pr-1">
            <span className="text-xs font-semibold uppercase tracking-wider opacity-80">
              Video Clip
            </span>
            <span className="text-xs font-mono opacity-75">
              {formatPreciseMs(startMs)} – {formatPreciseMs(endMs)}
            </span>
          </div>

          <div
            ref={trackBarRef}
            onPointerDown={(e) => {
              if ((e.target as HTMLElement).dataset.handle) return;
              seekVideoToMs(clientXToMs(e.clientX));
            }}
            className="relative flex-1 h-8 rounded-md bg-base-300 border border-base-content/15 cursor-pointer select-none touch-none overflow-hidden"
          >
            <div
              className="absolute inset-y-0 left-0 bg-base-100/60 pointer-events-none"
              style={{ width: `${startPct}%` }}
            />
            <div
              className="absolute inset-y-0 right-0 bg-base-100/60 pointer-events-none"
              style={{ width: `${Math.max(0, 100 - endPct)}%` }}
            />
            <div
              className="absolute inset-y-0 bg-primary/20 border-y border-primary/50 pointer-events-none"
              style={{
                left: `${startPct}%`,
                width: `${Math.max(0, endPct - startPct)}%`,
              }}
            />
            <div
              className="absolute inset-y-0 w-0.5 bg-warning pointer-events-none z-10"
              style={{ left: `${playheadPct}%` }}
            />
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
              <span className="w-0.5 h-4 bg-primary-content/85 rounded-full pointer-events-none" />
            </button>
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
              <span className="w-0.5 h-4 bg-primary-content/85 rounded-full pointer-events-none" />
            </button>
          </div>
        </div>

        {/* Per-Track Audio Waveform Lanes */}
        {probing ? (
          <div className="flex items-center gap-2 text-xs opacity-70 py-1.5 px-1">
            <Loader2 className="w-4 h-4 animate-spin" />
            Inspecting embedded audio streams...
          </div>
        ) : probeError ? (
          <div className="flex items-center justify-between gap-2 text-xs text-error py-1.5 px-1">
            <span>Audio track probe failed: {probeError}</span>
            <button
              type="button"
              onClick={handleRetryPreviews}
              className="btn btn-xs btn-outline btn-error gap-1"
            >
              <RefreshCw className="w-3 h-3" />
              Retry
            </button>
          </div>
        ) : tracks.length === 0 ? (
          <div className="text-xs opacity-65 py-1 px-1">
            No embedded audio streams detected (video-only export).
          </div>
        ) : (
          <div className="space-y-1.5">
            {tracks.map((track) => {
              const preview = previewMap.get(track.streamIndex);
              const peaks = preview?.peaks ?? [];
              const trackFailed = failedTracks[track.streamIndex] !== undefined;
              const trackPlayable = Boolean(preview?.audioPath) && !trackFailed;
              const included = exportTracks.includes(track.streamIndex);
              const isSolo = soloTrackIdx === track.streamIndex;
              const isAudible = isTrackAudibleInPreview(track.streamIndex);

              return (
                <div key={track.streamIndex} className="flex items-center gap-3">
                  {/* Track Label + Solo / Monitor Mute / Export Controls */}
                  <div className="w-56 sm:w-64 shrink-0 flex items-center justify-between gap-2 bg-base-200/85 border border-base-300 rounded-md px-2.5 py-1.5">
                    <div className="min-w-0 flex-1">
                      <div className="text-xs font-semibold truncate" title={track.title}>
                        {track.title}
                      </div>
                      <div className="text-[11px] font-mono opacity-65">
                        Track {track.streamIndex + 1} • {track.codec}
                        {trackFailed ? " • retry needed" : ""}
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0">
                      <button
                        type="button"
                        disabled={!trackPlayable}
                        title={
                          !trackPlayable
                            ? "Preview stream unavailable (click Retry Previews)"
                            : isSolo
                            ? "Disable Solo"
                            : "Solo this track in player"
                        }
                        onClick={() => toggleSoloTrack(track.streamIndex)}
                        className={`btn btn-xs h-6 min-h-0 px-2 font-mono text-xs ${
                          isSolo
                            ? "btn-warning text-warning-content"
                            : "btn-ghost opacity-75 hover:opacity-100"
                        }`}
                      >
                        S
                      </button>
                      <button
                        type="button"
                        disabled={!trackPlayable}
                        title={
                          !trackPlayable
                            ? "Preview stream unavailable (click Retry Previews)"
                            : isAudible
                            ? "Mute track in Lightbox preview"
                            : "Listen to track in Lightbox preview"
                        }
                        onClick={() => toggleAuditionTrack(track.streamIndex)}
                        className={`btn btn-xs h-6 min-h-0 px-2 ${
                          isAudible
                            ? "btn-ghost text-info"
                            : "btn-ghost opacity-45 hover:opacity-90"
                        }`}
                      >
                        {isAudible ? (
                          <Volume2 className="w-3.5 h-3.5" />
                        ) : (
                          <VolumeX className="w-3.5 h-3.5" />
                        )}
                      </button>
                      <button
                        type="button"
                        title={
                          included
                            ? "Included in exported clip (click to exclude)"
                            : "Excluded from export (click to include)"
                        }
                        onClick={() => toggleExportTrack(track.streamIndex)}
                        className={`btn btn-xs h-6 min-h-0 px-2 gap-1 text-xs ${
                          included ? "btn-primary" : "btn-outline opacity-60"
                        }`}
                      >
                        {included && <Check className="w-3 h-3" />}
                        Exp
                      </button>
                    </div>
                  </div>

                  {/* Click-to-Seek Waveform Bar with Identical Playhead Alignment */}
                  <div
                    onPointerDown={(e) => {
                      const rect = e.currentTarget.getBoundingClientRect();
                      if (rect.width <= 0) return;
                      const ratio = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
                      seekVideoToMs(Math.round(ratio * effectiveDurationMs));
                    }}
                    className={`relative flex-1 h-10 rounded-md bg-base-200/80 border cursor-pointer overflow-hidden flex items-center px-0.5 transition-opacity ${
                      included ? "border-base-content/15" : "border-base-300 opacity-50"
                    }`}
                  >
                    {loadingWaveforms && peaks.length === 0 ? (
                      <div className="w-full text-center text-xs opacity-60 font-mono">
                        Generating waveform...
                      </div>
                    ) : peaks.length === 0 ? (
                      <div className="w-full h-px bg-base-content/15" />
                    ) : (
                      <div className="w-full h-8 flex items-center gap-[1px] pointer-events-none">
                        {peaks.map((amp, i) => {
                          const barPct = ((i + 0.5) / peaks.length) * 100;
                          const inSelection = barPct >= startPct && barPct <= endPct;
                          const heightPct = Math.max(10, Math.round(amp * 100));
                          return (
                            <span
                              key={i}
                              className={`flex-1 rounded-full ${
                                !inSelection
                                  ? "bg-base-content/20"
                                  : isAudible
                                  ? "bg-info"
                                  : included
                                  ? "bg-primary/75"
                                  : "bg-base-content/35"
                              }`}
                              style={{ height: `${heightPct}%` }}
                            />
                          );
                        })}
                      </div>
                    )}

                    {/* Dimmed outside selection + Unified Playhead */}
                    <div
                      className="absolute inset-y-0 left-0 bg-base-100/45 pointer-events-none"
                      style={{ width: `${startPct}%` }}
                    />
                    <div
                      className="absolute inset-y-0 right-0 bg-base-100/45 pointer-events-none"
                      style={{ width: `${Math.max(0, 100 - endPct)}%` }}
                    />
                    <div
                      className="absolute inset-y-0 w-0.5 bg-warning pointer-events-none z-10"
                      style={{ left: `${playheadPct}%` }}
                    />
                  </div>
                </div>
              );
            })}
          </div>
        )}

        <div className="flex items-center justify-between text-xs font-mono opacity-75 pt-0.5">
          <span>In: {formatPreciseMs(startMs)}</span>
          <span>Playhead: {formatPreciseMs(currentTimeMs)}</span>
          <span>Out: {formatPreciseMs(endMs)} / {formatPreciseMs(effectiveDurationMs)}</span>
        </div>
      </div>

      {/* Mix Options + Target Size Presets + Export Submit */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-1.5 border-t border-base-300">
        <div className="space-y-1.5">
          <div className="flex flex-wrap items-center gap-1.5">
            {SIZE_PRESETS.map((preset) => {
              const active = targetMaxMb === preset.value;
              return (
                <button
                  key={preset.label}
                  type="button"
                  className={`btn btn-xs text-xs ${active ? "btn-primary" : "btn-outline"}`}
                  onClick={() => setTargetMaxMb(preset.value)}
                >
                  {preset.label}
                </button>
              );
            })}
          </div>

          <div className="flex flex-wrap items-center gap-3 text-xs">
            {exportTracks.length > 1 && (
              <label className="flex items-center gap-1.5 cursor-pointer">
                <input
                  type="checkbox"
                  className="checkbox checkbox-xs checkbox-primary"
                  checked={mixSelectedAudio}
                  onChange={(e) => setMixSelectedAudio(e.target.checked)}
                />
                <span>Mix {exportTracks.length} exported tracks into 1 Discord stream</span>
              </label>
            )}
            {exportTracks.length > 1 && !mixSelectedAudio && (
              <span className="opacity-70 flex items-center gap-1">
                <Layers className="w-3.5 h-3.5" />
                Multi-track `.mp4` ({exportTracks.length} streams)
              </span>
            )}
            {masterAndExtraSelected && (
              <span className="text-warning opacity-90">
                Tip: Master Mix already blends Desktop + Mic
              </span>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-3 ml-auto">
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
            className="btn btn-sm btn-primary gap-1.5 text-xs"
            disabled={trimming || selectedSpanMs < MIN_SPAN_MS}
            onClick={handleTrimSubmit}
          >
            {trimming ? (
              <>
                <span className="loading loading-spinner loading-xs" />
                Trimming ({(trimElapsedMs / 1000).toFixed(1)}s)...
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

      {trimming && (
        <div className="rounded-lg bg-info/10 border border-info/30 p-2.5 space-y-1.5 text-xs">
          <div className="flex items-center justify-between gap-2">
            <div className="flex items-center gap-2 font-medium text-info">
              <span className="loading loading-spinner loading-xs" />
              <span>
                {targetMaxMb
                  ? `Trimming & compressing highlight to <= ${targetMaxMb} MB (${exportTracks.length} audio stream${exportTracks.length === 1 ? "" : "s"})...`
                  : `Running lossless keyframe trim (${exportTracks.length} audio stream${exportTracks.length === 1 ? "" : "s"})...`}
              </span>
            </div>
            <span className="font-mono opacity-80">
              {(trimElapsedMs / 1000).toFixed(1)}s
            </span>
          </div>
          <progress className="progress progress-info w-full h-1.5" />
        </div>
      )}

      {!trimming && lastSavedClip && (
        <div className="alert alert-success py-2 px-3 text-xs flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Check className="w-4 h-4 shrink-0" />
            <span>
              Saved trimmed highlight ({formatPreciseMs(lastSavedClip.durationMs ?? selectedSpanMs)} •{" "}
              {formatBytes(lastSavedClip.sizeBytes)}) to your Gallery.
            </span>
          </div>
          {onOpenClip && (
            <button
              type="button"
              onClick={() => onOpenClip(lastSavedClip)}
              className="btn btn-xs btn-ghost border border-current/30 shrink-0"
            >
              Open Trimmed Clip
            </button>
          )}
        </div>
      )}

      {(errorMsg || failedEntries.length > 0) && (
        <div className="alert alert-error py-2 px-3 text-xs flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <AlertCircle className="w-4 h-4 shrink-0" />
            <span>
              {errorMsg ||
                `Failed to decode preview stream for ${failedEntries
                  .map(([idx, title]) => title || `Track ${Number(idx) + 1}`)
                  .join(", ")}. Original video audio stays available.`}
            </span>
          </div>
          {failedEntries.length > 0 && (
            <button
              type="button"
              disabled={loadingWaveforms}
              onClick={handleRetryPreviews}
              className="btn btn-xs btn-ghost border border-current/30 gap-1 shrink-0"
            >
              <RefreshCw className={`w-3 h-3 ${loadingWaveforms ? "animate-spin" : ""}`} />
              Retry
            </button>
          )}
        </div>
      )}
    </div>
  );
};
