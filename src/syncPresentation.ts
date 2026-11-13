import { SyncStoreState } from "./store";
import { t } from "./i18n";

export function syncPresentation(state: SyncStoreState) {
  const { config, isOnline, credentialAlert, syncRunning, syncAction, globalState } = state;
  if (!config?.bucket_name.trim() || !config.local_path.trim())
    return { text: t("Not configured"), badgeClass: "badge-offline", led: "led-offline" };
  if (syncAction === "starting")
    return { text: t("Connecting…"), badgeClass: "badge-syncing", led: "led-syncing" };
  if (syncAction === "stopping")
    return { text: t("Stopping…"), badgeClass: "badge-offline", led: "led-offline" };
  if (credentialAlert)
    return { text: t("Check your credentials"), badgeClass: "badge-error", led: "led-error" };
  if (syncRunning === null)
    return { text: t("Status unavailable"), badgeClass: "badge-offline", led: "led-offline" };
  if (!syncRunning)
    return { text: t("Stopped"), badgeClass: "badge-offline", led: "led-offline" };
  if (!isOnline)
    return { text: t("Offline"), badgeClass: "badge-offline", led: "led-offline" };
  if (globalState === "Error")
    return { text: t("Sync error"), badgeClass: "badge-error", led: "led-error" };
  if (globalState === "Syncing")
    return { text: t("Syncing"), badgeClass: "badge-syncing", led: "led-syncing" };
  return { text: t("Up to date"), badgeClass: "badge-uptodate", led: "led-online" };
}
