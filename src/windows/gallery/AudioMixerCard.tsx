import React, { useCallback, useEffect, useRef, useState } from "react";
import {
  ChevronDown,
  Headphones,
  Mic,
  MicOff,
  Radio,
  RefreshCw,
  Trash2,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import {
  getLiveAudioPeaks,
  getMicTestLevel,
  listActiveAudioSessions,
  listAudioInputDevices,
  setAudioTrackVolumes,
  setMicTestGain,
  setMicTestSidetone,
  startMicTest,
  stopMicTest,
  updateAppAudioRule,
} from "../../lib/tauri-client";
import { useSettingsStore } from "../../stores/useSettingsStore";
import type {
  AudioInputDevice,
  AudioSessionInfo,
  SettingsPayload,
} from "../../types/ipc";

interface AudioMixerCardProps {
  form: SettingsPayload;
  onFormChange: (next: SettingsPayload) => void;
}

/** Converts linear WASAPI amplitude (0..1) into a perceptual -48 dBFS .. 0 dBFS bar percentage. */
function peakToMeterPercent(peak: number): number {
  if (peak <= 0.0001) return 0;
  const db = 20 * Math.log10(peak);
  return Math.max(0, Math.min(100, Math.round(((db + 48) / 48) * 100)));
}

export const AudioMixerCard: React.FC<AudioMixerCardProps> = React.memo(
  ({ form, onFormChange }) => {
    const { loadSettings } = useSettingsStore();
    const [audioSessions, setAudioSessions] = useState<AudioSessionInfo[]>([]);
    const [livePeaks, setLivePeaks] = useState<Record<string, number>>({});
    const [micDevices, setMicDevices] = useState<AudioInputDevice[]>([]);
    const [showIdleAudio, setShowIdleAudio] = useState(false);
    const [isScanning, setIsScanning] = useState(false);
    const [scanNote, setScanNote] = useState<string | null>(null);
    const [isMicTesting, setIsMicTesting] = useState(false);
    const [micSidetone, setMicSidetone] = useState(false);
    const [micTestPeak, setMicTestPeak] = useState(0);
    const [micSignalDetected, setMicSignalDetected] = useState(false);
    const volCommitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const pendingIsolateRef = useRef<Set<string>>(new Set());

    const isBufferActive = form.replayEnabled ?? true;
    const desktopVol = form.audioDesktopVolume ?? 1.0;
    const micVol = form.audioMicVolume ?? 1.0;
    const appVolumes = form.audioAppVolumes ?? {};
    const maxIsolatedSlots = 4;
    const configuredIsolated = form.audioIsolatedApps || [];
    const isolatedCount = configuredIsolated.length;
    const slotsFull = isolatedCount >= maxIsolatedSlots;
    const [slotAlert, setSlotAlert] = useState<string | null>(null);
    const [showTracksDropdown, setShowTracksDropdown] = useState(false);

    const scanSessions = useCallback(async () => {
      setIsScanning(true);
      try {
        const [list, mics] = await Promise.all([
          listActiveAudioSessions(showIdleAudio),
          listAudioInputDevices(),
        ]);
        setAudioSessions(list);
        setMicDevices(mics);
        const initPeaks: Record<string, number> = {};
        for (const s of list) {
          initPeaks[s.exeName.toLowerCase()] = s.peakLevel;
        }
        setLivePeaks(initPeaks);
        setScanNote(
          list.length > 0
            ? `Found ${list.length} app audio session(s)`
            : "No active app audio streams detected right now"
        );
      } catch {
        setScanNote("Failed to query OS audio sessions");
      } finally {
        setIsScanning(false);
      }
    }, [showIdleAudio]);

    // Initial load when card mounts or idle toggle changes
    useEffect(() => {
      scanSessions();
    }, [scanSessions]);

    // Fast ~14 Hz peak-only meter polling (0 SQLite locks) + slow 3s session list discovery while buffer active
    useEffect(() => {
      if (!isBufferActive) return;
      const peakTimer = setInterval(() => {
        getLiveAudioPeaks()
          .then((fresh) => {
            setLivePeaks((prev) => {
              const next: Record<string, number> = { ...fresh };
              // Smooth decay for apps briefly between buffers so meters feel organic
              for (const [k, prevVal] of Object.entries(prev)) {
                const cur = next[k] ?? 0;
                if (prevVal * 0.78 > cur) {
                  next[k] = prevVal * 0.78;
                }
              }
              return next;
            });
          })
          .catch(() => {});
      }, 70);

      const discoveryTimer = setInterval(() => {
        listActiveAudioSessions(showIdleAudio)
          .then(setAudioSessions)
          .catch(() => {});
      }, 3000);

      return () => {
        clearInterval(peakTimer);
        clearInterval(discoveryTimer);
      };
    }, [isBufferActive, showIdleAudio]);

    // Poll WASAPI mic test level at ~20 Hz while Microphone Test is active, and cleanup on unmount
    useEffect(() => {
      if (!isMicTesting) {
        setMicTestPeak(0);
        return;
      }
      const timer = setInterval(() => {
        getMicTestLevel()
          .then((lvl) => {
            setMicTestPeak(lvl);
            if (lvl > 0.03) {
              setMicSignalDetected(true);
            }
          })
          .catch(() => {});
      }, 50);
      return () => {
        clearInterval(timer);
      };
    }, [isMicTesting]);

    useEffect(() => {
      return () => {
        stopMicTest().catch(() => {});
      };
    }, []);

    const handleToggleMicTest = async () => {
      if (isMicTesting) {
        await stopMicTest();
        setIsMicTesting(false);
        setMicTestPeak(0);
      } else {
        setMicSignalDetected(false);
        await startMicTest(form.audioMicDeviceId || "", micSidetone);
        setIsMicTesting(true);
      }
    };

    const handleMicDeviceChange = async (newDeviceId: string) => {
      onFormChange({ ...form, audioMicDeviceId: newDeviceId });
      if (isMicTesting) {
        setMicSignalDetected(false);
        await startMicTest(newDeviceId, micSidetone);
      }
    };

    const handleToggleAppMute = async (sess: AudioSessionInfo) => {
      setSlotAlert(null);
      const nextMuted = !sess.isMuted;
      await updateAppAudioRule(sess.exeName, { muted: nextMuted });
      await loadSettings();
      await scanSessions();
    };

    const handleToggleAppIsolate = async (sess: AudioSessionInfo) => {
      const nextIso = !sess.isIsolated;
      if (nextIso && slotsFull) {
        setSlotAlert(
          "All 4 Isolated track slots are in use. Open Tracks to remove one or Clear Tracks."
        );
        return;
      }
      setSlotAlert(null);
      try {
        await updateAppAudioRule(sess.exeName, { isolated: nextIso });
        await loadSettings();
        await scanSessions();
      } catch (err) {
        setSlotAlert(String(err));
      }
    };

    const handleRemoveIsolatedApp = async (exeName: string) => {
      setSlotAlert(null);
      await updateAppAudioRule(exeName, { isolated: false });
      await loadSettings();
      await scanSessions();
    };

    const handleClearAllTracks = async () => {
      setSlotAlert(null);
      for (const exe of configuredIsolated) {
        await updateAppAudioRule(exe, { isolated: false });
      }
      await loadSettings();
      await scanSessions();
      setShowTracksDropdown(false);
    };

    const scheduleVolumeCommit = (opts: {
      desktopVolume?: number;
      micVolume?: number;
      appExeName?: string;
      appVolume?: number;
    }) => {
      if (volCommitTimerRef.current) {
        clearTimeout(volCommitTimerRef.current);
      }
      volCommitTimerRef.current = setTimeout(() => {
        setAudioTrackVolumes(opts).catch(() => {});
      }, 150);
    };

    const handleDesktopVolumeChange = (val: number) => {
      onFormChange({ ...form, audioDesktopVolume: val });
      scheduleVolumeCommit({ desktopVolume: val });
    };

    const handleMicVolumeChange = (val: number) => {
      onFormChange({ ...form, audioMicVolume: val });
      setMicTestGain(val).catch(() => {});
      scheduleVolumeCommit({ micVolume: val });
    };

    const handleAppVolumeChange = async (sess: AudioSessionInfo, val: number) => {
      const key = sess.exeName.toLowerCase();
      if (!sess.isIsolated && slotsFull && Math.abs(val - 1.0) > 0.02) {
        setSlotAlert(
          "All 4 Isolated track slots are in use. Free a slot from Tracks before adjusting an un-isolated app's Track Vol."
        );
        return;
      }
      setSlotAlert(null);
      onFormChange({
        ...form,
        audioAppVolumes: { ...appVolumes, [key]: val },
      });
      scheduleVolumeCommit({ appExeName: sess.exeName, appVolume: val });
      // Auto-isolate once if un-isolated so independent track gain applies cleanly without restarting on every drag tick
      if (
        Math.abs(val - 1.0) > 0.02 &&
        !sess.isIsolated &&
        !sess.isMuted &&
        !pendingIsolateRef.current.has(key)
      ) {
        pendingIsolateRef.current.add(key);
        try {
          await updateAppAudioRule(sess.exeName, { isolated: true });
          await loadSettings();
          await scanSessions();
        } catch (err) {
          setSlotAlert(String(err));
        } finally {
          pendingIsolateRef.current.delete(key);
        }
      }
    };

    return (
      <div className="card bg-base-200 border border-base-300 shadow-sm">
        <div className="card-body p-5 space-y-3 text-xs">
          <div className="flex items-center justify-between gap-2">
            <h3 className="card-title text-sm flex items-center gap-2">
              <Volume2 className="w-4 h-4 text-accent" />
              Dynamic Audio Track Router (v2.0.2)
            </h3>
            <div className="flex items-center gap-1.5">
              <span
                className={`badge badge-xs font-mono ${
                  isBufferActive ? "badge-success" : "badge-ghost"
                }`}
                title="Dynamic Audio Track Router runs automatically whenever Hardware Replay Buffer is active"
              >
                {isBufferActive ? "Synced w/ Buffer" : "Standby"}
              </span>
              <button
                type="button"
                disabled={isScanning}
                onClick={scanSessions}
                className="btn btn-xs btn-ghost border border-base-300 gap-1"
              >
                <RefreshCw
                  className={`w-3 h-3 ${isScanning ? "animate-spin" : ""}`}
                />
                {isScanning ? "Scanning..." : "Scan Apps"}
              </button>
            </div>
          </div>

          {/* Base Desktop + Mic + Single-Mix Toggles */}
          <div className="grid grid-cols-3 gap-2">
            <label className="flex items-center justify-between p-2 rounded-lg bg-base-100 border border-base-300 cursor-pointer">
              <span className="flex items-center gap-1 font-medium text-[11px]">
                <Volume2 className="w-3.5 h-3.5 text-primary" /> Desktop
              </span>
              <input
                type="checkbox"
                checked={form.audioRecordDesktop ?? true}
                onChange={(e) =>
                  onFormChange({
                    ...form,
                    audioRecordDesktop: e.target.checked,
                  })
                }
                className="toggle toggle-primary toggle-xs"
              />
            </label>

            <label className="flex items-center justify-between p-2 rounded-lg bg-base-100 border border-base-300 cursor-pointer">
              <span className="flex items-center gap-1 font-medium text-[11px]">
                <Mic className="w-3.5 h-3.5 text-success" /> Mic
              </span>
              <input
                type="checkbox"
                checked={form.audioRecordMic ?? true}
                onChange={(e) =>
                  onFormChange({ ...form, audioRecordMic: e.target.checked })
                }
                className="toggle toggle-success toggle-xs"
              />
            </label>

            <label
              title="Mix all unmuted audio into Track 1 (instant Discord/web compatibility) instead of separate AAC tracks"
              className="flex items-center justify-between p-2 rounded-lg bg-base-100 border border-base-300 cursor-pointer"
            >
              <span className="flex items-center gap-1 font-medium text-[11px]">
                <Radio className="w-3.5 h-3.5 text-warning" /> Single Mix
              </span>
              <input
                type="checkbox"
                checked={form.audioSingleMix ?? false}
                onChange={(e) =>
                  onFormChange({ ...form, audioSingleMix: e.target.checked })
                }
                className="toggle toggle-warning toggle-xs"
              />
            </label>
          </div>

          {/* Base Track Volume Gain Sliders (Desktop & Microphone) */}
          <div className="grid grid-cols-2 gap-2">
            <div className="p-2 rounded-lg bg-base-100 border border-base-300 space-y-1">
              <div className="flex items-center justify-between text-[10px]">
                <span className="font-medium text-base-content/80">Desktop Track Gain</span>
                <span className="font-mono font-semibold text-primary">
                  {Math.round(desktopVol * 100)}%
                </span>
              </div>
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                disabled={!(form.audioRecordDesktop ?? true)}
                value={desktopVol}
                onChange={(e) => handleDesktopVolumeChange(Number(e.target.value))}
                className="range range-primary range-xs w-full"
              />
            </div>

            <div className="p-2 rounded-lg bg-base-100 border border-base-300 space-y-1">
              <div className="flex items-center justify-between text-[10px]">
                <span className="font-medium text-base-content/80">Microphone Track Gain</span>
                <span className="font-mono font-semibold text-success">
                  {Math.round(micVol * 100)}%
                </span>
              </div>
              <input
                type="range"
                min={0}
                max={2}
                step={0.05}
                disabled={!(form.audioRecordMic ?? true)}
                value={micVol}
                onChange={(e) => handleMicVolumeChange(Number(e.target.value))}
                className="range range-success range-xs w-full"
              />
            </div>
          </div>

          {/* Microphone Input Device Selector */}
          <div className="space-y-1.5">
            <label className="text-[11px] font-medium text-base-content/75 flex items-center gap-1.5">
              <Mic className="w-3 h-3 text-success" />
              Microphone Input Channel
            </label>
            <select
              disabled={!(form.audioRecordMic ?? true)}
              value={form.audioMicDeviceId ?? ""}
              onChange={(e) => handleMicDeviceChange(e.target.value)}
              className="select select-xs select-bordered w-full bg-base-100 text-[11px]"
            >
              <option value="">System Default Communication / Console Mic</option>
              {micDevices.map((dev) => (
                <option key={dev.id} value={dev.id}>
                  {dev.name}
                  {dev.isDefault ? " (Windows Default)" : ""}
                </option>
              ))}
            </select>

            {/* Microphone Test (WASAPI live input meter + optional Sidetone) */}
            <div className="p-2.5 rounded-lg bg-base-100 border border-base-300 space-y-2">
              <div className="flex items-center justify-between gap-2">
                <div className="flex items-center gap-2">
                  <button
                    type="button"
                    disabled={!(form.audioRecordMic ?? true)}
                    onClick={handleToggleMicTest}
                    className={`btn btn-xs gap-1 ${
                      isMicTesting ? "btn-error" : "btn-outline border-base-300"
                    }`}
                  >
                    {isMicTesting ? (
                      <>
                        <MicOff className="w-3 h-3" /> Stop Test
                      </>
                    ) : (
                      <>
                        <Mic className="w-3 h-3 text-success" /> Microphone Test
                      </>
                    )}
                  </button>
                  <label
                    className="flex items-center gap-1 cursor-pointer text-[10px] text-base-content/75"
                    title="Play captured microphone audio back through your speakers/headset"
                  >
                    <input
                      type="checkbox"
                      disabled={!(form.audioRecordMic ?? true)}
                      checked={micSidetone}
                      onChange={async (e) => {
                        const next = e.target.checked;
                        setMicSidetone(next);
                        if (isMicTesting) {
                          await setMicTestSidetone(next);
                        }
                      }}
                      className="checkbox checkbox-xs checkbox-success"
                    />
                    <Headphones className="w-3 h-3 text-info" />
                    Hear Yourself
                  </label>
                </div>
                <span
                  className={`font-mono text-[10px] ${
                    isMicTesting
                      ? micSignalDetected
                        ? "text-success font-semibold"
                        : "text-warning"
                      : "text-base-content/50"
                  }`}
                >
                  {isMicTesting
                    ? `${peakToMeterPercent(micTestPeak)}% • ${
                        micSignalDetected ? "Signal OK" : "Speak now..."
                      }`
                    : "Idle"}
                </span>
              </div>
              <progress
                className={`progress w-full h-1.5 ${
                  micTestPeak > 0.85
                    ? "progress-error"
                    : micTestPeak > 0.05
                    ? "progress-success"
                    : "progress-info"
                }`}
                value={peakToMeterPercent(micTestPeak)}
                max={100}
              />
            </div>
          </div>

          {/* Live Detected Audio-Emitting Apps */}
          <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-[11px] font-semibold text-base-content/75">
                Audio-Emitting Apps ({audioSessions.length})
              </span>
              <div className="relative">
                <button
                  type="button"
                  onClick={() => setShowTracksDropdown((v) => !v)}
                  className={`btn btn-xs h-5 min-h-0 px-2 gap-1 font-mono text-[10px] ${
                    slotsFull
                      ? "btn-warning"
                      : isolatedCount > 0
                      ? "btn-accent btn-soft"
                      : "btn-ghost border border-base-300"
                  }`}
                  title="Inspect or clear which apps are holding the 4 Isolated Track slots (including closed apps)"
                >
                  Tracks: {isolatedCount}/{maxIsolatedSlots}
                  {slotsFull ? " (Full)" : ""}
                  <ChevronDown className="w-3 h-3" />
                </button>

                {showTracksDropdown && (
                  <div className="absolute left-0 top-6 z-30 w-64 rounded-lg bg-base-100 border border-base-300 shadow-lg p-2 space-y-1.5">
                    <div className="flex items-center justify-between border-b border-base-300 pb-1">
                      <span className="text-[10px] font-semibold text-base-content/80">
                        Assigned Isolated Slots ({isolatedCount}/{maxIsolatedSlots})
                      </span>
                      {isolatedCount > 0 && (
                        <button
                          type="button"
                          onClick={handleClearAllTracks}
                          className="btn btn-xs h-5 min-h-0 px-1.5 btn-error btn-soft gap-1 text-[10px]"
                        >
                          <Trash2 className="w-2.5 h-2.5" />
                          Clear Tracks
                        </button>
                      )}
                    </div>
                    {configuredIsolated.length === 0 ? (
                      <div className="text-[10px] text-base-content/55 py-1.5 text-center">
                        No isolated apps assigned. Click <strong>Isolate</strong> on an app below to bind a slot.
                      </div>
                    ) : (
                      <div className="space-y-1 max-h-40 overflow-y-auto">
                        {configuredIsolated.map((exe, idx) => {
                          const isRunning = audioSessions.some(
                            (s) => s.exeName.toLowerCase() === exe.trim().toLowerCase()
                          );
                          return (
                            <div
                              key={`${idx}-${exe}`}
                              className="flex items-center justify-between gap-1.5 px-1.5 py-1 rounded bg-base-200/70 border border-base-300"
                            >
                              <div className="min-w-0 flex items-center gap-1.5">
                                <span className="font-mono text-[10px] font-semibold truncate">
                                  #{idx + 1} {exe}
                                </span>
                                <span
                                  className={`badge badge-xs text-[9px] ${
                                    isRunning ? "badge-success badge-soft" : "badge-ghost"
                                  }`}
                                >
                                  {isRunning ? "Active" : "Closed"}
                                </span>
                              </div>
                              <button
                                type="button"
                                onClick={() => handleRemoveIsolatedApp(exe)}
                                title={`Remove ${exe} from Isolated slots`}
                                className="btn btn-xs btn-square h-5 w-5 min-h-0 btn-ghost text-error"
                              >
                                <X className="w-3 h-3" />
                              </button>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                )}
              </div>
              {isolatedCount > 0 && (
                <button
                  type="button"
                  onClick={handleClearAllTracks}
                  title="Clear all Isolated track slots (including closed apps)"
                  className="btn btn-xs h-5 min-h-0 px-1.5 btn-ghost border border-base-300 text-[10px] gap-1 text-base-content/75 hover:text-error"
                >
                  <Trash2 className="w-2.5 h-2.5" />
                  Clear Tracks
                </button>
              )}
              {scanNote && (
                <span className="text-[10px] text-base-content/50 font-mono">
                  {scanNote}
                </span>
              )}
            </div>
            <label className="flex items-center gap-1.5 cursor-pointer text-[10px] text-base-content/60">
              <input
                type="checkbox"
                checked={showIdleAudio}
                onChange={(e) => setShowIdleAudio(e.target.checked)}
                className="checkbox checkbox-xs"
              />
              Show Idle/Paused Sessions
            </label>
          </div>

          {slotAlert && (
            <div className="alert alert-warning py-1.5 px-2.5 text-[11px]">
              <span>{slotAlert}</span>
            </div>
          )}

          <div className="space-y-1.5 max-h-40 overflow-y-auto pr-1">
            {audioSessions.length === 0 ? (
              <div className="p-3 rounded-lg bg-base-100 border border-base-300 text-[11px] text-base-content/55 text-center">
                No active apps emitting audio right now. Click{" "}
                <strong>Scan Apps</strong> while audio is playing—non-audio apps
                (Notepad, Calculator) stay hidden automatically.
              </div>
            ) : (
              audioSessions.map((sess) => {
                const exeKey = sess.exeName.toLowerCase();
                const appVol = appVolumes[exeKey] ?? 1.0;
                const rawPeak = livePeaks[exeKey] ?? sess.peakLevel;
                const effectivePeak = sess.isMuted ? 0 : Math.min(1, rawPeak * appVol);
                return (
                <div
                  key={`${sess.pid}-${sess.exeName}`}
                  className="p-2 rounded-lg bg-base-100 border border-base-300 space-y-1.5"
                >
                  <div className="flex items-center justify-between gap-2">
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-1.5">
                        <span className="font-mono font-semibold text-[11px] truncate">
                          {sess.exeName}
                        </span>
                        {rawPeak > 0.002 && !sess.isMuted && (
                          <span className="w-1.5 h-1.5 rounded-full bg-success shrink-0" />
                        )}
                      </div>
                      <progress
                        className="progress progress-accent w-full h-1.5 mt-1 transition-all duration-75"
                        value={peakToMeterPercent(effectivePeak)}
                        max={100}
                      />
                    </div>

                    <div className="flex items-center gap-1 shrink-0">
                      <button
                        type="button"
                        disabled={!sess.isIsolated && slotsFull}
                        onClick={() => handleToggleAppIsolate(sess)}
                        title={
                          !sess.isIsolated && slotsFull
                            ? "All 4 Isolated track slots are in use — remove one from Tracks first"
                            : "Route this app onto its own dedicated AAC audio track"
                        }
                        className={`btn btn-xs h-6 min-h-0 px-2 font-mono text-[10px] ${
                          sess.isIsolated
                            ? "btn-accent"
                            : "btn-ghost border border-base-300 disabled:opacity-40"
                        }`}
                      >
                        {sess.isIsolated ? "Isolated" : slotsFull ? "Slots Full" : "Isolate"}
                      </button>
                      <button
                        type="button"
                        onClick={() => handleToggleAppMute(sess)}
                        title="Exclude/mute this app at capture time"
                        className={`btn btn-xs btn-square h-6 w-6 min-h-0 ${
                          sess.isMuted
                            ? "btn-error"
                            : "btn-ghost border border-base-300"
                        }`}
                      >
                        {sess.isMuted ? (
                          <VolumeX className="w-3 h-3" />
                        ) : (
                          <Volume2 className="w-3 h-3" />
                        )}
                      </button>
                    </div>
                  </div>

                  {/* Per-App Track Volume Slider */}
                  <div className="flex items-center gap-2 pt-0.5">
                    <span className="text-[10px] text-base-content/55 w-14 shrink-0">
                      Track Vol
                    </span>
                    <input
                      type="range"
                      min={0}
                      max={2}
                      step={0.05}
                      disabled={sess.isMuted}
                      value={appVol}
                      onChange={(e) =>
                        handleAppVolumeChange(sess, Number(e.target.value))
                      }
                      className="range range-accent range-xs flex-1"
                    />
                    <span className="font-mono text-[10px] w-9 text-right text-base-content/75">
                      {Math.round(appVol * 100)}%
                    </span>
                  </div>
                </div>
                );
              })
            )}
          </div>
          <p className="text-[10px] text-base-content/55">
            <strong>Master Mix</strong> blends Desktop + Mic cleanly without
            echo; <strong>Isolated</strong> adds a separate discrete track for
            the v2.0.3 Trimmer &amp; video editors. Windows supports excluding 1
            muted app tree from Desktop Audio at a time.
          </p>
        </div>
      </div>
    );
  }
);
