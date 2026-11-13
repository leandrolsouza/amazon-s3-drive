import { UploadDialog } from "../components/UploadDialog";
import { Icon } from "../components/Icon";
import React, { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSyncStore } from "../store";
import { syncPresentation } from "../syncPresentation";
import { runSyncCommand } from "../syncCommands";
import { DeleteConfirmation } from "../components/DeleteConfirmation";
import {
  FileStatus,
  ObjectDetailedMetadata,
  QueueStats,
  S3ObjectVersion,
  SyncState,
} from "../types";
import { locale, t } from "../i18n";

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
        label: t("Pending"),
        code: "QUEUED",
        className: "state-pending",
      };
    case "Syncing":
      return {
        srIcon: "🔄",
        label: t("Syncing"),
        code: "SYNC",
        className: "state-syncing",
      };
    case "Synced":
      return {
        srIcon: "✅",
        label: t("Synced"),
        code: "OK",
        className: "state-synced",
      };
    case "Error":
      return {
        srIcon: "❌",
        label: t("Error: {0}", state.message),
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

export function formatBytes(bytes: number): string {
  if (!bytes || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1
  );
  const val = bytes / Math.pow(1024, i);
  return `${val.toLocaleString(locale, { minimumFractionDigits: i === 0 ? 0 : 1, maximumFractionDigits: i === 0 ? 0 : 1, useGrouping: false })} ${units[i]}`;
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
  const badge = syncPresentation(useSyncStore());
  const syncAction = useSyncStore((s) => s.syncAction);
  const syncRunning = useSyncStore((s) => s.syncRunning);
  const [pendingDelete, setPendingDelete] = useState<string[] | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState(false);
  const queueStats = useSyncStore((s) => s.queueStats);
  const setFileStatuses = useSyncStore((s) => s.setFileStatuses);
  const setQueueStats = useSyncStore((s) => s.setQueueStats);

  const [searchQuery, setSearchQuery] = useState("");
  const [stateFilter, setStateFilter] = useState<
    "ALL" | "Synced" | "Active" | "Error"
  >("ALL");
  const [viewMode, setViewMode] = useState<"flat" | "tree">("tree");
  const [currentFolder, setCurrentFolder] = useState<string>("");
  const [uploadPaths, setUploadPaths] = useState<string[] | null>(null);
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
  const [renameTargetKey, setRenameTargetKey] = useState<string>("");
  const [actionFeedback, setActionFeedback] = useState<{
    type: "ok" | "error";
    text: string;
  } | null>(null);

  useEffect(() => {
    if (actionFeedback?.type !== "ok") return;
    const timer = setTimeout(() => setActionFeedback(null), 5500);
    return () => clearTimeout(timer);
  }, [actionFeedback]);

  useEffect(() => {
    let disposed = false;
    invoke<FileStatus[]>("get_sync_status")
      .then((statuses) => {
        if (!disposed && Array.isArray(statuses) && statuses.length > 0) {
          setFileStatuses(statuses);
        }
      })
      .catch(() => {});

    invoke<QueueStats>("get_queue_stats")
      .then((stats) => {
        if (!disposed && stats && typeof stats.pending_count === "number") {
          setQueueStats(stats);
        }
      })
      .catch(() => {});
    return () => { disposed = true; };
  }, [setFileStatuses, setQueueStats, syncRunning]);

  useEffect(() => {
    let unlistenDrop: (() => void) | undefined;
    let unlistenEnter: (() => void) | undefined;
    let unlistenLeave: (() => void) | undefined;

    listen<{ paths?: string[] }>("tauri://drag-drop", (ev) => {
      setIsDragOver(false);
      const droppedPaths = ev.payload?.paths;
      if (Array.isArray(droppedPaths) && droppedPaths.length > 0) {
        setUploadPaths((open) => open ?? droppedPaths);
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
        stateFilter === "ALL" || (stateFilter === "Active"
          ? file.state.type === "Syncing" || file.state.type === "Pending"
          : file.state.type === stateFilter);
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
      await runSyncCommand("start_sync");
      setActionFeedback({ type: "ok", text: t("Sync started.") });
    } catch {
      setActionFeedback({ type: "error", text: t("Could not start sync. Check your settings and credentials, then select Start to try again.") });
    }
  };

  const handleStopSync = async () => {
    try {
      await runSyncCommand("stop_sync");
      setActionFeedback({ type: "ok", text: t("Sync stopped.") });
    } catch {
      setActionFeedback({ type: "error", text: t("Could not stop sync. Select Stop to try again.") });
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
        text: t("Presigned link (1 hour) copied to clipboard: {0}", s3Key),
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
        text: t("Sync requeued for {0}", s3Key),
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
        text: t("{0} failed file(s) requeued for sync.", count),
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
        text: t("Presigned link copied to clipboard ({0}).", s3Key),
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
        text: t("Folder created locally: {0}", prefix),
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
        text: t("Version {0} restored for {1}.", versionId.slice(0, 10), s3Key),
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
        text: t("{0} object(s) requeued for sync.", count),
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
        text: t("{0} presigned link(s) (1 hour) copied to clipboard.", urls.length),
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  const handleConfirmDelete = async () => {
    if (!pendingDelete || deleting) return;
    setDeleting(true);
    setDeleteError(false);
    try {
      if (pendingDelete.length === 1) {
        await invoke("delete_object_sync", { s3Key: pendingDelete[0] });
      } else {
        await invoke<number>("batch_delete_objects", { s3Keys: pendingDelete });
      }
      const deleted = new Set(pendingDelete);
      const store = useSyncStore.getState();
      setFileStatuses(store.fileStatuses.filter((f) => !deleted.has(f.path)));
      if (selectedFileKey && deleted.has(selectedFileKey)) setSelectedFileKey(null);
      setSelectedBatchKeys((keys) => keys.filter((key) => !deleted.has(key)));
      setActionFeedback({ type: "ok", text: t("{0} file(s) deleted locally and queued for removal from S3.", pendingDelete.length) });
      setPendingDelete(null);
    } catch {
      setDeleteError(true);
    } finally {
      setDeleting(false);
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
        text: t("Object moved or renamed to {0}", newKey),
      });
    } catch (err) {
      setActionFeedback({ type: "error", text: String(err) });
    }
  };

  return (
    <div className="pane-container">
      {uploadPaths && (
        <UploadDialog
          currentFolder={currentFolder}
          initialPaths={uploadPaths}
          onCancel={() => setUploadPaths(null)}
          onImported={(count) => {
            setUploadPaths(null);
            setActionFeedback({ type: "ok", text: t("{0} file(s) imported for sync.", count) });
          }}
        />
      )}
      {pendingDelete && <DeleteConfirmation keysToDelete={pendingDelete} busy={deleting} failed={deleteError} onCancel={() => setPendingDelete(null)} onConfirm={handleConfirmDelete} />}
      {/* Top Command Bar (Estilo Linear / Panic Transmit) */}
      <header className="command-bar">
        <div className="command-bar-left">
          <h2 className="pane-title">{t("Objects & Transfers")}</h2>
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
              placeholder={t("Filter S3 keys...")}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              aria-label={t("Filter files")}
            />
          </div>

          <div className="segmented-filter">
            {(
              [
                { id: "ALL", label: t("All ({0})", counts.total) },
                { id: "Synced", label: `OK (${counts.synced})` },
                {
                  id: "Active",
                  label: t("Active ({0})", counts.syncing + counts.pending),
                },
                { id: "Error", label: t("Errors ({0})", counts.errors) },
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
              className={`seg-btn toolbar-icon ${viewMode === "flat" ? "seg-active" : ""}`}
              aria-pressed={viewMode === "flat"}
              aria-label={t("List")}
              onClick={() => {
                setViewMode("flat");
                setCurrentFolder("");
              }}
              title={t("Show a flat list of S3 keys")}
            >
              <Icon name="list" />
            </button>
            <button
              type="button"
              className={`seg-btn toolbar-icon ${viewMode === "tree" ? "seg-active" : ""}`}
              aria-pressed={viewMode === "tree"}
              aria-label={t("Folders")}
              onClick={() => setViewMode("tree")}
              title={t("Browse folders")}
            >
              <Icon name="folder" />
            </button>
          </div>

          <div className="toolbar-divider" />

          {counts.errors > 0 && (
            <button
              type="button"
              className="btn-compact toolbar-icon row-act-retry"
              aria-label={t("Retry {0} failed file(s)", counts.errors)}
              onClick={handleRetryAllErrors}
              title={t("Requeue all failed files")}
            >
              <Icon name="retry" />
            </button>
          )}

          <button
            type="button"
            className={`btn-compact toolbar-icon ${
              showNewFolderInput ? "row-act-active" : ""
            }`}
            aria-expanded={showNewFolderInput}
            onClick={() => setShowNewFolderInput((prev) => !prev)}
            title={t("Create a subfolder in the current location")}
          >
            <Icon name="folderPlus" /><span className="sr-only">{t("New Folder")}</span>
          </button>

          <button
            type="button"
            className="btn-compact toolbar-icon"
            aria-label={t("Upload files")}
            onClick={() => setUploadPaths([])}
            title={t("Import and upload files to the current folder")}
          >
            <Icon name="upload" />
          </button>

          <button
            type="button"
            className="btn-compact toolbar-icon"
            aria-label={t("Start")}
            onClick={handleSyncNow}
            disabled={syncAction !== null}
            title={t("Force sync now")}
          >
            <Icon name={syncAction === "starting" ? "retry" : "play"} />
          </button>
          <button
            type="button"
            className="btn-compact toolbar-icon"
            aria-label={t("Stop")}
            onClick={handleStopSync}
            disabled={syncAction !== null}
            title={t("Pause the sync engine")}
          >
            <Icon name={syncAction === "stopping" ? "retry" : "stop"} />
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
            placeholder={t("subfolder-name")}
            value={newFolderName}
            onChange={(e) => setNewFolderName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleCreateSubfolder();
              if (e.key === "Escape") setShowNewFolderInput(false);
            }}
            aria-label={t("New folder name")}
          />
          <button
            type="button"
            className="btn-compact btn-primary-accent"
            onClick={handleCreateSubfolder}
          >
            {t("Create Folder")}
          </button>
          <button
            type="button"
            className="btn-compact"
            onClick={() => setShowNewFolderInput(false)}
          >
            {t("Cancel")}
          </button>
        </div>
      )}

      {selectedBatchKeys.length > 0 && (
        <div className="batch-action-bar" role="region" aria-label={t("Batch actions")}>
          <span className="mono batch-count">
            {selectedBatchKeys.length} {t("object(s) selected")}
          </span>
          <div className="batch-btns">
            <button
              type="button"
              className="btn-compact"
              onClick={handleBatchResync}
              title={t("Requeue all selected objects for sync")}
            >
              {t("Batch Resync (")}{selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact"
              onClick={handleBatchCopyLinks}
              title={t("Generate and copy presigned URLs (1 hour) for all selected objects")}
            >
              {t("Copy Links (")}{selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact btn-danger-compact"
              onClick={() => { setDeleteError(false); setPendingDelete([...selectedBatchKeys]); }}
              title={t("Delete selected objects locally and from the S3 bucket")}
            >
              {t("Batch Delete (")}{selectedBatchKeys.length})
            </button>
            <button
              type="button"
              className="btn-compact"
              onClick={() => setSelectedBatchKeys([])}
            >
              {t("Clear Selection")}
            </button>
          </div>
        </div>
      )}

      {queueStats.is_full && (
        <div role="alert" className="banner banner-error">
          {t("The queue has reached its limit of 10,000 operations. New operations were blocked.")}
        </div>
      )}

      {actionFeedback && (
        <div
          role={actionFeedback.type === "error" ? "alert" : "status"}
          className={`banner action-toast ${
            actionFeedback.type === "ok" ? "banner-ok" : "banner-error"
          }`}
        >
          <span className="mono">{actionFeedback.text}</span>
          <button
            type="button"
            className="btn-compact"
            onClick={() => setActionFeedback(null)}
          >
            {t("Close")}
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
          {t("Drop files here to review the upload")}
        </div>
      )}

      {/* Tabela Densa de Arquivos + Painel Inspector Lateral */}
      {fileStatuses.length === 0 ? (
        <div className="empty-table-view">
          <p className="empty-status">{syncAction === "starting" ? t("Connecting and checking files…") : t("No files are currently tracked.")}</p>
          <span className="empty-subtext">
            {syncAction === "starting" ? t("Wait for sync to initialize.") : syncRunning ? t("No files to display. Upload files or add them to the local folder.") : t("Start sync to load files from the local folder and S3.")}
          </span>
        </div>
      ) : (
        <div className="status-split-body">
          <div className="table-viewport">
            <div className="table-head">
              <button
                type="button"
                className="state-sort sortable-col"
                onClick={() => handleSort("state")}
                title={t("Sort by status")}
                aria-label={t("Sort by status")}
              >
                ST {sortField === "state" ? (sortAsc ? "▲" : "▼") : ""}
              </button>
              <span
                className="col-key sortable-col"
                onClick={() => handleSort("path")}
                title={t("Sort by S3 key")}
              >
                <input
                  type="checkbox"
                  className="row-check"
                  checked={allVisibleSelected}
                  onChange={handleToggleAllVisible}
                  onClick={(e) => e.stopPropagation()}
                  aria-label={t("Select all visible files")}
                />
                {t("S3 OBJECT KEY")}{" "}
                {sortField === "path" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span
                className="col-size sortable-col"
                onClick={() => handleSort("size")}
                title={t("Sort by size")}
              >
                {t("SIZE") + " "}{sortField === "size" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span
                className="col-time sortable-col"
                onClick={() => handleSort("time")}
                title={t("Sort by modification date")}
              >
                {t("MODIFIED (UTC)")}{" "}
                {sortField === "time" ? (sortAsc ? "▲" : "▼") : ""}
              </span>
              <span className="col-actions">{t("ACTIONS")}</span>
            </div>

            <ul className="file-status-list" aria-label={t("File list")}>
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
                      <span className="folder-count"> ({folder.count} {folder.count === 1 ? "item" : t("items")})</span>
                    </span>
                    <span className="col-size">
                      {formatBytes(folder.totalBytes)}
                    </span>
                    <span className="col-time">—</span>
                    <span className="col-actions">
                      <button
                        type="button"
                        className="row-act-btn"
                        onClick={(e) => {
                          e.stopPropagation();
                          setCurrentFolder(folder.prefix);
                        }}
                        title={t("Open folder")}
                      >
                        <Icon name="chevronRight" /><span className="sr-only">{t("Open Folder")}</span>
                      </button>
                    </span>
                  </li>
                ))}

              {(viewMode === "tree" ? treeEntries.files : filteredFiles).map(
                (file) => {
                  const meta = renderStateMeta(file.state);
                  const { dir, base } = splitPath(file.path);
                  const prog = file.state.type === "Syncing" ? transferProgress[file.path] : undefined;
                  const stateLabel = prog ? `${meta.label} (${prog.percent}%)` : meta.label;
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
                        role="img"
                        aria-label={stateLabel}
                        title={stateLabel}
                      >
                        {renderStateVectorIcon(file.state)}
                        <span className="sr-only">{meta.srIcon}</span>
                        {prog && (
                          <span className="inline-progress-track">
                            <span
                              className="inline-progress-fill"
                              style={{ width: `${prog.percent}%` }}
                            />
                          </span>
                        )}
                      </span>

                      <span className="col-key mono" title={file.path}>
                        <input
                          type="checkbox"
                          className="row-check"
                          checked={selectedBatchKeys.includes(file.path)}
                          onChange={() => handleToggleBatchKey(file.path)}
                          onClick={(e) => e.stopPropagation()}
                          aria-label={t("Select {0}", file.path)}
                        />
                        {viewMode === "flat" && dir && (
                          <span
                            className="key-prefix key-prefix-clickable"
                            onClick={(e) => {
                              e.stopPropagation();
                              setViewMode("tree");
                              setCurrentFolder(dir);
                            }}
                            title={t("Filter folder {0}", dir)}
                          >
                            {dir}
                          </span>
                        )}
                        <span className="key-name">{base}</span>
                      </span>

                      <span className="col-size">
                        {file.size_bytes > 0
                          ? formatBytes(file.size_bytes)
                          : "—"}
                      </span>

                      <span className="col-time">
                        {file.last_modified
                          ? file.last_modified.replace("T", " ").slice(0, 19)
                          : "—"}
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
                            title={t("Retry sync")}
                          >
                            <Icon name="retry" /><span className="sr-only">Retry</span>
                          </button>
                        )}
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleOpenFile(file.path)}
                          title={t("Open file in the default app")}
                        >
                          <Icon name="open" /><span className="sr-only">{t("Open")}</span>
                        </button>
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleRevealFile(file.path)}
                          title={t("Reveal in Explorer / Finder")}
                        >
                          <Icon name="folder" /><span className="sr-only">{t("Folder")}</span>
                        </button>
                        <button
                          type="button"
                          className="row-act-btn"
                          onClick={() => handleCopyPresignedUrl(file.path)}
                          title={t("Copy temporary S3 link (presigned URL, 1 hour)")}
                        >
                          <Icon name="link" /><span className="sr-only">Link</span>
                        </button>
                        <button
                          type="button"
                          className={`row-act-btn ${
                            isSelected ? "row-act-active" : ""
                          }`}
                          onClick={() =>
                            setSelectedFileKey(isSelected ? null : file.path)
                          }
                          title={t("Inspect object metadata")}
                        >
                          <Icon name="info" /><span className="sr-only">Info</span>
                        </button>
                      </span>
                    </li>
                  );
                }
              )}
            </ul>
          </div>

          {selectedFileKey && (
            <aside className="inspector-drawer" aria-label={t("Object Inspector")}>
              <div className="inspector-header">
                <span className="inspector-title">{t("S3 INSPECTOR")}</span>
                <button
                  type="button"
                  className="row-act-btn"
                  onClick={() => setSelectedFileKey(null)}
                >
                  {t("Close")}
                </button>
              </div>

              <div className="inspector-body">
                <div className="inspector-kv">
                  <span className="inspector-k">{t("S3 KEY")}</span>
                  <span className="inspector-v mono break-all">
                    {selectedFileKey}
                  </span>
                </div>

                {selectedMetadata && (
                  <>
                    <div className="inspector-kv">
                      <span className="inspector-k">{t("LOCAL PATH")}</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.local_path}
                      </span>
                    </div>

                    <div className="inspector-grid-2">
                      <div className="inspector-kv">
                        <span className="inspector-k">{t("SIZE")}</span>
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
                        <span className="inspector-k">{t("ENCRYPTION")}</span>
                        <span className="inspector-v mono">
                          {selectedMetadata.server_side_encryption
                            ? "AES-256 (SSE-S3)"
                            : t("Bucket default")}
                        </span>
                      </div>
                      <div className="inspector-kv">
                        <span className="inspector-k">{t("LOCAL FILE")}</span>
                        <span className="inspector-v mono">
                          {selectedMetadata.local_exists
                            ? t("On disk")
                            : t("Remote only")}
                        </span>
                      </div>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">{t("REMOTE ETAG")}</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.etag || "—"}
                      </span>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">{t("LOCAL MD5 CHECKSUM")}</span>
                      <span className="inspector-v mono break-all">
                        {selectedMetadata.local_md5 || "—"}
                      </span>
                    </div>

                    <div className="inspector-kv">
                      <span className="inspector-k">{t("LAST SYNC (UTC)")}</span>
                      <span className="inspector-v mono">
                        {selectedMetadata.last_sync_time || "—"}
                      </span>
                    </div>
                  </>
                )}

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">
                    {t("SHARE TEMPORARY LINK (PRESIGNED URL)")}
                  </span>
                  <div className="preset-tags">
                    {[
                      { secs: 900, label: "15 min" },
                      { secs: 3600, label: t("1 hour") },
                      { secs: 86400, label: t("24 hours") },
                      { secs: 604800, label: t("7 days") },
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
                    {t("Generate & Copy Presigned URL")}
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
                    {t("S3 VERSION HISTORY (")}{objectVersions.length})
                  </span>
                  {objectVersions.length === 0 ? (
                    <span className="prop-sub">
                      {t("No versions found. S3 Versioning may be disabled, or the object has not been uploaded yet.")}
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
                            title={t("Download this S3 version and restore it to the local file")}
                          >
                            {t("Restore")}
                          </button>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">
                    {t("RENAME / MOVE S3 KEY")}
                  </span>
                  <input
                    type="text"
                    className="mono"
                    value={renameTargetKey}
                    onChange={(e) => setRenameTargetKey(e.target.value)}
                    placeholder={t("new-folder/new-name.ext")}
                    aria-label={t("New S3 key")}
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
                    {t("Apply Rename / Move")}
                  </button>
                </div>

                <div className="inspector-divider" />

                <div className="inspector-kv">
                  <span className="inspector-k">{t("DESTRUCTIVE OPERATIONS")}</span>
                  <button
                    type="button"
                    className="btn-compact btn-danger-compact inspector-full-btn"
                    onClick={() => { setDeleteError(false); setPendingDelete([selectedFileKey]); }}
                    title={t("Delete the local file and queue removal from the S3 bucket")}
                  >
                    {t("Delete Object (Local + S3)")}
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
