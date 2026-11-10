import React, { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "../store";
import { ConflictEvent } from "../types";

export const NotificationHandler: React.FC = () => {
  const config = useSyncStore((s) => s.config);
  const conflicts = useSyncStore((s) => s.conflicts);
  const errors = useSyncStore((s) => s.errors);
  const initialCompleteCount = useSyncStore((s) => s.initialCompleteCount);
  const credentialAlert = useSyncStore((s) => s.credentialAlert);

  const removeConflict = useSyncStore((s) => s.removeConflict);
  const dismissError = useSyncStore((s) => s.dismissError);
  const setInitialCompleteCount = useSyncStore((s) => s.setInitialCompleteCount);
  const setCredentialAlert = useSyncStore((s) => s.setCredentialAlert);

  const syncCompleteEnabled =
    config?.notifications.sync_complete_enabled ?? true;
  const conflictEnabled = config?.notifications.conflict_enabled ?? true;

  // Requisito 10.1: notificação de conclusão da sincronização inicial visível por no mínimo 5 segundos
  useEffect(() => {
    if (initialCompleteCount === null || !syncCompleteEnabled) {
      return;
    }
    const timer = setTimeout(() => {
      setInitialCompleteCount(null);
    }, 5500);
    return () => clearTimeout(timer);
  }, [initialCompleteCount, syncCompleteEnabled, setInitialCompleteCount]);

  // Requisito 10.3: resolução de conflito via UI a partir da notificação
  const handleResolveConflict = async (
    conflict: ConflictEvent,
    strategy: "local" | "remote" | "both"
  ) => {
    try {
      if (strategy === "local" || strategy === "remote") {
        await invoke("resolve_conflict", {
          strategy,
          localPath: conflict.local_path,
          conflictPath: conflict.conflict_copy_path,
        });
      }
    } finally {
      removeConflict(conflict.local_path);
    }
  };

  return (
    <div className="notification-container" aria-live="polite">
      {/* Alerta de credenciais expiradas / inválidas (Requisito 4.4) */}
      {credentialAlert && (
        <div
          role="alert"
          data-testid="notification-credential"
          className="notification-card notification-error"
        >
          <p>{credentialAlert}</p>
          <button type="button" onClick={() => setCredentialAlert(null)}>
            Dispensar
          </button>
        </div>
      )}

      {/* Conclusão da primeira sincronização completa (Requisito 10.1) */}
      {syncCompleteEnabled && initialCompleteCount !== null && (
        <div
          role="status"
          data-testid="notification-sync-complete"
          className="notification-card notification-success"
        >
          <p>
            Sincronização inicial concluída: {initialCompleteCount} arquivo(s)
            sincronizado(s).
          </p>
          <button
            type="button"
            onClick={() => setInitialCompleteCount(null)}
          >
            Fechar
          </button>
        </div>
      )}

      {/* Erros de sincronização após 3 tentativas — sempre exibidos (Requisito 10.2, 10.4) */}
      {errors
        .filter((err) => err.retry_count >= 3)
        .map((err) => (
          <div
            key={err.file}
            role="alert"
            data-testid={`notification-error-${err.file}`}
            className="notification-card notification-error"
          >
            <strong>Erro de sincronização: {err.file}</strong>
            <p>{err.error}</p>
            <button type="button" onClick={() => dismissError(err.file)}>
              Dispensar
            </button>
          </div>
        ))}

      {/* Conflitos detectados — persistente até seleção de opção (Requisito 10.3) */}
      {conflictEnabled &&
        conflicts.map((conflict) => (
          <div
            key={conflict.local_path}
            role="alert"
            data-testid={`notification-conflict-${conflict.file_name}`}
            className="notification-card notification-warning"
          >
            <strong>Conflito detectado: {conflict.file_name}</strong>
            <p>
              Local: {conflict.local_modified} ({conflict.local_size_bytes}{" "}
              bytes) | Remoto: {conflict.remote_modified} (
              {conflict.remote_size_bytes} bytes)
            </p>
            <div className="conflict-actions">
              <button
                type="button"
                onClick={() => handleResolveConflict(conflict, "local")}
              >
                Manter local
              </button>
              <button
                type="button"
                onClick={() => handleResolveConflict(conflict, "remote")}
              >
                Manter remota
              </button>
              <button
                type="button"
                onClick={() => handleResolveConflict(conflict, "both")}
              >
                Manter ambas
              </button>
            </div>
          </div>
        ))}
    </div>
  );
};
