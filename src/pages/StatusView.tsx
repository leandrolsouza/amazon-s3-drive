import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSyncStore } from "../store";
import {
  FileStatus,
  GlobalSyncState,
  ObjectDetailedMetadata,
  QueueStats,
  S3ObjectVersion,
  StorageAnalytics,
  SyncState,
} from "../types";

function renderStateMeta(state: SyncState): {
  srIcon: string;
  label: string;
  code: string;
  className: string;
} {
  switch (state.type) {
    case "Pending":
      return {
        srIcon: "⏳",
        label: "Pendente",
        code: "QUEUED",
        className: "state-pending",
      };
    case "Syncing":
      return {
        srIcon: "🔄",
        label: "Sincronizando",
        code: "SYNC",
        className: "state-syncing",
      };
    case "Synced":
      return {
        srIcon: "✅",
        label: "Sincronizado",
        code: "OK",
        className: "state-synced",
      };
    case "Error":
      return {
        srIcon: "❌",
        label: `Erro: ${state.message}`,
        code: "ERR",
        className: "state-error",
      };
  }
}

function renderStateVectorIcon(state: SyncState) {
  switch (state.type) {
    case "Pending":
      return (
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        >
          <circle cx="12" cy="12" r="9" />
          <polyline points="12 7 12 12 15 15" />
        </svg>
      );
    case "Syncing":
      return (
        <svg
          className="spin-svg"
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        >
          <path d="M21 12a9 9 0 1 1-6.219-8.56" />
        </svg>
      );
    case "Synced":
      return (
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.2"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
      );
    case "Error":
      return (
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
        >
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      );
  }
}

function renderGlobalBadge(globalState: GlobalSyncState): {
  text: string;
  badgeClass: string;
} {
  switch (globalState) {
    case "UpToDate":
      return { text: "UpToDate", badgeClass: "badge-uptodate" };
    case "Syncing":
      return { text: "Syncing", badgeClass: "badge-syncing" };
    case "Error":
      return { text: "Error", badgeClass: "badge-error" };
    case "Offline":
      return { text: "Offline", badgeClass: "badge-offline" };
  }
}

