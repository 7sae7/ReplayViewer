import { create } from "zustand";
import type {
  CaptureRecord,
  DateRangeFilter,
  GalleryFilter,
  MediaType,
  SortByField,
  SortDirection,
} from "../types/ipc";
import {
  addTagToCapture,
  batchAddTags,
  batchSetGroup,
  deleteCaptures,
  listCaptures,
  moveCapturesToFolder,
  reconcileOrphanedCaptures,
  seedSyntheticBenchmarkItems,
  togglePinCapture,
} from "../lib/tauri-client";

interface GalleryState {
  items: CaptureRecord[];
  groups: string[];
  totalCount: number;
  totalBytes: number;
  isLoading: boolean;

  // Filters & Sorting
  searchQuery: string;
  mediaTypeFilter: MediaType | undefined;
  onlyPinned: boolean;
  onlyRecentClipboard: boolean;
  activeGroup: string | undefined;
  activeTag: string | undefined;
  dateRange: DateRangeFilter;
  sortBy: SortByField;
  sortDir: SortDirection;

  // Layout & Multi-Selection
  columnCount: number | "auto";
  selectedIds: string[];
  selectedCapture: CaptureRecord | null;
  lastReconciledCount: number | null;

  // Actions
  fetchCaptures: () => Promise<void>;
  setSearchQuery: (query: string) => Promise<void>;
  setMediaTypeFilter: (mediaType: MediaType | undefined) => Promise<void>;
  setOnlyPinned: (onlyPinned: boolean) => Promise<void>;
  setOnlyRecentClipboard: (onlyRecentClipboard: boolean) => Promise<void>;
  setActiveGroup: (groupName: string | undefined) => Promise<void>;
  setActiveTag: (tag: string | undefined) => Promise<void>;
  setDateRange: (dateRange: DateRangeFilter) => Promise<void>;
  setSortBy: (sortBy: SortByField) => Promise<void>;
  toggleSortDir: () => Promise<void>;
  setColumnCount: (cols: number | "auto") => void;

  // Multi-Select Actions
  setSelectedIds: (ids: string[]) => void;
  toggleSelectId: (id: string) => void;
  selectAllVisible: () => void;
  clearSelection: () => void;
  batchTagSelected: (tags: string[]) => Promise<number>;
  batchGroupSelected: (groupName: string) => Promise<number>;
  batchMoveSelectedToFolder: (targetDir: string) => Promise<number>;

  // Single / Shared Actions
  setSelectedCapture: (capture: CaptureRecord | null) => void;
  togglePin: (id: string, currentPinned: boolean) => Promise<void>;
  removeCaptures: (ids: string[]) => Promise<void>;
  addTag: (captureId: string, tagName: string) => Promise<void>;
  reconcileOrphans: () => Promise<number>;
  runBenchmarkSeed: (count?: number) => Promise<void>;
}

