import React, { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { NotificationHandler } from "./components/NotificationHandler";
import { AppUpdateNotice } from "./components/AppUpdateNotice";
import { DeletionReviewPanel } from "./components/DeletionReviewPanel";
import { useSyncEvents } from "./hooks/useSyncEvents";
import { ConfigurationPage } from "./pages/ConfigurationPage";
import { BucketsPage } from "./pages/BucketsPage";
import { Icon } from "./components/Icon";
import { StorageView } from "./pages/StorageView";
import { SiteView } from "./pages/SiteView";
import { LogsView } from "./pages/LogsView";
import { StatusView, formatBytes } from "./pages/StatusView";
import { useSyncStore } from "./store";
import "./styles.css";
import { syncPresentation } from "./syncPresentation";
import { AppConfig } from "./types";
import { runSyncCommand } from "./syncCommands";
import { initializeLocale, locale, t } from "./i18n";

export const App: React.FC = () => {
  useSyncEvents();

  const [activeTab, setActiveTab] = useState<"config" | "status" | "logs" | "storage" | "sites" | "buckets">(
    useSyncStore.getState().config?.bucket_name.trim() && useSyncStore.getState().config?.local_path.trim() ? "status" : "config"
  );
  const [sitesVisited, setSitesVisited] = useState(false);
  const [configLoaded, setConfigLoaded] = useState(() => useSyncStore.getState().config !== null);
  const [configLoadError, setConfigLoadError] = useState(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const startupAttempted = useRef(false);
  const [startupError, setStartupError] = useState<string | null>(null);
  const [startupRetry, setStartupRetry] = useState(0);
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
  const presentation = syncPresentation(useSyncStore());
  const [folderError, setFolderError] = useState(false);
  const fileStatuses = useSyncStore((s) => s.fileStatuses);
  const transferProgress = useSyncStore((s) => s.transferProgress);
  const queueStats = useSyncStore((s) => s.queueStats);
  const conflicts = useSyncStore((s) => s.conflicts);
  const errors = useSyncStore((s) => s.errors);

  useEffect(() => {
    if (configLoaded) return;
    let disposed = false;
    setConfigLoadError(false);
    invoke<AppConfig | null>("get_config")
      .then((loaded) => {
        if (disposed) return;
        if (loaded) useSyncStore.getState().setConfig(loaded);
        setActiveTab(loaded?.bucket_name.trim() && loaded.local_path.trim() ? "status" : "config");
        setConfigLoaded(true);
      })
      .catch(() => { if (!disposed) setConfigLoadError(true); });
    return () => { disposed = true; };
  }, [configLoaded, loadAttempt]);

  useEffect(() => {
    if (!configLoaded || startupAttempted.current) return;
    startupAttempted.current = true;
    if (!config?.bucket_name.trim() || !config.local_path.trim()) return;
    const start = async () => {
      setStartupError(null);
      try {
        const running = await invoke<boolean>("get_sync_running");
        if (typeof running !== "boolean") throw new Error(t("Invalid response from the sync engine."));
        useSyncStore.getState().setSyncRunning(running);
        if (!running && !useSyncStore.getState().syncAction) await runSyncCommand("start_sync");
      } catch (error) {
        setStartupError(String(error));
      }
    };
    void start();
  }, [configLoaded, config, startupRetry]);

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

  const [selectedLanguage, setSelectedLanguage] = useState(locale);
  const [languageError, setLanguageError] = useState(false);
  const [changingLanguage, setChangingLanguage] = useState(false);
  const changeLanguage = async (language: string) => {
    setChangingLanguage(true);
    setLanguageError(false);
    try {
      await invoke("set_language", { language });
      initializeLocale(language);
      setSelectedLanguage(locale);
      setChangingLanguage(false);
    } catch {
      setLanguageError(true);
      setChangingLanguage(false);
    }
  };

  const handleOpenLocalFolder = async () => {
    if (!config?.local_path) return;
    setFolderError(false);
    try {
      await invoke("open_local_folder", { path: config.local_path });
    } catch {
      setFolderError(true);
    }
  };

  useEffect(() => {
    let disposed = false;
    let loading = false;
    const refresh = async () => {
      const before = useSyncStore.getState();
      if (loading || before.syncAction) return;
      const canApply = () => {
        const current = useSyncStore.getState();
        return !disposed && !current.syncAction && current.syncRunning === before.syncRunning && current.config === before.config;
      };
      loading = true;
      try {
        const running = await invoke<boolean>("get_sync_running");
        if (canApply())
          useSyncStore.getState().setSyncRunning(typeof running === "boolean" ? running : null);
      } catch {
        if (canApply())
          useSyncStore.getState().setSyncRunning(null);
      } finally {
        loading = false;
      }
    };
    void refresh();
    const timer = setInterval(refresh, 2500);
    return () => { disposed = true; clearInterval(timer); };
  }, []);

  const totalBytes = fileStatuses.reduce(
    (acc, f) => acc + (f.size_bytes || 0),
    0
  );
  const syncedCount = fileStatuses.filter(
    (f) => f.state.type === "Synced"
  ).length;
  const activeFiles = fileStatuses.filter((file) => file.state.type === "Syncing");
  const activeFile = activeFiles[0];
  const progress = activeFile ? transferProgress[activeFile.path] : undefined;
  const transferLabel = progress?.operation === "Upload" ? t("Uploading")
    : progress?.operation === "Download" ? t("Downloading") : t("Syncing");
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
          <nav className="sidebar-nav" aria-label={t("Main navigation")}>
            <button
              type="button"
              className={activeTab === "buckets" ? "sidebar-link nav-active" : "sidebar-link"}
              aria-current={activeTab === "buckets" ? "page" : undefined}
              onClick={() => setActiveTab("buckets")}
            >
              <span className="sidebar-link-main">Buckets</span>
            </button>
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
                <span>{t("Sync Status")}</span>
              </span>
              <span className="sidebar-counter">{fileStatuses.length}</span>
            </button>

            <button
              type="button"
              className={activeTab === "storage" ? "sidebar-link nav-active" : "sidebar-link"}
              aria-current={activeTab === "storage" ? "page" : undefined}
              onClick={() => setActiveTab("storage")}
            >
              <span className="sidebar-link-main">
                <Icon name="chart" />
                <span>{t("Storage")}</span>
              </span>
            </button>

            <button
              type="button"
              className={activeTab === "sites" ? "sidebar-link nav-active" : "sidebar-link"}
              aria-current={activeTab === "sites" ? "page" : undefined}
              onClick={() => { setSitesVisited(true); setActiveTab("sites"); }}
            >
              <span className="sidebar-link-main">
                <Icon name="link" />
                <span>Sites</span>
              </span>
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
                <span>{t("Settings")}</span>
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
                <span>{t("Logs & Diagnostics")}</span>
              </span>
              {errors.length > 0 && (
                <span className="sidebar-counter counter-alert">
                  {errors.length}
                </span>
              )}
            </button>
          </nav>

          <div className="sidebar-group-label">{t("PRIMARY BUCKET")}</div>
          <div className="sidebar-mount-info">
            <div className="mount-row">
              <span className="mount-key">BUCKET</span>
              <span className="mount-val mono">
                {config?.bucket_name || t("not set")}
              </span>
            </div>
            <div className="mount-row">
              <span className="mount-key">{t("REGION")}</span>
              <span className="mount-val mono">
                {config?.aws_region || "—"}
              </span>
            </div>
            <div className="mount-row">
              <span className="mount-key">{t("PREFIX")}</span>
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
                <span>{t("Open Local Folder")}</span>
              </button>
            )}
          </div>

          <div className="sidebar-bottom">
            <label className="language-select">
              {t("Language")}
              <select value={selectedLanguage} disabled={changingLanguage} onChange={(event) => void changeLanguage(event.target.value)}>
                <option value="en-US" lang="en">English</option>
                <option value="pt-BR" lang="pt-BR">Português (Brasil)</option>
              </select>
            </label>
            {languageError && <p role="alert">{t("Could not save the language preference.")}</p>}
            <button
              type="button"
              className="theme-switch"
              onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
            >
              <span className="theme-switch-label">{t("Appearance")}</span>
              <span className="theme-switch-value mono">
                {theme === "dark" ? t("DARK") : t("LIGHT")}
              </span>
            </button>
          </div>
        </aside>

        {/* Painel Central */}
        <div className="workspace-pane">
          <AppUpdateNotice />
          <NotificationHandler onOpenConfig={() => setActiveTab("config")} />
          {activeTab !== "buckets" && <DeletionReviewPanel />}
          {startupError && (
            <div role="alert" className="banner banner-error">
              {t("Could not start sync:") + " "}{startupError}
              <button type="button" onClick={() => { startupAttempted.current = false; setStartupRetry((attempt) => attempt + 1); }}>{t("Try again")}</button>
              <button type="button" onClick={() => setActiveTab("config")}>{t("View settings")}</button>
            </div>
          )}
          {folderError && (
            <div role="alert" className="banner banner-error">
              {t("Could not open the local folder.")}
              <button type="button" onClick={handleOpenLocalFolder}>{t("Try again")}</button>
              <button type="button" onClick={() => setFolderError(false)}>{t("Dismiss")}</button>
            </div>
          )}
          <main className="workspace-content">
            {!configLoaded ? (
              <div className="banner" role={configLoadError ? "alert" : "status"}>
                {configLoadError ? t("Could not load settings.") : t("Loading settings…")}
                {configLoadError && <button type="button" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>{t("Try again")}</button>}
              </div>
            ) : <>
              <div style={{ display: activeTab === "config" ? "flex" : "none", flex: 1, minHeight: 0, flexDirection: "column" }}>
                <ConfigurationPage onSyncStarted={() => setActiveTab("status")} />
              </div>
              {sitesVisited && (
                <div style={{ display: activeTab === "sites" ? "flex" : "none", flex: 1, minHeight: 0, flexDirection: "column" }}>
                  <SiteView active={activeTab === "sites"} />
                </div>
              )}
              {activeTab === "buckets" ? (
              <BucketsPage config={config} primaryStatus={presentation.text}
                onOpenPrimary={() => setActiveTab("status")} onOpenConfig={() => setActiveTab("config")} />
            ) : activeTab === "storage" ? (
              <StorageView />
            ) : activeTab === "logs" ? (
              <LogsView />
            ) : activeTab === "status" ? (
              <StatusView />
            ) : null}
            </>}
          </main>
        </div>
      </div>

      {/* Barra de Status Inferior (Estilo VS Code / Panic Transmit) */}
      <footer className="statusbar">
        <div className="statusbar-left">
          <span className="statusbar-item">
            <span
              className={`led-dot ${presentation.led}`}
            />
            <strong>
              {presentation.text}
            </strong>
          </span>

          <span className="statusbar-sep" />

          <span className="statusbar-item mono">
            {config?.bucket_name ? `s3://${config.bucket_name}` : t("no bucket")}
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

        {activeFile && (
          <span className="statusbar-item statusbar-transfer" title={activeFile.path}>
            <span className="led-dot led-syncing" aria-hidden="true" />
            <span>{transferLabel}</span>
            <span className="statusbar-transfer-name">
              {activeFile.path.slice(activeFile.path.lastIndexOf("/") + 1) || activeFile.path}
            </span>
            {progress?.operation === "Upload" && <span className="mono">{progress.percent}%</span>}
            {activeFiles.length > 1 && <span>+{activeFiles.length - 1}</span>}
          </span>
        )}

        <div className="statusbar-right">
          <span className="statusbar-item mono">
            {t("QUEUE:") + " "}{queueStats.pending_count}/10000
          </span>
          <span className="statusbar-sep" />
          <span className="statusbar-item mono">
            {t("FILES:") + " "}{syncedCount}/{fileStatuses.length}
          </span>
          <span className="statusbar-sep" />
          <span className="statusbar-item mono">{formatBytes(totalBytes)}</span>
        </div>
      </footer>
    </div>
  );
};

export default App;