export function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1
  );
  const val = bytes / Math.pow(1024, i);
  return `${val.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function splitPath(fullPath: string): { dir: string; base: string } {
  const idx = fullPath.lastIndexOf("/");
  if (idx === -1) return { dir: "", base: fullPath };
  return {
    dir: fullPath.slice(0, idx + 1),
    base: fullPath.slice(idx + 1),
  };
}

export const StatusView: React.FC = () => {
  const fileStatuses = useSyncStore((s) => s.fileStatuses);
  const transferProgress = useSyncStore((s) => s.transferProgress);
  const globalState = useSyncStore((s) => s.globalState);
  const queueStats = useSyncStore((s) => s.queueStats);
  const setFileStatuses = useSyncStore((s) => s.setFileStatuses);
  const setQueueStats = useSyncStore((s) => s.setQueueStats);

  const [searchQuery, setSearchQuery] = useState("");
  const [stateFilter, setStateFilter] = useState<
    "ALL" | "Synced" | "Syncing" | "Pending" | "Error"
  >("ALL");
  const [viewMode, setViewMode] = useState<"flat" | "tree">("flat");
  const [currentFolder, setCurrentFolder] = useState<string>("");
  const [isDragOver, setIsDragOver] = useState(false);
  const [selectedFileKey, setSelectedFileKey] = useState<string | null>(null);
  const [selectedMetadata, setSelectedMetadata] =
    useState<ObjectDetailedMetadata | null>(null);
  const [objectVersions, setObjectVersions] = useState<S3ObjectVersion[]>([]);
  const [presignExpirySecs, setPresignExpirySecs] = useState<number>(3600);
  const [generatedPresignedUrl, setGeneratedPresignedUrl] = useState<
    string | null
  >(null);
  const [sortField, setSortField] = useState<
    "path" | "size" | "time" | "state"
  >("path");
  const [sortAsc, setSortAsc] = useState<boolean>(true);
  const [showNewFolderInput, setShowNewFolderInput] = useState<boolean>(false);
  const [newFolderName, setNewFolderName] = useState<string>("");
  const [selectedBatchKeys, setSelectedBatchKeys] = useState<string[]>([]);
  const [showAnalyticsBar, setShowAnalyticsBar] = useState<boolean>(false);
  const [storageAnalytics, setStorageAnalytics] =
    useState<StorageAnalytics | null>(null);
  const [renameTargetKey, setRenameTargetKey] = useState<string>("");
  const [actionFeedback, setActionFeedback] = useState<{
    type: "ok" | "error";
    text: string;
  } | null>(null);

  useEffect(() => {
    invoke<FileStatus[]>("get_sync_status")
      .then((statuses) => {
        if (Array.isArray(statuses) && statuses.length > 0) {
          setFileStatuses(statuses);
        }
      })
      .catch(() => {});

    invoke<QueueStats>("get_queue_stats")
      .then((stats) => {
        if (stats && typeof stats.pending_count === "number") {
          setQueueStats(stats);
        }
      })
      .catch(() => {});
  }, [setFileStatuses, setQueueStats]);

  useEffect(() => {
    let unlistenDrop: (() => void) | undefined;
    let unlistenEnter: (() => void) | undefined;
    let unlistenLeave: (() => void) | undefined;

    listen<{ paths?: string[] }>("tauri://drag-drop", (ev) => {
      setIsDragOver(false);
      const droppedPaths = ev.payload?.paths;
      if (Array.isArray(droppedPaths) && droppedPaths.length > 0) {
        invoke<number>("import_files_to_drive", {
          paths: droppedPaths,
          targetSubfolder: currentFolder || null,
        })
          .then((count) => {
            setActionFeedback({
              type: "ok",
              text: `${count} arquivo(s) importado(s) via Drag & Drop para sincronização.`,
            });
          })
          .catch((err) => {
            setActionFeedback({ type: "error", text: String(err) });
          });
      }
    })
      .then((fn) => {
        unlistenDrop = fn;
      })
      .catch(() => {});

    listen("tauri://drag-enter", () => setIsDragOver(true))
      .then((fn) => {
        unlistenEnter = fn;
      })
      .catch(() => {});

    listen("tauri://drag-leave", () => setIsDragOver(false))
      .then((fn) => {
        unlistenLeave = fn;
      })
      .catch(() => {});

    return () => {
      unlistenDrop?.();
      unlistenEnter?.();
      unlistenLeave?.();
    };
  }, [currentFolder]);

  useEffect(() => {
    if (!selectedFileKey) {
      setSelectedMetadata(null);
      setObjectVersions([]);
      setGeneratedPresignedUrl(null);
      setRenameTargetKey("");
      return;
    }
    setGeneratedPresignedUrl(null);
    setObjectVersions([]);
    setRenameTargetKey(selectedFileKey);
    invoke<ObjectDetailedMetadata>("get_object_details", {
      s3Key: selectedFileKey,
    })
      .then((meta) => {
        if (meta) setSelectedMetadata(meta);
      })
      .catch(() => {});

    invoke<S3ObjectVersion[]>("list_object_versions", {
      s3Key: selectedFileKey,
    })
      .then((vers) => {
        if (Array.isArray(vers)) setObjectVersions(vers);
      })
      .catch(() => {});
  }, [selectedFileKey]);

  useEffect(() => {
    if (!showAnalyticsBar) return;
    invoke<StorageAnalytics>("get_storage_analytics")
      .then((stats) => {
        if (stats && typeof stats.total_files === "number") {
          setStorageAnalytics(stats);
        }
      })
      .catch(() => {});
  }, [showAnalyticsBar, fileStatuses]);

  const badge = renderGlobalBadge(globalState);

  const counts = useMemo(() => {
    const total = fileStatuses.length;
    const synced = fileStatuses.filter((f) => f.state.type === "Synced").length;
    const syncing = fileStatuses.filter(
      (f) => f.state.type === "Syncing"
    ).length;
    const pending = fileStatuses.filter(
      (f) => f.state.type === "Pending"
    ).length;
    const errors = fileStatuses.filter((f) => f.state.type === "Error").length;
    return { total, synced, syncing, pending, errors };
  }, [fileStatuses]);

  const filteredFiles = useMemo(() => {
    const list = fileStatuses.filter((file) => {
      const matchesSearch =
        !searchQuery.trim() ||
        file.path.toLowerCase().includes(searchQuery.toLowerCase());
      const matchesFilter =
        stateFilter === "ALL" || file.state.type === stateFilter;
      if (!matchesSearch || !matchesFilter) return false;
      if (viewMode === "tree" && currentFolder) {
        return file.path.startsWith(currentFolder);
      }
      return true;
    });

    const dirMult = sortAsc ? 1 : -1;
    return [...list].sort((a, b) => {
      if (sortField === "size") {
        return ((a.size_bytes || 0) - (b.size_bytes || 0)) * dirMult;
      }
      if (sortField === "time") {
        return (
          (a.last_modified || "").localeCompare(b.last_modified || "") * dirMult
        );
      }
      if (sortField === "state") {
        return a.state.type.localeCompare(b.state.type) * dirMult;
      }
      return a.path.localeCompare(b.path) * dirMult;
    });
  }, [
    fileStatuses,
    searchQuery,
    stateFilter,
    viewMode,
    currentFolder,
    sortField,
    sortAsc,
  ]);

  const treeEntries = useMemo(() => {
    if (viewMode !== "tree") {
      return { folders: [] as { name: string; prefix: string; count: number; totalBytes: number }[], files: filteredFiles };
    }
    const folderMap = new Map<string, { count: number; totalBytes: number }>();
    const directFiles: FileStatus[] = [];

    for (const file of filteredFiles) {
      const rel = currentFolder
        ? file.path.slice(currentFolder.length)
        : file.path;
      const slashIdx = rel.indexOf("/");
      if (slashIdx === -1) {
        directFiles.push(file);
      } else {
        const folderName = rel.slice(0, slashIdx);
        const prev = folderMap.get(folderName) ?? { count: 0, totalBytes: 0 };
        folderMap.set(folderName, {
          count: prev.count + 1,
          totalBytes: prev.totalBytes + (file.size_bytes || 0),
        });
      }
    }

    const folders = Array.from(folderMap.entries())
      .map(([name, info]) => ({
        name,
        prefix: `${currentFolder}${name}/`,
        count: info.count,
        totalBytes: info.totalBytes,
      }))
      .sort((a, b) => a.name.localeCompare(b.name));

    return { folders, files: directFiles };
  }, [viewMode, filteredFiles, currentFolder]);

  const breadcrumbSegments = useMemo(() => {
    if (!currentFolder) return [];
    const parts = currentFolder.split("/").filter(Boolean);
    return parts.map((part, idx) => ({
      label: part,
      prefix: parts.slice(0, idx + 1).join("/") + "/",
    }));
  }, [currentFolder]);

  const handleSyncNow = async () => {
    try {
      await invoke("start_sync");
    } catch {
      // Handled via event/banner
    }
  };

  const handleStopSync = async () => {
    try {
      await invoke("stop_sync");
    } catch {
      // Ignore
    }
  };

  const handleOpenFile = async (s3Key: string) => {
    try {
      await invoke("open_file_in_os", { s3Key });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleRevealFile = async (s3Key: string) => {
    try {
      await invoke("reveal_file_in_folder", { s3Key });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleCopyPresignedUrl = async (s3Key: string) => {
    try {
      const url = await invoke<string>("get_presigned_url", {
        s3Key,
        expiresInSecs: 3600,
      });
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(url);
      }
      setActionFeedback({
        type: "ok",
        text: `Link presigned (1h) copiado para a área de transferência: ${s3Key}`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleRetryFile = async (s3Key: string) => {
    try {
      await invoke("retry_file_sync", { s3Key });
      setActionFeedback({
        type: "ok",
        text: `Sincronização reenfileirada para ${s3Key}`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleRetryAllErrors = async () => {
    try {
      const count = await invoke<number>("retry_all_errors");
      setActionFeedback({
        type: "ok",
        text: `${count} arquivo(s) com erro reenfileirado(s) para sincronização.`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleUploadFilesDialog = async () => {
    try {
      const selectedPaths = await invoke<string[]>("open_files_dialog");
      if (!selectedPaths || selectedPaths.length === 0) return;
      const count = await invoke<number>("import_files_to_drive", {
        paths: selectedPaths,
        targetSubfolder: currentFolder || null,
      });
      setActionFeedback({
        type: "ok",
        text: `${count} arquivo(s) importado(s) e enfileirado(s) para upload.`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleGenerateCustomPresigned = async (
    s3Key: string,
    expiresInSecs: number
  ) => {
    try {
      const url = await invoke<string>("get_presigned_url", {
        s3Key,
        expiresInSecs,
      });
      setGeneratedPresignedUrl(url);
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(url);
      }
      setActionFeedback({
        type: "ok",
        text: `Link presigned copiado para a área de transferência (${s3Key}).`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleSort = (field: "path" | "size" | "time" | "state") => {
    if (sortField === field) {
      setSortAsc((prev) => !prev);
    } else {
      setSortField(field);
      setSortAsc(true);
    }
  };

  const handleCreateSubfolder = async () => {
    const trimmed = newFolderName.trim();
    if (!trimmed) return;
    try {
      const prefix = await invoke<string>("create_subfolder", {
        parentPrefix: currentFolder || null,
        folderName: trimmed,
      });
      setNewFolderName("");
      setShowNewFolderInput(false);
      setViewMode("tree");
      setCurrentFolder(prefix);
      setActionFeedback({
        type: "ok",
        text: `Pasta criada localmente: ${prefix}`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleDeleteObject = async (s3Key: string) => {
    try {
      await invoke("delete_object_sync", { s3Key });
      setFileStatuses(fileStatuses.filter((f) => f.path !== s3Key));
      if (selectedFileKey === s3Key) {
        setSelectedFileKey(null);
      }
      setActionFeedback({
        type: "ok",
        text: `Objeto removido localmente e enfileirado para exclusão no S3: ${s3Key}`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleRestoreVersion = async (s3Key: string, versionId: string) => {
    try {
      await invoke("restore_object_version", { s3Key, versionId });
      setActionFeedback({
        type: "ok",
        text: `Versão ${versionId.slice(0, 10)} restaurada com sucesso para ${s3Key}.`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleToggleBatchKey = (s3Key: string) => {
    setSelectedBatchKeys((prev) =>
      prev.includes(s3Key) ? prev.filter((k) => k !== s3Key) : [...prev, s3Key]
    );
  };

  const visibleFileRows =
    viewMode === "tree" ? treeEntries.files : filteredFiles;

  const allVisibleSelected =
    visibleFileRows.length > 0 &&
    visibleFileRows.every((f) => selectedBatchKeys.includes(f.path));

  const handleToggleAllVisible = () => {
    if (allVisibleSelected) {
      const visibleSet = new Set(visibleFileRows.map((f) => f.path));
      setSelectedBatchKeys((prev) => prev.filter((k) => !visibleSet.has(k)));
    } else {
      const merged = new Set([
        ...selectedBatchKeys,
        ...visibleFileRows.map((f) => f.path),
      ]);
      setSelectedBatchKeys(Array.from(merged));
    }
  };

  const handleBatchResync = async () => {
    if (selectedBatchKeys.length === 0) return;
    try {
      const count = await invoke<number>("batch_resync_objects", {
        s3Keys: selectedBatchKeys,
      });
      setActionFeedback({
        type: "ok",
        text: `${count} objeto(s) reenfileirado(s) em lote para sincronização.`,
      });
      setSelectedBatchKeys([]);
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleBatchCopyLinks = async () => {
    if (selectedBatchKeys.length === 0) return;
    try {
      const urls = await invoke<string[]>("batch_presigned_urls", {
        s3Keys: selectedBatchKeys,
        expiresInSecs: 3600,
      });
      if (navigator.clipboard && navigator.clipboard.writeText) {
        await navigator.clipboard.writeText(urls.join("\n"));
      }
      setActionFeedback({
        type: "ok",
        text: `${urls.length} link(s) presigned (1h) copiado(s) para a área de transferência.`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleBatchDelete = async () => {
    if (selectedBatchKeys.length === 0) return;
    try {
      const count = await invoke<number>("batch_delete_objects", {
        s3Keys: selectedBatchKeys,
      });
      const delSet = new Set(selectedBatchKeys);
      setFileStatuses(fileStatuses.filter((f) => !delSet.has(f.path)));
      if (selectedFileKey && delSet.has(selectedFileKey)) {
        setSelectedFileKey(null);
      }
      setSelectedBatchKeys([]);
      setActionFeedback({
        type: "ok",
        text: `${count} objeto(s) excluído(s) localmente e enfileirado(s) para remoção no S3.`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleRenameOrMoveObject = async () => {
    if (!selectedFileKey) return;
    const trimmed = renameTargetKey.trim();
    if (!trimmed || trimmed === selectedFileKey) return;
    try {
      const newKey = await invoke<string>("rename_or_move_object", {
        oldS3Key: selectedFileKey,
        newS3Key: trimmed,
      });
      setFileStatuses(
        fileStatuses.map((f) =>
          f.path === selectedFileKey ? { ...f, path: newKey } : f
        )
      );
      setSelectedFileKey(newKey);
      setActionFeedback({
        type: "ok",
        text: `Objeto movido/renomeado para ${newKey}`,
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  return (
    <div className="pane-container">
      {/* Top Command Bar (Estilo Linear / Panic Transmit) */}
      <header className="command-bar">
        <div className="command-bar-left">
          <h2 className="pane-title">Objetos & Transferências</h2>
          <div
            data-testid="global-state-badge"
            className={`state-tag mono ${badge.badgeClass}`}
          >
            {badge.text}
          </div>
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
              placeholder="Filtrar chaves S3..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              aria-label="Filtrar arquivos"
            />
          </div>

          <div className="segmented-filter">
            {(
              [
                { id: "ALL", label: `Todos (${counts.total})` },
                { id: "Synced", label: `OK (${counts.synced})` },
                {
                  id: "Syncing",
                  label: `Ativos (${counts.syncing + counts.pending})`,
                },
                { id: "Error", label: `Erros (${counts.errors})` },
              ] as const
            ).map((item) => (
              <button
                key={item.id}
                type="button"
                className={`seg-btn ${
                  stateFilter === item.id ? "seg-active" : ""
                }`}
                onClick={() => setStateFilter(item.id)}
              >
                {item.label}
              </button>
            ))}
          </div>

          <div className="segmented-filter">
            <button
              type="button"
              className={`seg-btn ${viewMode === "flat" ? "seg-active" : ""}`}
              onClick={() => {
                setViewMode("flat");
                setCurrentFolder("");
              }}
              title="Exibir lista plana de chaves S3"
            >
              Lista
            </button>
            <button
              type="button"
              className={`seg-btn ${viewMode === "tree" ? "seg-active" : ""}`}
              onClick={() => setViewMode("tree")}
              title="Navegar por hierarquia de pastas"
            >
              Pastas
            </button>
          </div>

          <div className="toolbar-divider" />

          {counts.errors > 0 && (
            <button
              type="button"
              className="btn-compact row-act-retry"
              onClick={handleRetryAllErrors}
              title="Reenfileirar todos os arquivos com erro"
            >
              Retry Erros ({counts.errors})
            </button>
          )}

          <button
            type="button"
            className={`btn-compact ${
              showAnalyticsBar ? "row-act-active" : ""
            }`}
            onClick={() => setShowAnalyticsBar((prev) => !prev)}
            title="Exibir métricas de armazenamento e estimativa de custo S3"
          >
            Métricas
          </button>

          <button
            type="button"
            className={`btn-compact ${
              showNewFolderInput ? "row-act-active" : ""
            }`}
            onClick={() => setShowNewFolderInput((prev) => !prev)}
            title="Criar nova subpasta na localização atual"
          >
            Nova Pasta
          </button>

          <button
            type="button"
            className="btn-compact"
            onClick={handleUploadFilesDialog}
            title="Importar e enviar arquivos para a pasta atual"
          >
            Enviar Arquivo(s)
          </button>

          <button
            type="button"
            className="btn-compact"
            onClick={handleSyncNow}
            title="Forçar sincronização imediata"
          >
            Sync
          </button>
          <button
            type="button"
            className="btn-compact"
            onClick={handleStopSync}
            title="Pausar motor de sincronização"
          >
            Pausar
          </button>
        </div>
      </header>

      {showNewFolderInput && (
        <div className="new-folder-bar">
          <span className="mono new-folder-prefix">
            s3://root/{currentFolder}
          </span>
          <input
            type="text"
            className="mono"
            placeholder="nome-da-subpasta"
            value={newFolderName}
            onChange={(e) => setNewFolderName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleCreateSubfolder();
              if (e.key === "Escape") setShowNewFolderInput(false);
            }}
            aria-label="Nome da nova pasta"
          />
          <button
            type="button"
            className="btn-compact btn-primary-accent"
            onClick={handleCreateSubfolder}
          >
            Criar Pasta
          </button>
          <button
            type="button"
            className="btn-compact"
            onClick={() => setShowNewFolderInput(false)}
          >
            Cancelar
          </button>
        </div>
      )}

      {showAnalyticsBar && storageAnalytics && (
        <div className="analytics-strip mono" aria-label="Métricas de Armazenamento S3">
          <div className="analytics-metric">
            <span className="analytics-label">VOLUME TOTAL</span>
            <span className="analytics-val">
              {formatBytes(storageAnalytics.total_bytes)}
            </span>
          </div>
          <div className="analytics-metric">
            <span className="analytics-label">OBJETOS</span>
            <span className="analytics-val">
              {storageAnalytics.total_files} ({storageAnalytics.synced_files} OK)
            </span>
          </div>
          <div className="analytics-metric">
            <span className="analytics-label">STORAGE CLASS</span>
            <span className="analytics-val">
              {storageAnalytics.storage_class}
            </span>
          </div>
          <div className="analytics-metric">
            <span className="analytics-label">CUSTO S3 EST.</span>
            <span className="analytics-val analytics-accent">
              US$ {storageAnalytics.estimated_monthly_usd.toFixed(4)} / mês
            </span>
          </div>
          {storageAnalytics.top_extensions.length > 0 && (
            <div className="analytics-metric analytics-exts">
              <span className="analytics-label">TOP EXTENSÕES</span>
              <span className="analytics-val">
                {storageAnalytics.top_extensions
                  .map(
                    (ext) =>
                      `${ext.extension}: ${formatBytes(ext.bytes)} (${ext.count})`
                  )
                  .join(" · ")}
              </span>
            </div>
          )}
        </div>
      )}

      {selectedBatchKeys.length > 0 && (
        <div className="batch-action-bar" role="region" aria-label="Ações em lote">
          <span className="mono batch-count">
            {selectedBatchKeys.length} objeto(s) selecionado(s)
          </span>
          <div className="batch-btns">
            <button
              type="button"
              className="btn-compact"
              onClick={handleBatchResync}
              title="Reenfileirar todos os selecionados para sincronização"
            >
              Re-sync Lote ({selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact"
              onClick={handleBatchCopyLinks}
              title="Gerar e copiar Presigned URLs (1h) de todos os selecionados"
            >
              Copiar Links ({selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact btn-danger-compact"
              onClick={handleBatchDelete}
              title="Excluir selecionados localmente e no bucket S3"
            >
              Excluir Lote ({selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact"
              onClick={() => setSelectedBatchKeys([])}
            >
              Limpar Seleção
            </button>
          </div>
        </div>
      )}

      {queueStats.is_full && (
        <div role="alert" className="banner banner-error">
          Capacidade máxima da fila atingida (10.000 operações). Novas operações
          foram bloqueadas.
        </div>
      )}

      {actionFeedback && (
        <div
          role="status"
          className={`banner ${
            actionFeedback.type === "ok" ? "banner-ok" : "banner-error"
          }`}
        >
          <span className="mono">{actionFeedback.text}</span>
          <button
            type="button"
            className="btn-compact"
            onClick={() => setActionFeedback(null)}
          >
            Fechar
          </button>
        </div>
      )}

      {viewMode === "tree" && (
        <div className="breadcrumb-bar mono">
          <button
            type="button"
            className={`crumb-btn ${!currentFolder ? "crumb-active" : ""}`}
            onClick={() => setCurrentFolder("")}
          >
            s3://root
          </button>
          {breadcrumbSegments.map((seg) => (
            <React.Fragment key={seg.prefix}>
              <span className="crumb-sep">/</span>
              <button
                type="button"
                className={`crumb-btn ${
                  currentFolder === seg.prefix ? "crumb-active" : ""
                }`}
                onClick={() => setCurrentFolder(seg.prefix)}
              >
                {seg.label}
              </button>
            </React.Fragment>
          ))}
        </div>
      )}

      {isDragOver && (
        <div className="dropzone-banner mono">
          SOLTE OS ARQUIVOS AQUI PARA IMPORTAR E ENVIAR AO S3 (
          {currentFolder || "s3://root"})
        </div>
      )}

      {/* Tabela Densa de Arquivos + Painel Inspector Lateral */}
      {fileStatuses.length === 0 ? (
        <div className="empty-table-view">
          <p className="empty-status">Nenhum arquivo rastreado no momento.</p>
          <span className="empty-subtext">
            Inicie o motor de sincronização, clique em &ldquo;Enviar
            Arquivo(s)&rdquo; ou arraste arquivos para cá.
          </span>
        </div>
      ) : (
        <div className="status-split-body">
          <div className="table-viewport">
            <div className="table-head">
              <span className="col-st">ST</span>
              <span
                className="col-key sortable-col"
                onClick={() => handleSort("path")}
                title="Ordenar por Chave S3"
              >
                <input
                  type="checkbox"
                  className="row-check"
                  checked={allVisibleSelected}
                  onChange={handleToggleAllVisible}
                  onClick={(e) => e.stopPropagation()}
                  aria-label="Selecionar todos os arquivos visíveis"
                />
                CHAVE DO OBJETO S3{" "}
                {sortField === "path" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span
                className="col-size sortable-col"
                onClick={() => handleSort("size")}
                title="Ordenar por Tamanho"
              >
                TAMANHO {sortField === "size" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span
                className="col-time sortable-col"
                onClick={() => handleSort("time")}
                title="Ordenar por Data de Modificação"
              >
                MODIFICADO (UTC){" "}
                {sortField === "time" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span
                className="col-status sortable-col"
                onClick={() => handleSort("state")}
                title="Ordenar por Estado"
              >
                ESTADO {sortField === "state" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span className="col-actions">AÇÕES</span>
            </div>

            <ul className="file-status-list" aria-label="Lista de arquivos">
              {viewMode === "tree" &&
                treeEntries.folders.map((folder) => (
                  <li
                    key={`folder-${folder.prefix}`}
                    className="table-row folder-row"
                    onClick={() => setCurrentFolder(folder.prefix)}
                  >
                    <span className="col-st state-icon-cell">
                      <svg
                        width="13"
                        height="13"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                      >
                        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                      </svg>
                    </span>
                    <span className="col-key mono">
                      <span className="folder-name">{folder.name}/</span>
                    </span>
                    <span className="col-size mono">
                      {formatBytes(folder.totalBytes)}
                    </span>
                    <span className="col-time mono">—</span>
                    <span className="col-status mono">
                      {folder.count} {folder.count === 1 ? "item" : "itens"}
                    </span>
                    <span className="col-actions">
                      <button
                        type="button"
                        className="row-act-btn"
                        onClick={(e) => {
                          e.stopPropagation();
                          setCurrentFolder(folder.prefix);
                        }}
                      >
                        Abrir Pasta
                      </button>
                    </span>
                  </li>
                ))}

              {(viewMode === "tree" ? treeEntries.files : filteredFiles).map(
                (file) => {
                  const meta = renderStateMeta(file.state);
                  const { dir, base } = splitPath(file.path);
                  const prog = transferProgress[file.path];
                  const isSelected = selectedFileKey === file.path;

                  return (
                    <li
                      key={file.path}
                      className={`table-row ${meta.className} ${
                        isSelected ? "row-selected" : ""
                      }`}
                      data-testid={`file-item-${file.path}`}
                      onClick={() =>
                        setSelectedFileKey(isSelected ? null : file.path)
                      }
                    >
                      <span
                        className={`col-st state-icon-cell ${meta.className}`}
                        data-testid={`icon-${file.state.type}`}
                        aria-label={meta.label}
                      >
                        {renderStateVectorIcon(file.state)}
                        <span className="sr-only">{meta.srIcon}</span>
                      </span>

                      <span className="col-key mono" title={file.path}>
                        <input
                          type="checkbox"
                          className="row-check"
                          checked={selectedBatchKeys.includes(file.path)}
                          onChange={() => handleToggleBatchKey(file.path)}
                          onClick={(e) => e.stopPropagation()}
                          aria-label={`Selecionar ${file.path}`}
                        />
                        {viewMode === "flat" && dir && (
                          <span
                            className="key-prefix key-prefix-clickable"
                            onClick={(e) => {
                              e.stopPropagation();
                              setViewMode("tree");
                              setCurrentFolder(dir);
                            }}
                            title={`Filtrar pasta ${dir}`}
                          >
                            {dir}
                          </span>
                        )}
                        <span className="key-name">{base}</span>
                      </span>

                      <span className="col-size mono">
                        {file.size_bytes > 0
                          ? formatBytes(file.size_bytes)
                          : "—"}
                      </span>

                      <span className="col-time mono">
                        {file.last_modified
                          ? file.last_modified.replace("T", " ").slice(0, 19)
                          : "—"}
                      </span>

                      <span className="col-status" title={meta.label}>
                        <span className={`status-code mono ${meta.className}`}>
                          {file.state.type === "Syncing" && prog
                            ? `${meta.label} (${prog.percent}%)`
                            : meta.label}
                        </span>
                        {file.state.type === "Syncing" && prog && (
                          <span className="inline-progress-track">
                            <span
                              className="inline-progress-fill"
                              style={{ width: `${prog.percent}%` }}
                            />
                          </span>
                        )}
                      </span>

                      <span
                        className="col-actions"
                        onClick={(e) => e.stopPropagation()}
                      >
                        {file.state.type === "Error" && (
                          <button
                            type="button"
                            className="row-act-btn row-act-retry"
                            onClick={() => handleRetryFile(file.path)}
                            title="Tentar sincronizar novamente"
                          >
                            Retry
                          </button>
                        )}
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleOpenFile(file.path)}
                          title="Abrir arquivo no aplicativo padrão"
                        >
                          Abrir
                        </button>
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleRevealFile(file.path)}
                          title="Revelar no Explorer / Finder"
                        >
                          Pasta
                        </button>
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleCopyPresignedUrl(file.path)}
                          title="Copiar link temporário S3 (Presigned URL 1h)"
                        >
                          Link
                        </button>
                        <button
                          type="button"
                          className={`row-act-btn ${
                            isSelected ? "row-act-active" : ""
                          }`}
                          onClick={() =>
                            setSelectedFileKey(isSelected ? null : file.path)
                          }
                          title="Inspecionar metadados técnicos do objeto"
                        >
                          Info
                        </button>
                      </span>
                    </li>
                  );
                }
              )}
            </ul>
          </div>

          {selectedFileKey && (
            <aside className="inspector-drawer" aria-label="Inspector de Objeto">
              <div className="inspector-header">
                <span className="inspector-title">INSPECTOR S3</span>
                <button
                  type="button"
                  className="row-act-btn"
                  onClick={() => setSelectedFileKey(null)}
                >
                  Fechar
                </button>
              </div>

              <div className="inspector-body">
                <div className="inspector-kv">
                  <span className="inspector-k">CHAVE S3</span>
                  <span className="inspector-v mono break-all">
                    {selectedFileKey}
                  </span>
                </div>

                {selectedMetadata && (
                  <>
                    <div className="inspector-kv">
                      <span className="inspector-k">CAMINHO LOCAL</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.local_path}
                      </span>
                    </div>

                    <div className="inspector-grid-2">
                      <div className="inspector-kv">
                        <span className="inspector-k">TAMANHO</span>
                        <span className="inspector-v mono">
                          {formatBytes(selectedMetadata.size_bytes)} (
                          {selectedMetadata.size_bytes} B)
                        </span>
                      </div>
                      <div className="inspector-kv">
                        <span className="inspector-k">STORAGE CLASS</span>
                        <span className="inspector-v mono">
                          {selectedMetadata.storage_class}
                        </span>
                      </div>
                    </div>

                    <div className="inspector-grid-2">
                      <div className="inspector-kv">
                        <span className="inspector-k">CRIPTOGRAFIA</span>
                        <span className="inspector-v mono">
                          {selectedMetadata.server_side_encryption
                            ? "AES-256 (SSE-S3)"
                            : "Padrão do Bucket"}
                        </span>
                      </div>
                      <div className="inspector-kv">
                        <span className="inspector-k">ARQUIVO LOCAL</span>
                        <span className="inspector-v mono">
                          {selectedMetadata.local_exists
                            ? "Presente em disco"
                            : "Apenas remoto"}
                        </span>
                      </div>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">ETAG REMOTO</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.etag || "—"}
                      </span>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">CHECKSUM MD5 LOCAL</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.local_md5 || "—"}
                      </span>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">ÚLTIMO SYNC (UTC)</span>
                      <span className="inspector-v mono">
                        {selectedMetadata.last_sync_time || "—"}
                      </span>
                    </div>
                  </>
                )}

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">
                    COMPARTILHAR LINK TEMPORÁRIO (PRESIGNED URL)
                  </span>
                  <div className="preset-tags">
                    {[
                      { secs: 900, label: "15 min" },
                      { secs: 3600, label: "1 hora" },
                      { secs: 86400, label: "24 horas" },
                      { secs: 604800, label: "7 dias" },
                    ].map((opt) => (
                      <button
                        key={opt.secs}
                        type="button"
                        className={`tag-btn mono ${
                          presignExpirySecs === opt.secs ? "tag-btn-active" : ""
                        }`}
                        onClick={() => setPresignExpirySecs(opt.secs)}
                      >
                        {opt.label}
                      </button>
                    ))}
                  </div>
                  <button
                    type="button"
                    className="btn-compact inspector-full-btn"
                    onClick={() =>
                      handleGenerateCustomPresigned(
                        selectedFileKey,
                        presignExpirySecs
                      )
                    }
                  >
                    Gerar & Copiar Presigned URL
                  </button>
                  {generatedPresignedUrl && (
                    <div className="presigned-preview mono">
                      {generatedPresignedUrl}
                    </div>
                  )}
                </div>

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">
                    HISTÓRICO DE VERSÕES S3 ({objectVersions.length})
                  </span>
                  {objectVersions.length === 0 ? (
                    <span className="prop-sub">
                      Nenhuma versão listada (Bucket sem S3 Versioning ativo ou
                      objeto ainda não enviado).
                    </span>
                  ) : (
                    <ul className="version-list">
                      {objectVersions.map((ver) => (
                        <li key={ver.version_id} className="version-item">
                          <div className="version-meta">
                            <span className="mono version-id">
                              {ver.version_id.slice(0, 12)}
                              {ver.is_latest && (
                                <span className="version-latest-tag">
                                  LATEST
                                </span>
                              )}
                            </span>
                            <span className="mono version-sub">
                              {ver.last_modified
                                ? ver.last_modified
                                    .replace("T", " ")
                                    .slice(0, 19)
                                : "—"}{" "}
                              · {formatBytes(ver.size_bytes)}
                            </span>
                          </div>
                          <button
                            type="button"
                            className="row-act-btn"
                            onClick={() =>
                              handleRestoreVersion(
                                selectedFileKey,
                                ver.version_id
                              )
                            }
                            title="Baixar e restaurar esta versão S3 para o arquivo local"
                          >
                            Restaurar
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">
                    RENOMEAR / MOVER CHAVE S3
                  </span>
                  <input
                    type="text"
                    className="mono"
                    value={renameTargetKey}
                    onChange={(e) => setRenameTargetKey(e.target.value)}
                    placeholder="nova-pasta/novo-nome.ext"
                    aria-label="Nova chave S3"
                  />
                  <button
                    type="button"
                    className="btn-compact inspector-full-btn"
                    disabled={
                      !renameTargetKey.trim() ||
                      renameTargetKey.trim() === selectedFileKey
                    }
                    onClick={handleRenameOrMoveObject}
                  >
                    Aplicar Renomeação / Mover
                  </button>
                </div>

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">OPERAÇÕES DESTRUTIVAS</span>
                  <button
                    type="button"
                    className="btn-compact btn-danger-compact inspector-full-btn"
                    onClick={() => handleDeleteObject(selectedFileKey)}
                    title="Excluir arquivo local e enfileirar Delete no bucket S3"
                  >
                    Excluir Objeto (Local + S3)
                  </button>
                </div>
              </div>
            </aside>
          )}
        </div>
      )}
    </div>
  );
};
