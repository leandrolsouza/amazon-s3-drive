import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "./store";
import { t } from "./i18n";

export async function runSyncCommand(command: "start_sync" | "stop_sync") {
  const store = useSyncStore.getState();
  if (store.syncAction) throw new Error(t("Wait for the current sync operation to finish."));
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
