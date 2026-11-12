import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "../store";
import type { StorageAnalytics } from "../types";
import { formatBytes } from "./StatusView";
import { StorageCostForecast } from "../components/StorageCostForecast";
import { S3MonthlySpend } from "../components/S3MonthlySpend";

const percentage = new Intl.NumberFormat("pt-BR", {
  style: "percent",
  maximumFractionDigits: 1,
});

export function StorageView() {
  const config = useSyncStore((state) => state.config);
  const files = useSyncStore((state) => state.fileStatuses);
  const [storageAnalytics, setStorageAnalytics] = useState<StorageAnalytics | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let disposed = false;
    setFailed(false);
    invoke<StorageAnalytics>("get_storage_analytics")
      .then((stats) => {
        if (!stats || !Array.isArray(stats.top_extensions)) throw new Error("Métricas indisponíveis");
        if (!disposed) setStorageAnalytics(stats);
      })
      .catch(() => { if (!disposed) setFailed(true); });
    return () => { disposed = true; };
  }, [config, files, attempt]);

  const totalObjects = storageAnalytics?.total_files ?? 0;
  const syncStates = storageAnalytics ? [
    { label: "Sincronizados", count: storageAnalytics.synced_files, tone: "synced" },
    { label: "Pendentes ou em andamento", count: storageAnalytics.pending_or_syncing_files, tone: "pending" },
    { label: "Com erro", count: storageAnalytics.error_files, tone: "error" },
  ] : [];

  return (
    <div className="pane-container">
      <header className="command-bar storage-header">
        <h2 className="pane-title">Armazenamento</h2>
      </header>
      <div className="storage-content">
        {failed && (
          <div className="banner banner-error" role="alert">
            Não foi possível carregar as métricas de armazenamento.
            <button type="button" onClick={() => setAttempt((value) => value + 1)}>Tentar novamente</button>
          </div>
        )}
        {!failed && !storageAnalytics && <p role="status">Carregando métricas…</p>}
        {storageAnalytics && (
          <section className="analytics-strip" aria-label="Métricas de Armazenamento S3">
            <dl className="analytics-summary">
              <div className="analytics-metric analytics-cost">
                <dt>Armazenamento de referência</dt>
                <dd>
                  {new Intl.NumberFormat("pt-BR", {
                    style: "currency", currency: "USD", maximumFractionDigits: 4,
                  }).format(storageAnalytics.estimated_monthly_usd)}
                  <small> / mês</small>
                </dd>
                <small>Armazenamento · {storageAnalytics.storage_class}</small>
              </div>
              <div className="analytics-metric">
                <dt>Volume total</dt>
                <dd>{formatBytes(storageAnalytics.total_bytes)}</dd>
                <small>
                  {totalObjects > 0
                    ? `${formatBytes(storageAnalytics.total_bytes / totalObjects)} em média por objeto`
                    : "Sem objetos para calcular a média"}
                </small>
              </div>
              <div className="analytics-metric">
                <dt>Objetos</dt>
                <dd>{storageAnalytics.total_files.toLocaleString("pt-BR")}</dd>
              </div>
            </dl>
            <StorageCostForecast
              key={JSON.stringify([config?.aws_region, config?.storage_class, config?.endpoint_url])}
              region={config?.aws_region ?? ""}
              customEndpoint={Boolean(config?.endpoint_url?.trim())}
              storageUsd={storageAnalytics.estimated_monthly_usd}
              storageBytes={storageAnalytics.total_bytes}
            />
            {config?.bucket_name && !config.endpoint_url?.trim() && (
              <S3MonthlySpend key={JSON.stringify([config.bucket_name, config.aws_region, config.use_iam_role])} />
            )}
            <div className="analytics-breakdown">
              <h3>Estado da sincronização</h3>
              {totalObjects > 0 && (
                <div className="analytics-track analytics-sync-track" aria-hidden="true">
                  {syncStates.map(({ tone, count }) => (
                    <span key={tone} className={`analytics-sync-${tone}`} style={{ flexGrow: count }} />
                  ))}
                </div>
              )}
              <dl className="analytics-sync-legend">
                {syncStates.map(({ label, count, tone }) => (
                  <div key={tone}>
                    <dt><span className={`analytics-sync-${tone}`} aria-hidden="true" />{label}</dt>
                    <dd>{count.toLocaleString("pt-BR")}</dd>
                  </div>
                ))}
              </dl>
            </div>
            {storageAnalytics.top_extensions.length > 0 && (
              <div className="analytics-breakdown">
                <h3>Volume por extensão</h3>
                <ul>
                  {storageAnalytics.top_extensions.map((ext) => {
                    const share = storageAnalytics.total_bytes > 0
                      ? ext.bytes / storageAnalytics.total_bytes
                      : 0;
                    return (
                      <li key={ext.extension}>
                        <span className="analytics-extension" title={ext.extension}>{ext.extension}</span>
                        <div className="analytics-track" aria-hidden="true">
                          <div style={{ width: share * 100 + "%" }} />
                        </div>
                        <span className="analytics-extension-stats">
                          <strong>{formatBytes(ext.bytes)}</strong>
                          <span>{percentage.format(share)} do total · {ext.count.toLocaleString("pt-BR")} {ext.count === 1 ? "objeto" : "objetos"}</span>
                        </span>
                      </li>
                    );
                  })}
                </ul>
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}
