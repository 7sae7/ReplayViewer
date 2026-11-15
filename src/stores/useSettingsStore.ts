import { create } from "zustand";
import type { SettingsPayload } from "../types/ipc";
import { getSettings, updateSettings } from "../lib/tauri-client";

interface SettingsState {
  settings: SettingsPayload | null;
  isSaving: boolean;
  loadSettings: () => Promise<void>;
  saveSettings: (next: SettingsPayload) => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set) => ({
  settings: null,
  isSaving: false,

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
}));
