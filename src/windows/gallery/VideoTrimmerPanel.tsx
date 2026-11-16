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
  onTrimmed: (trimmed: CaptureRecord, replacedId?: string) => void;
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
  onTrimmed,
}) => {
  const trackBarRef = useRef<HTMLDivElement>(null);
  const audioRefs = useRef<Record<number, HTMLAudioElement | null>>({});

  const [probeInfo, setProbeInfo] = useState<VideoProbeInfo | null>(null);
  const [previews, setPreviews] = useState<VideoAudioTrackPreview[]>([]);
  const [probing, setProbing] = useState(false);
  const [loadingWaveforms, setLoadingWaveforms] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);

  const effectiveDurationMs = useMemo(() => {
    const candidate = durationMs || probeInfo?.durationMs || capture.durationMs || 0;
    return Math.max(MIN_SPAN_MS, candidate);
  }, [durationMs, probeInfo?.durationMs, capture.durationMs]);

  const [startMs, setStartMs] = useState(0);
  const [endMs, setEndMs] = useState(effectiveDurationMs);
  const [exportTracks, setExportTracks] = useState<number[]>([0]);
  const [soloTrackIdx, setSoloTrackIdx] = useState<number | null>(null);
  const [auditionMutedTracks, setAuditionMutedTracks] = useState<number[]>([]);
  const [mixSelectedAudio, setMixSelectedAudio] = useState(false);
  const [targetMaxMb, setTargetMaxMb] = useState<number | null>(null);
  const [replaceOriginal, setReplaceOriginal] = useState(false);
  const [trimming, setTrimming] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [draggingHandle, setDraggingHandle] = useState<"start" | "end" | null>(null);
  const [loopSelection, setLoopSelection] = useState(false);

  // Probe embedded tracks + lazily generate per-track waveforms & demuxed preview streams
  useEffect(() => {
    let cancelled = false;
    setErrorMsg(null);
    setProbeError(null);
    setProbing(true);
    setLoadingWaveforms(true);
    setPreviews([]);
    setSoloTrackIdx(null);

    probeVideoAudioTracks(capture.id)
      .then((info) => {
        if (cancelled) return;
        setProbeInfo(info);
        if (info.audioTracks.length > 0) {
          const firstIdx = info.audioTracks[0].streamIndex;
          setExportTracks([firstIdx]);
          // Default auditioning to Track 0 only so Master Mix doesn't double-play over Desktop/Mic
          setAuditionMutedTracks(
            info.audioTracks
              .filter((t) => t.streamIndex !== firstIdx)
              .map((t) => t.streamIndex)
          );
        } else {
          setExportTracks([]);
          setAuditionMutedTracks([]);
        }
        setMixSelectedAudio(false);
      })
      .catch((err) => {
        if (cancelled) return;
        setProbeInfo(null);
        setProbeError(err instanceof Error ? err.message : String(err));
        setExportTracks([0]);
        setAuditionMutedTracks([]);
      })
      .finally(() => {
        if (!cancelled) setProbing(false);
      });

    prepareVideoTrackPreviews(capture.id, WAVEFORM_BUCKETS)
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
  }, [capture.id]);

  // Reset trim range whenever capture changes or metadata duration loads
  useEffect(() => {
    setStartMs(0);
    setEndMs(effectiveDurationMs);
    setLoopSelection(false);
  }, [capture.id, effectiveDurationMs]);

  const hasLiveAudioStreams = useMemo(
    () => previews.some((p) => Boolean(p.audioPath)),
    [previews]
  );

  const isTrackAudibleInPreview = useCallback(
    (streamIndex: number): boolean => {
      if (soloTrackIdx !== null) {
        return streamIndex === soloTrackIdx;
      }
      return !auditionMutedTracks.includes(streamIndex);
    },
    [soloTrackIdx, auditionMutedTracks]
  );

  // Mute native <video> track while demuxed preview <audio> streams are active
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !hasLiveAudioStreams) return;
    const prevMuted = video.muted;
    video.muted = true;
    return () => {
      video.muted = prevMuted;
    };
  }, [videoRef, hasLiveAudioStreams]);

  // Update <audio> mute/volume states whenever Solo/Mute toggles mid-playback
  useEffect(() => {
    const video = videoRef.current;
    if (!hasLiveAudioStreams) return;
    const vol = video?.volume ?? 1;
    for (const preview of previews) {
      const el = audioRefs.current[preview.streamIndex];
      if (!el) continue;
      el.muted = !isTrackAudibleInPreview(preview.streamIndex);
      el.volume = vol;
    }
  }, [videoRef, hasLiveAudioStreams, previews, isTrackAudibleInPreview]);

  // Lockstep sync between <video> transport and all demuxed <audio> preview elements
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !hasLiveAudioStreams) return;

    const getElements = () =>
      Object.values(audioRefs.current).filter((el): el is HTMLAudioElement => Boolean(el));

    const syncTimeAndRate = () => {
      const targetTime = video.currentTime;
      for (const el of getElements()) {
        el.playbackRate = video.playbackRate || 1;
        if (Math.abs(el.currentTime - targetTime) > 0.08) {
          try {
            el.currentTime = targetTime;
          } catch {
            // Ignore seek before metadata loaded
          }
        }
      }
    };

    const handlePlay = () => {
      syncTimeAndRate();
      for (const el of getElements()) {
        el.play().catch(() => undefined);
      }
    };

    const handlePause = () => {
      for (const el of getElements()) {
        el.pause();
      }
      syncTimeAndRate();
    };

    const handleSeek = () => {
      syncTimeAndRate();
    };

    const handleTimeUpdate = () => {
      if (video.paused) return;
      const targetTime = video.currentTime;
      for (const el of getElements()) {
        if (Math.abs(el.currentTime - targetTime) > 0.14) {
          try {
            el.currentTime = targetTime;
          } catch {
            // Ignore seek before metadata loaded
          }
        }
      }
    };

    const handleVolumeChange = () => {
      for (const el of getElements()) {
        el.volume = video.volume;
      }
    };

    if (!video.paused) {
      handlePlay();
    } else {
      syncTimeAndRate();
    }

    video.addEventListener("play", handlePlay);
    video.addEventListener("pause", handlePause);
    video.addEventListener("seeking", handleSeek);
    video.addEventListener("seeked", handleSeek);
    video.addEventListener("ratechange", syncTimeAndRate);
    video.addEventListener("timeupdate", handleTimeUpdate);
    video.addEventListener("volumechange", handleVolumeChange);

    return () => {
      video.removeEventListener("play", handlePlay);
      video.removeEventListener("pause", handlePause);
      video.removeEventListener("seeking", handleSeek);
      video.removeEventListener("seeked", handleSeek);
      video.removeEventListener("ratechange", syncTimeAndRate);
      video.removeEventListener("timeupdate", handleTimeUpdate);
      video.removeEventListener("volumechange", handleVolumeChange);
      for (const el of getElements()) {
        el.pause();
      }
    };
  }, [videoRef, hasLiveAudioStreams, previews]);

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
        if (!el) continue;
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

  const toggleExportTrack = (streamIndex: number) => {
    setExportTracks((prev) =>
      prev.includes(streamIndex)
        ? prev.filter((idx) => idx !== streamIndex)
        : [...prev, streamIndex].sort((a, b) => a - b)
    );
  };

  const toggleSoloTrack = (streamIndex: number) => {
    setSoloTrackIdx((prev) => (prev === streamIndex ? null : streamIndex));
  };

  const toggleAuditionMute = (streamIndex: number) => {
    if (soloTrackIdx !== null) {
      setSoloTrackIdx(null);
      setAuditionMutedTracks(
        tracks.filter((t) => t.streamIndex !== streamIndex).map((t) => t.streamIndex)
      );
      return;
    }
    setAuditionMutedTracks((prev) =>
      prev.includes(streamIndex)
        ? prev.filter((idx) => idx !== streamIndex)
        : [...prev, streamIndex]
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
      seekVideoToMs(startMs);
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
        selectedAudioIndices: exportTracks,
        mixSelectedAudio: exportTracks.length > 1 ? mixSelectedAudio : false,
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

  return (
    <div className="w-full max-w-5xl mt-3 rounded-xl bg-base-200/95 border border-base-300 p-3.5 shadow-lg space-y-3">
      {/* Hidden synchronized <audio> elements for zero-glitches per-track Solo/Mute auditioning */}
      {previews
        .filter((p) => Boolean(p.audioPath))
        .map((p) => (
          <audio
            key={`${capture.id}-audio-${p.streamIndex}`}
            ref={(el) => {
              audioRefs.current[p.streamIndex] = el;
            }}
            src={resolveMediaSrc(p.audioPath)}
            preload="auto"
          />
        ))}

      {/* Header Row */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Scissors className="w-4 h-4 text-primary" />
          <span className="text-xs font-semibold tracking-tight">
            Trim &amp; Multi-Track Audio Mixer
          </span>
          <span className="badge badge-xs badge-ghost font-mono">
            {formatPreciseMs(selectedSpanMs)} selected
          </span>
          <span className="text-[11px] opacity-65 font-mono">{estimatedSizeLabel}</span>
          {hasLiveAudioStreams && (
            <span className="badge badge-xs badge-primary/80 gap-1">
              <Headphones className="w-2.5 h-2.5" />
              {soloTrackIdx !== null
                ? `Solo: ${tracks.find((t) => t.streamIndex === soloTrackIdx)?.title ?? `#${soloTrackIdx + 1}`}`
                : "Track Monitor Active"}
            </span>
          )}
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

      {/* Unified Multi-Lane Timeline (Aligned Playhead Across Video + Audio Waveforms) */}
      <div className="rounded-lg bg-base-300/60 border border-base-300 p-2 space-y-1.5">
        {/* Video Scrubber Lane */}
        <div className="flex items-center gap-2.5">
          <div className="w-48 sm:w-56 shrink-0 flex items-center justify-between pr-1">
            <span className="text-[11px] font-semibold uppercase tracking-wider opacity-75">
              Video Clip
            </span>
            <span className="text-[10px] font-mono opacity-70">
              {formatPreciseMs(startMs)} – {formatPreciseMs(endMs)}
            </span>
          </div>

          <div
            ref={trackBarRef}
            onPointerDown={(e) => {
              if ((e.target as HTMLElement).dataset.handle) return;
              seekVideoToMs(clientXToMs(e.clientX));
            }}
            className="relative flex-1 h-7 rounded-md bg-base-300 border border-base-content/10 cursor-pointer select-none touch-none overflow-hidden"
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
              <span className="w-0.5 h-3.5 bg-primary-content/80 rounded-full pointer-events-none" />
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
              <span className="w-0.5 h-3.5 bg-primary-content/80 rounded-full pointer-events-none" />
            </button>
          </div>
        </div>

        {/* Per-Track Audio Waveform Lanes */}
        {probing ? (
          <div className="flex items-center gap-2 text-xs opacity-65 py-1.5 px-1">
            <Loader2 className="w-3.5 h-3.5 animate-spin" />
            Inspecting embedded audio streams...
          </div>
        ) : probeError ? (
          <div className="text-xs text-error py-1.5 px-1">
            Audio track probe failed: {probeError}
          </div>
        ) : tracks.length === 0 ? (
          <div className="text-xs opacity-60 py-1 px-1">
            No embedded audio streams detected (video-only export).
          </div>
        ) : (
          <div className="space-y-1">
            {tracks.map((track) => {
              const preview = previewMap.get(track.streamIndex);
              const peaks = preview?.peaks ?? [];
              const included = exportTracks.includes(track.streamIndex);
              const isSolo = soloTrackIdx === track.streamIndex;
              const isAudible = isTrackAudibleInPreview(track.streamIndex);

              return (
                <div key={track.streamIndex} className="flex items-center gap-2.5">
                  {/* Track Label + Solo / Monitor Mute / Export Controls */}
                  <div className="w-48 sm:w-56 shrink-0 flex items-center justify-between gap-1.5 bg-base-200/80 border border-base-300 rounded-md px-2 py-1">
                    <div className="min-w-0 flex-1">
                      <div className="text-[11px] font-medium truncate" title={track.title}>
                        {track.title}
                      </div>
                      <div className="text-[9px] font-mono opacity-55">
                        Track {track.streamIndex + 1} • {track.codec}
                      </div>
                    </div>

                    <div className="flex items-center gap-1 shrink-0">
                      <button
                        type="button"
                        disabled={!hasLiveAudioStreams}
                        title={isSolo ? "Disable Solo" : "Solo this track in player"}
                        onClick={() => toggleSoloTrack(track.streamIndex)}
                        className={`btn btn-xs h-5 min-h-0 px-1.5 font-mono text-[10px] ${
                          isSolo
                            ? "btn-warning text-warning-content"
                            : "btn-ghost opacity-70 hover:opacity-100"
                        }`}
                      >
                        S
                      </button>
                      <button
                        type="button"
                        disabled={!hasLiveAudioStreams}
                        title={
                          isAudible
                            ? "Mute track in Lightbox preview"
                            : "Listen to track in Lightbox preview"
                        }
                        onClick={() => toggleAuditionMute(track.streamIndex)}
                        className={`btn btn-xs h-5 min-h-0 px-1.5 ${
                          isAudible
                            ? "btn-ghost text-info"
                            : "btn-ghost opacity-45 hover:opacity-85"
                        }`}
                      >
                        {isAudible ? (
                          <Volume2 className="w-3 h-3" />
                        ) : (
                          <VolumeX className="w-3 h-3" />
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
                        className={`btn btn-xs h-5 min-h-0 px-1.5 gap-0.5 text-[10px] ${
                          included ? "btn-primary" : "btn-outline opacity-55"
                        }`}
                      >
                        {included && <Check className="w-2.5 h-2.5" />}
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
                    className={`relative flex-1 h-9 rounded-md bg-base-200/75 border cursor-pointer overflow-hidden flex items-center px-0.5 transition-opacity ${
                      included ? "border-base-content/15" : "border-base-300 opacity-45"
                    }`}
                  >
                    {loadingWaveforms && peaks.length === 0 ? (
                      <div className="w-full text-center text-[10px] opacity-50 font-mono">
                        Generating waveform...
                      </div>
                    ) : peaks.length === 0 ? (
                      <div className="w-full h-px bg-base-content/15" />
                    ) : (
                      <div className="w-full h-7 flex items-center gap-[1px] pointer-events-none">
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

        <div className="flex items-center justify-between text-[11px] font-mono opacity-75 pt-0.5">
          <span>In: {formatPreciseMs(startMs)}</span>
          <span>Playhead: {formatPreciseMs(currentTimeMs)}</span>
          <span>Out: {formatPreciseMs(endMs)} / {formatPreciseMs(effectiveDurationMs)}</span>
        </div>
      </div>

      {/* Mix Options + Target Size Presets + Export Submit */}
      <div className="flex flex-wrap items-center justify-between gap-3 pt-1 border-t border-base-300">
        <div className="space-y-1">
          <div className="flex flex-wrap items-center gap-1.5">
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

          <div className="flex flex-wrap items-center gap-3 text-[11px]">
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
              <span className="opacity-65 flex items-center gap-1">
                <Layers className="w-3 h-3" />
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

      {errorMsg && (
        <div className="alert alert-error py-2 px-3 text-xs">
          <AlertCircle className="w-4 h-4 shrink-0" />
          <span>{errorMsg}</span>
        </div>
      )}
    </div>
  );
};
