import { create } from "zustand";
import {
  AppConfig,
  ConflictEvent,
  FileStatus,
  GlobalSyncState,
  QueueStats,
  SyncErrorEvent,
  SyncState,
  TransferProgressEvent,
  computeFrontendGlobalState,
} from "./types";

export interface SyncStoreState {
  config: AppConfig | null;
  globalState: GlobalSyncState;
  fileStatuses: FileStatus[];
  transferProgress: Record<string, TransferProgressEvent>;
  queueStats: QueueStats;
  isOnline: boolean;
  conflicts: ConflictEvent[];
  errors: SyncErrorEvent[];
  initialCompleteCount: number | null;
  credentialAlert: string | null;
  syncRunning: boolean | null;
  syncAction: "starting" | "stopping" | null;
  setSyncRunning: (running: boolean | null) => void;
  setSyncAction: (action: "starting" | "stopping" | null) => void;

  setConfig: (config: AppConfig) => void;
  setGlobalState: (state: GlobalSyncState) => void;
  setFileStatuses: (statuses: FileStatus[]) => void;
  updateFileStatus: (path: string, state: SyncState) => void;
  setTransferProgress: (ev: TransferProgressEvent) => void;
  setQueueStats: (stats: QueueStats) => void;
  setOnline: (online: boolean) => void;
  addConflict: (conflict: ConflictEvent) => void;
  removeConflict: (localPath: string) => void;
  addError: (err: SyncErrorEvent) => void;
  dismissError: (file: string) => void;
  setInitialCompleteCount: (count: number | null) => void;
  setCredentialAlert: (alert: string | null) => void;
  resetStore: () => void;
}

const initialState = {
  config: null,
  globalState: "UpToDate" as GlobalSyncState,
  fileStatuses: [],
  transferProgress: {} as Record<string, TransferProgressEvent>,
  queueStats: { pending_count: 0, is_full: false },
  isOnline: true,
  conflicts: [],
  errors: [],
  initialCompleteCount: null,
  credentialAlert: null,
  syncRunning: null,
  syncAction: null,
};

export const useSyncStore = create<SyncStoreState>((set) => ({
  ...initialState,
  setSyncRunning: (syncRunning) => set({ syncRunning }),
  setSyncAction: (syncAction) => set({ syncAction }),

  setConfig: (config) => set({ config }),

  setGlobalState: (globalState) => set({ globalState }),

  setFileStatuses: (fileStatuses) =>
    set((prev) => ({
      fileStatuses,
      globalState: computeFrontendGlobalState(fileStatuses, prev.isOnline),
    })),

  updateFileStatus: (path, state) =>
    set((prev) => {
      const existingIdx = prev.fileStatuses.findIndex((f) => f.path === path);
      let nextStatuses: FileStatus[];
      if (existingIdx >= 0) {
        nextStatuses = prev.fileStatuses.map((f, idx) =>
          idx === existingIdx
            ? { ...f, state, last_modified: new Date().toISOString() }
            : f
        );
      } else {
        nextStatuses = [
          ...prev.fileStatuses,
          {
            path,
            state,
            last_modified: new Date().toISOString(),
            size_bytes: 0,
          },
        ];
      }
      let transferProgress = prev.transferProgress;
      if (state.type !== "Syncing" && Object.prototype.hasOwnProperty.call(transferProgress, path)) {
        transferProgress = { ...transferProgress };
        delete transferProgress[path];
      }
      return {
        fileStatuses: nextStatuses,
        transferProgress,
        globalState: computeFrontendGlobalState(nextStatuses, prev.isOnline),
      };
    }),

  setTransferProgress: (ev) =>
    set((prev) => ({
      transferProgress: {
        ...prev.transferProgress,
        [ev.path]: ev,
      },
    })),

  setQueueStats: (queueStats) => set({ queueStats }),

  setOnline: (isOnline) =>
    set((prev) => ({
      isOnline,
      globalState: computeFrontendGlobalState(prev.fileStatuses, isOnline),
    })),

  addConflict: (conflict) =>
    set((prev) => ({
      conflicts: [
        ...prev.conflicts.filter((c) => c.local_path !== conflict.local_path),
        conflict,
      ],
    })),

  removeConflict: (localPath) =>
    set((prev) => ({
      conflicts: prev.conflicts.filter((c) => c.local_path !== localPath),
    })),

  addError: (err) =>
    set((prev) => ({
      errors: [...prev.errors.filter((e) => e.file !== err.file), err],
    })),

  dismissError: (file) =>
    set((prev) => ({
      errors: prev.errors.filter((e) => e.file !== file),
    })),

  setInitialCompleteCount: (initialCompleteCount) =>
    set({ initialCompleteCount }),

  setCredentialAlert: (credentialAlert) => set({ credentialAlert }),

  resetStore: () => set({ ...initialState }),
}));
