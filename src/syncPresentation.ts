import { SyncStoreState } from "./store";

export function syncPresentation(state: SyncStoreState) {
  const { config, isOnline, credentialAlert, syncRunning, syncAction, globalState } = state;
  if (!config?.bucket_name.trim() || !config.local_path.trim())
    return { text: "Não configurado", badgeClass: "badge-offline", led: "led-offline" };
  if (syncAction === "starting")
    return { text: "Conectando…", badgeClass: "badge-syncing", led: "led-syncing" };
  if (syncAction === "stopping")
    return { text: "Parando…", badgeClass: "badge-offline", led: "led-offline" };
  if (credentialAlert)
    return { text: "Verifique as credenciais", badgeClass: "badge-error", led: "led-error" };
  if (syncRunning === null)
    return { text: "Estado indisponível", badgeClass: "badge-offline", led: "led-offline" };
  if (!syncRunning)
    return { text: "Parado", badgeClass: "badge-offline", led: "led-offline" };
  if (!isOnline)
    return { text: "Sem conexão", badgeClass: "badge-offline", led: "led-offline" };
  if (globalState === "Error")
    return { text: "Erro de sincronização", badgeClass: "badge-error", led: "led-error" };
  if (globalState === "Syncing")
    return { text: "Sincronizando", badgeClass: "badge-syncing", led: "led-syncing" };
  return { text: "Atualizado", badgeClass: "badge-uptodate", led: "led-online" };
}
