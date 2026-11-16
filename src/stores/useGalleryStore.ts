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
  restoreQueuedCaptures,
  seedSyntheticBenchmarkItems,
  togglePinCapture,
} from "../lib/tauri-client";

interface GalleryState {
  items: CaptureRecord[];
  groups: string[];
  totalCount: number;
  totalBytes: number;
  queuedDeletionCount: number;
  queuedDeletionBytes: number;
  isLoading: boolean;

  // Filters & Sorting
  searchQuery: string;
  mediaTypeFilter: MediaType | undefined;
  onlyPinned: boolean;
  onlyRecentClipboard: boolean;
  onlyQueuedDeletion: boolean;
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
  setOnlyQueuedDeletion: (onlyQueuedDeletion: boolean) => Promise<void>;
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
  restoreQueued: (ids: string[], alsoPin?: boolean) => Promise<number>;
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
  queuedDeletionCount: 0,
  queuedDeletionBytes: 0,
  isLoading: false,
  searchQuery: "",
  mediaTypeFilter: undefined,
  onlyPinned: false,
  onlyRecentClipboard: false,
  onlyQueuedDeletion: false,
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
        onlyQueuedDeletion: state.onlyQueuedDeletion,
        groupName: state.activeGroup,
        tag: state.activeTag,
        dateRange: state.dateRange === "all" ? undefined : state.dateRange,
        sortBy: state.sortBy,
        sortDir: state.sortDir,
      };
      const result = await listCaptures(null, 5000, filter);
      set((prev) => ({
        items: result.items,
        groups: result.groups || [],
        totalCount: result.totalCount,
        totalBytes: result.totalBytes,
        queuedDeletionCount: result.queuedDeletionCount ?? 0,
        queuedDeletionBytes: result.queuedDeletionBytes ?? 0,
        selectedCapture: prev.selectedCapture
          ? result.items.find((i) => i.id === prev.selectedCapture?.id) ?? prev.selectedCapture
          : null,
        isLoading: false,
      }));
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
    set({ mediaTypeFilter, onlyQueuedDeletion: false });
    await get().fetchCaptures();
  },

  setOnlyPinned: async (onlyPinned) => {
    set({
      onlyPinned,
      onlyQueuedDeletion: onlyPinned ? false : get().onlyQueuedDeletion,
    });
    await get().fetchCaptures();
  },

  setOnlyRecentClipboard: async (onlyRecentClipboard) => {
    set({
      onlyRecentClipboard,
      onlyQueuedDeletion: onlyRecentClipboard ? false : get().onlyQueuedDeletion,
    });
    await get().fetchCaptures();
  },

  setOnlyQueuedDeletion: async (onlyQueuedDeletion) => {
    set({
      onlyQueuedDeletion,
      onlyPinned: onlyQueuedDeletion ? false : get().onlyPinned,
      onlyRecentClipboard: onlyQueuedDeletion ? false : get().onlyRecentClipboard,
      mediaTypeFilter: onlyQueuedDeletion ? undefined : get().mediaTypeFilter,
      selectedIds: [],
    });
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
      items: state.items
        .map((item) =>
          item.id === id
            ? {
                ...item,
                isPinned: nextPinned,
                queuedDeleteAt: nextPinned ? null : item.queuedDeleteAt,
              }
            : item
        )
        .filter((item) => !(state.onlyQueuedDeletion && item.id === id && nextPinned)),
      selectedCapture:
        state.selectedCapture?.id === id
          ? {
              ...state.selectedCapture,
              isPinned: nextPinned,
              queuedDeleteAt: nextPinned ? null : state.selectedCapture.queuedDeleteAt,
            }
          : state.selectedCapture,
    }));
    await get().fetchCaptures();
  },

  restoreQueued: async (ids, alsoPin = false) => {
    if (ids.length === 0) return 0;
    const restored = await restoreQueuedCaptures(ids, alsoPin);
    set((state) => ({
      selectedIds: state.selectedIds.filter((id) => !ids.includes(id)),
      selectedCapture:
        state.selectedCapture && ids.includes(state.selectedCapture.id)
          ? {
              ...state.selectedCapture,
              queuedDeleteAt: null,
              isPinned: alsoPin ? true : state.selectedCapture.isPinned,
            }
          : state.selectedCapture,
    }));
    await get().fetchCaptures();
    return restored;
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
