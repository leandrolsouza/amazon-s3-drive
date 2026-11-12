import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "./store";

export async function runSyncCommand(command: "start_sync" | "stop_sync") {
  const store = useSyncStore.getState();
  if (store.syncAction) throw new Error("Aguarde a operação de sincronização em andamento.");
  store.setSyncAction(command === "start_sync" ? "starting" : "stopping");
  try {
    await invoke(command);
    store.setSyncRunning(command === "start_sync");
    if (command === "start_sync") store.setCredentialAlert(null);
  } catch (error) {
    try {
      const running = await invoke<boolean>("get_sync_running");
      store.setSyncRunning(typeof running === "boolean" ? running : null);
    } catch {
      store.setSyncRunning(null);
    }
    throw error;
  } finally {
    store.setSyncAction(null);
  }
}
