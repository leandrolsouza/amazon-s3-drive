import { useEffect } from "react";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { useSyncStore } from "../store";
import {
  ConflictEvent,
  FileStatusEvent,
  InitialCompleteEvent,
  SyncErrorEvent,
  TransferProgressEvent,
} from "../types";

export function useSyncEvents(): void {
  const updateFileStatus = useSyncStore((s) => s.updateFileStatus);
  const setTransferProgress = useSyncStore((s) => s.setTransferProgress);
  const addError = useSyncStore((s) => s.addError);
  const addConflict = useSyncStore((s) => s.addConflict);
  const setOnline = useSyncStore((s) => s.setOnline);
  const setQueueStats = useSyncStore((s) => s.setQueueStats);
  const setInitialCompleteCount = useSyncStore((s) => s.setInitialCompleteCount);
  const setCredentialAlert = useSyncStore((s) => s.setCredentialAlert);

  useEffect(() => {
    let isMounted = true;
    const unlisteners: UnlistenFn[] = [];

    async function setupListeners() {
      try {
        const u1 = await listen<FileStatusEvent>(
          "sync:file-status-changed",
          (ev) => {
            if (isMounted) {
              updateFileStatus(ev.payload.path, ev.payload.state);
            }
          }
        );
        unlisteners.push(u1);

        const u2 = await listen<SyncErrorEvent>("sync:error", (ev) => {
          if (isMounted) {
            updateFileStatus(ev.payload.file, {
              type: "Error",
              message: ev.payload.error,
            });
            addError(ev.payload);
          }
        });
        unlisteners.push(u2);

        const u3 = await listen<InitialCompleteEvent>(
          "sync:initial-complete",
          (ev) => {
            if (isMounted) {
              setInitialCompleteCount(ev.payload.files_synced);
            }
          }
        );
        unlisteners.push(u3);

        const u4 = await listen<ConflictEvent>("conflict:detected", (ev) => {
          if (isMounted) {
            addConflict(ev.payload);
          }
        });
        unlisteners.push(u4);

        const u5 = await listen<void>("network:offline", () => {
          if (isMounted) {
            setOnline(false);
          }
        });
        unlisteners.push(u5);

        const u6 = await listen<void>("network:online", () => {
          if (isMounted) {
            setOnline(true);
          }
        });
        unlisteners.push(u6);

        const u7 = await listen<void>("queue:full", () => {
          if (isMounted) {
            setQueueStats({ pending_count: 10000, is_full: true });
          }
        });
        unlisteners.push(u7);

        const u8 = await listen<void>("credentials:expired", () => {
          if (isMounted) {
            setCredentialAlert(
              "Suas credenciais AWS expiraram ou foram revogadas. A sincronização foi pausada."
            );
          }
        });
        unlisteners.push(u8);

        const u9 = await listen<void>("credentials:invalid", () => {
          if (isMounted) {
            setCredentialAlert(
              "Credenciais AWS inválidas. Verifique suas chaves de acesso."
            );
          }
        });
        unlisteners.push(u9);

        const u10 = await listen<TransferProgressEvent>(
          "sync:transfer-progress",
          (ev) => {
            if (isMounted) {
              setTransferProgress(ev.payload);
            }
          }
        );
        unlisteners.push(u10);
      } catch {
        // Ignorado em ambiente de teste sem runtime Tauri ativo
      }
    }

    setupListeners();

    return () => {
      isMounted = false;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, [
    updateFileStatus,
    setTransferProgress,
    addError,
    addConflict,
    setOnline,
    setQueueStats,
    setInitialCompleteCount,
    setCredentialAlert,
  ]);
}