export const useGalleryStore = create<GalleryState>((set, get) => ({
  items: [],
  groups: [],
  totalCount: 0,
  totalBytes: 0,
  isLoading: false,
  searchQuery: "",
  mediaTypeFilter: undefined,
  onlyPinned: false,
  onlyRecentClipboard: false,
  activeGroup: undefined,
  activeTag: undefined,
  dateRange: "all",
  sortBy: "created_at",
  sortDir: "desc",
  columnCount: "auto",
  selectedIds: [],
  selectedCapture: null,
  lastReconciledCount: null,

  fetchCaptures: async () => {
    set({ isLoading: true });
    try {
      const state = get();
      const filter: GalleryFilter = {
        searchQuery: state.searchQuery || undefined,
        mediaType: state.mediaTypeFilter,
        onlyPinned: state.onlyPinned,
        onlyRecentClipboard: state.onlyRecentClipboard,
        groupName: state.activeGroup,
        tag: state.activeTag,
        dateRange: state.dateRange === "all" ? undefined : state.dateRange,
        sortBy: state.sortBy,
        sortDir: state.sortDir,
      };
      const result = await listCaptures(null, 5000, filter);
      set({
        items: result.items,
        groups: result.groups || [],
        totalCount: result.totalCount,
        totalBytes: result.totalBytes,
        isLoading: false,
      });
    } catch (err) {
      console.error("Failed to fetch captures:", err);
      set({ isLoading: false });
    }
  },

  setSearchQuery: async (searchQuery) => {
    set({ searchQuery });
    await get().fetchCaptures();
  },

  setMediaTypeFilter: async (mediaTypeFilter) => {
    set({ mediaTypeFilter });
    await get().fetchCaptures();
  },

  setOnlyPinned: async (onlyPinned) => {
    set({ onlyPinned });
    await get().fetchCaptures();
  },

  setOnlyRecentClipboard: async (onlyRecentClipboard) => {
    set({ onlyRecentClipboard });
    await get().fetchCaptures();
  },

  setActiveGroup: async (activeGroup) => {
    set({ activeGroup });
    await get().fetchCaptures();
  },

  setActiveTag: async (activeTag) => {
    set({ activeTag });
    await get().fetchCaptures();
  },

  setDateRange: async (dateRange) => {
    set({ dateRange });
    await get().fetchCaptures();
  },

  setSortBy: async (sortBy) => {
    set({ sortBy });
    await get().fetchCaptures();
  },

  toggleSortDir: async () => {
    const nextDir: SortDirection = get().sortDir === "desc" ? "asc" : "desc";
    set({ sortDir: nextDir });
    await get().fetchCaptures();
  },

  setColumnCount: (columnCount) => set({ columnCount }),

  setSelectedIds: (selectedIds) => set({ selectedIds }),

  toggleSelectId: (id) =>
    set((state) => ({
      selectedIds: state.selectedIds.includes(id)
        ? state.selectedIds.filter((item) => item !== id)
        : [...state.selectedIds, id],
    })),

  selectAllVisible: () =>
    set((state) => ({
      selectedIds: state.items.map((i) => i.id),
    })),

  clearSelection: () => set({ selectedIds: [] }),

  batchTagSelected: async (tags) => {
    const ids = get().selectedIds;
    if (ids.length === 0) return 0;
    const count = await batchAddTags(ids, tags);
    await get().fetchCaptures();
    return count;
  },

  batchGroupSelected: async (groupName) => {
    const ids = get().selectedIds;
    if (ids.length === 0) return 0;
    const count = await batchSetGroup(ids, groupName);
    await get().fetchCaptures();
    return count;
  },

  batchMoveSelectedToFolder: async (targetDir) => {
    const ids = get().selectedIds;
    if (ids.length === 0) return 0;
    const moved = await moveCapturesToFolder(ids, targetDir);
    set({ selectedIds: [] });
    await get().fetchCaptures();
    return moved;
  },

  setSelectedCapture: (selectedCapture) => set({ selectedCapture }),

  togglePin: async (id, currentPinned) => {
    const nextPinned = !currentPinned;
    await togglePinCapture(id, nextPinned);
    set((state) => ({
      items: state.items.map((item) =>
        item.id === id ? { ...item, isPinned: nextPinned } : item
      ),
      selectedCapture:
        state.selectedCapture?.id === id
          ? { ...state.selectedCapture, isPinned: nextPinned }
          : state.selectedCapture,
    }));
  },

  removeCaptures: async (ids) => {
    await deleteCaptures(ids);
    set((state) => ({
      items: state.items.filter((item) => !ids.includes(item.id)),
      selectedIds: state.selectedIds.filter((id) => !ids.includes(id)),
      totalCount: Math.max(0, state.totalCount - ids.length),
      selectedCapture:
        state.selectedCapture && ids.includes(state.selectedCapture.id)
          ? null
          : state.selectedCapture,
    }));
    await get().fetchCaptures();
  },

  addTag: async (captureId, tagName) => {
    const updated = await addTagToCapture(captureId, tagName);
    set((state) => ({
      items: state.items.map((item) =>
        item.id === captureId ? updated : item
      ),
      selectedCapture:
        state.selectedCapture?.id === captureId
          ? updated
          : state.selectedCapture,
    }));
  },

  reconcileOrphans: async () => {
    const pruned = await reconcileOrphanedCaptures();
    set({ lastReconciledCount: pruned });
    await get().fetchCaptures();
    return pruned;
  },

  runBenchmarkSeed: async (count = 500) => {
    await seedSyntheticBenchmarkItems(count);
    await get().fetchCaptures();
  },
}));
