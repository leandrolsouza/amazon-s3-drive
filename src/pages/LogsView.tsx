import React, { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

export const LogsView: React.FC = () => {
  const [loadError, setLoadError] = useState(false);
  const [clearError, setClearError] = useState(false);
  const [lines, setLines] = useState<string[]>([]);
  const [filterText, setFilterText] = useState("");
  const [levelFilter, setLevelFilter] = useState<"ALL" | "ERROR" | "INFO">(
    "ALL"
  );
  const [autoRefresh, setAutoRefresh] = useState(true);

  const loadLogs = useCallback(() => {
    invoke<string[]>("get_sync_logs", { maxLines: 500 })
      .then((res) => {
        if (Array.isArray(res)) {
          setLines(res);
          setLoadError(false);
        }
      })
      .catch(() => setLoadError(true));
  }, []);

  useEffect(() => {
    loadLogs();
    if (!autoRefresh) return;
    const id = setInterval(loadLogs, 2500);
    return () => clearInterval(id);
  }, [loadLogs, autoRefresh]);

  const handleClearLogs = async () => {
    setClearError(false);
    try {
      await invoke("clear_sync_logs");
      setLines([]);
    } catch {
      setClearError(true);
    }
  };

  const filteredLines = useMemo(() => {
    return lines.filter((line) => {
      if (levelFilter === "ERROR" && !line.includes("[ERROR]")) return false;
      if (levelFilter === "INFO" && !line.includes("[INFO]")) return false;
      if (
        filterText.trim() &&
        !line.toLowerCase().includes(filterText.toLowerCase())
      ) {
        return false;
      }
      return true;
    });
  }, [lines, filterText, levelFilter]);

  return (
    <div className="pane-container">
      <header className="command-bar">
        <div className="command-bar-left">
          <h2 className="pane-title">{t("Logs & Diagnostics (sync.log)")}</h2>
          <span className="state-tag mono badge-offline">
            {filteredLines.length} {t("LINES")}
          </span>
        </div>

        <div className="command-bar-right">
          <div className="search-input-wrap">
            <svg
              width="13"
              height="13"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="2"
            >
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
            <input
              type="text"
              placeholder={t("Filter log events...")}
              value={filterText}
              onChange={(e) => setFilterText(e.target.value)}
              aria-label={t("Filter logs")}
            />
          </div>

          <div className="segmented-filter">
            {(
              [
                { id: "ALL", label: t("All") },
                { id: "ERROR", label: t("Errors") },
                { id: "INFO", label: "Info" },
              ] as const
            ).map((item) => (
              <button
                key={item.id}
                type="button"
                className={`seg-btn ${
                  levelFilter === item.id ? "seg-active" : ""
                }`}
                onClick={() => setLevelFilter(item.id)}
              >
                {item.label}
              </button>
            ))}
          </div>

          <div className="toolbar-divider" />

          <button
            type="button"
            className="btn-compact"
            onClick={() => setAutoRefresh(!autoRefresh)}
          >
            {autoRefresh ? "Auto: ON" : "Auto: OFF"}
          </button>

          <button type="button" className="btn-compact" onClick={loadLogs}>
            {t("Refresh")}
          </button>

          <button
            type="button"
            className="btn-compact"
            onClick={handleClearLogs}
          >
            {t("Clear")}
          </button>
        </div>
      </header>

      {(loadError || clearError) && (
        <div role="alert" className="banner banner-error">
          {loadError ? t("Could not load logs. The displayed entries may be out of date.") : t("Could not clear logs.")}
          <button type="button" onClick={loadError ? loadLogs : handleClearLogs}>{t("Try again")}</button>
        </div>
      )}
      {filteredLines.length === 0 && !loadError ? (
        <div className="empty-table-view">
          <p className="empty-status">{t("No log entries found.")}</p>
          <span className="empty-subtext">
            {t("Sync events and failures will appear here automatically.")}
          </span>
        </div>
      ) : (
        <div className="log-console mono" data-testid="log-console">
          {filteredLines.map((line, idx) => {
            const isErr = line.includes("[ERROR]");
            return (
              <div
                key={`${idx}-${line.slice(0, 24)}`}
                className={`log-line ${isErr ? "log-line-error" : "log-line-info"}`}
              >
                <span className="log-ln">{idx + 1}</span>
                <span className="log-text">{line}</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
