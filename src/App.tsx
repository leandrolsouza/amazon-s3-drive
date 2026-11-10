import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { NotificationHandler } from "./components/NotificationHandler";
import { useSyncEvents } from "./hooks/useSyncEvents";
import { ConfigurationPage } from "./pages/ConfigurationPage";
import { LogsView } from "./pages/LogsView";
import { StatusView, formatBytes } from "./pages/StatusView";
import { useSyncStore } from "./store";
import "./styles.css";

export const App: React.FC = () => {
  useSyncEvents();

  const [activeTab, setActiveTab] = useState<"config" | "status" | "logs">(
    "config"
  );
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    try {
      const saved = localStorage.getItem("s3drive-theme");
      if (saved === "dark" || saved === "light") return saved;
    } catch {
      // Fallback
    }
    return "dark";
  });

  const config = useSyncStore((s) => s.config);
  const globalState = useSyncStore((s) => s.globalState);
  const fileStatuses = useSyncStore((s) => s.fileStatuses);
  const queueStats = useSyncStore((s) => s.queueStats);
  const isOnline = useSyncStore((s) => s.isOnline);
  const conflicts = useSyncStore((s) => s.conflicts);
  const errors = useSyncStore((s) => s.errors);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem("s3drive-theme", theme);
    } catch {
      // Ignore
    }
  }, [theme]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen<string>("tray:navigate", (ev) => {
      if (ev.payload === "status" || ev.payload === "config") {
        setActiveTab(ev.payload);
      }
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    return () => {
      unlisten?.();
    };
  }, []);

  const handleOpenLocalFolder = async () => {
    if (!config?.local_path) return;
    try {
      await invoke("open_local_folder", { path: config.local_path });
    } catch {
      // Ignore
    }
  };

  const totalBytes = fileStatuses.reduce(
    (acc, f) => acc + (f.size_bytes || 0),
    0
  );
  const syncedCount = fileStatuses.filter(
    (f) => f.state.type === "Synced"
  ).length;
  const alertCount =
    conflicts.length + errors.filter((e) => e.retry_count >= 3).length;

  return (
    <div className="desktop-frame">
      <div className="desktop-body">
        {/* Sidebar Utilitária Compacta */}
        <aside className="sidebar">
          <div className="sidebar-brand">
            <div className="s3-mark" aria-hidden="true">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
                <path
                  d="M12 2L3 7v10l9 5 9-5V7l-9-5z"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinejoin="round"
                />
                <path
                  d="M3 7l9 5 9-5M12 22V12"
                  stroke="currentColor"
                  strokeWidth="1.75"
                  strokeLinejoin="round"
                />
              </svg>
            </div>
            <div className="sidebar-brand-title">
              <h1>Amazon S3 Drive</h1>
            </div>
          </div>

          <div className="sidebar-group-label">WORKSPACE</div>
          <nav className="sidebar-nav" aria-label="Navegação Principal">
            <button
              type="button"
              className={`sidebar-link ${
                activeTab === "status" ? "nav-active" : ""
              }`}
              onClick={() => setActiveTab("status")}
            >
              <span className="sidebar-link-main">
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                >
                  <path d="M3 6h18M3 12h18M3 18h18" />
                </svg>
                <span>Status de Sincronização</span>
              </span>
              <span className="sidebar-counter">{fileStatuses.length}</span>
            </button>

            <button
              type="button"
              className={`sidebar-link ${
                activeTab === "config" ? "nav-active" : ""
              }`}
              onClick={() => setActiveTab("config")}
            >
              <span className="sidebar-link-main">
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                >
                  <path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" />
                </svg>
                <span>Configurações</span>
              </span>
              {alertCount > 0 && (
                <span className="sidebar-counter counter-alert">
                  {alertCount}
                </span>
              )}
            </button>

            <button
              type="button"
              className={`sidebar-link ${
                activeTab === "logs" ? "nav-active" : ""
              }`}
              onClick={() => setActiveTab("logs")}
            >
              <span className="sidebar-link-main">
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                >
                  <polyline points="4 17 10 11 4 5" />
                  <line x1="12" y1="19" x2="20" y2="19" />
                </svg>
                <span>Logs & Diagnóstico</span>
              </span>
              {errors.length > 0 && (
                <span className="sidebar-counter counter-alert">
                  {errors.length}
                </span>
              )}
            </button>
          </nav>

          <div className="sidebar-group-label">CONEXÃO ATIVA</div>
          <div className="sidebar-mount-info">
            <div className="mount-row">
              <span className="mount-key">BUCKET</span>
              <span className="mount-val mono">
                {config?.bucket_name || "não definido"}
              </span>
            </div>
            <div className="mount-row">
              <span className="mount-key">REGIÃO</span>
              <span className="mount-val mono">
                {config?.aws_region || "—"}
              </span>
            </div>
            <div className="mount-row">
              <span className="mount-key">PREFIXO</span>
              <span className="mount-val mono">
                {config?.s3_prefix || "/"}
              </span>
            </div>

            {config?.local_path && (
              <button
                type="button"
                className="sidebar-open-dir-btn"
                onClick={handleOpenLocalFolder}
                title={config.local_path}
              >
                <svg
                  width="13"
                  height="13"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.8"
                >
                  <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                </svg>
                <span>Abrir no Explorer</span>
              </button>
            )}
          </div>

          <div className="sidebar-bottom">
            <button
              type="button"
              className="theme-switch"
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            >
              <span className="theme-switch-label">Aparência</span>
              <span className="theme-switch-value mono">
                {theme === "dark" ? "DARK" : "LIGHT"}
              </span>
            </button>
          </div>
        </aside>

        {/* Painel Central */}
        <div className="workspace-pane">
          <NotificationHandler />
          <main className="workspace-content">
            {activeTab === "config" ? (
              <ConfigurationPage onSyncStarted={() => setActiveTab("status")} />
            ) : activeTab === "logs" ? (
              <LogsView />
            ) : (
              <StatusView />
            )}
          </main>
        </div>
      </div>

      {/* Barra de Status Inferior (Estilo VS Code / Panic Transmit) */}
      <footer className="statusbar">
        <div className="statusbar-left">
          <span className="statusbar-item">
            <span
              className={`led-dot ${
                !isOnline
                  ? "led-offline"
                  : globalState === "Error"
                  ? "led-error"
                  : globalState === "Syncing"
                  ? "led-syncing"
                  : "led-online"
              }`}
            />
            <strong>
              {!isOnline
                ? "OFFLINE"
                : globalState === "Syncing"
                ? "SYNCING"
                : globalState === "Error"
                ? "ERROR"
                : "READY"}
            </strong>
          </span>

          <span className="statusbar-sep" />

          <span className="statusbar-item mono">
            {config?.bucket_name ? `s3://${config.bucket_name}` : "sem bucket"}
          </span>

          {config?.local_path && (
            <>
              <span className="statusbar-sep" />
              <span className="statusbar-item mono truncate">
                {config.local_path}
              </span>
            </>
          )}
        </div>

        <div className="statusbar-right">
          <span className="statusbar-item mono">
            FILA: {queueStats.pending_count}/10000
          </span>
          <span className="statusbar-sep" />
          <span className="statusbar-item mono">
            ARQUIVOS: {syncedCount}/{fileStatuses.length}
          </span>
          <span className="statusbar-sep" />
          <span className="statusbar-item mono">{formatBytes(totalBytes)}</span>
        </div>
      </footer>
    </div>
  );
};

export default App;
