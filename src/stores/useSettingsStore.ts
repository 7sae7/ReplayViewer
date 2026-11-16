import { create } from "zustand";
import type {
  FfmpegStatus,
  ReplayBufferSnapshot,
  ReplayRuntimeState,
  SettingsPayload,
} from "../types/ipc";
import {
  downloadFfmpegEngine,
  getFfmpegStatus,
  getReplayBufferSnapshot,
  getSettings,
  updateSettings,
} from "../lib/tauri-client";

interface SettingsState {
  settings: SettingsPayload | null;
  isSaving: boolean;
  ffmpegStatus: FfmpegStatus | null;
  replaySnapshot: ReplayBufferSnapshot | null;
  isDownloadingFfmpeg: boolean;
  downloadStageMsg: string | null;
  loadSettings: () => Promise<void>;
  saveSettings: (next: SettingsPayload) => Promise<void>;
  refreshReplayStatus: (forceFfmpegProbe?: boolean) => Promise<void>;
  startFfmpegDownload: () => Promise<{
    status: FfmpegStatus | null;
    error?: string;
  }>;
  setReplayRuntime: (runtime: ReplayRuntimeState) => void;
  setDownloadProgress: (stage: string, message: string) => void;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: null,
  isSaving: false,
  ffmpegStatus: null,
  replaySnapshot: null,
  isDownloadingFfmpeg: false,
  downloadStageMsg: null,

  loadSettings: async () => {
    const settings = await getSettings();
    set({ settings });
  },

  saveSettings: async (next) => {
    set({ isSaving: true });
    try {
      const updated = await updateSettings(next);
      set({ settings: updated, isSaving: false });
    } catch (err) {
      console.error("Failed to save settings:", err);
      set({ isSaving: false });
    }
  },

  refreshReplayStatus: async (forceFfmpegProbe = false) => {
    try {
      const [ff, snap] = await Promise.all([
        getFfmpegStatus(forceFfmpegProbe),
        getReplayBufferSnapshot(),
      ]);
      set((state) => ({
        ffmpegStatus: ff,
        replaySnapshot: snap,
        isDownloadingFfmpeg: ff.downloading ? true : state.isDownloadingFfmpeg,
        downloadStageMsg: ff.downloading
          ? ff.downloadStage || state.downloadStageMsg
          : state.isDownloadingFfmpeg
          ? state.downloadStageMsg
          : null,
      }));
    } catch {
      // Ignore background refresh errors
    }
  },

  startFfmpegDownload: async () => {
    if (get().isDownloadingFfmpeg) {
      return { status: get().ffmpegStatus };
    }
    set({
      isDownloadingFfmpeg: true,
      downloadStageMsg: "Connecting to release mirror (~45 MB LGPL)...",
    });
    try {
      const status = await downloadFfmpegEngine();
      set({
        ffmpegStatus: status,
        isDownloadingFfmpeg: false,
        downloadStageMsg: null,
      });
      await get().refreshReplayStatus(false);
      return { status };
    } catch (err) {
      set({
        isDownloadingFfmpeg: false,
        downloadStageMsg: null,
      });
      return { status: null, error: String(err) };
    }
  },

  setReplayRuntime: (runtime) => {
    set((state) => ({
      replaySnapshot: state.replaySnapshot
        ? { ...state.replaySnapshot, runtime }
        : state.replaySnapshot,
    }));
  },

  setDownloadProgress: (stage, message) => {
    set({
      downloadStageMsg: message,
      isDownloadingFfmpeg: stage === "downloading" || stage === "extracting",
    });
  },
}));
