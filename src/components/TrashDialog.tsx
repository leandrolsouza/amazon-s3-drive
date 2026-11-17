import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { locale, t } from "../i18n";
import type { TrashEntry, TrashListing, TrashRestore } from "../types";

function entryId(entry: TrashEntry) {
  return `${entry.source}:${entry.locator}`;
}

export function TrashDialog({ bucket, prefix, bucketId, onClose, onRestored, formatSize }: {
  bucket: string;
  prefix: string | null;
  bucketId?: number;
  onClose: () => void;
  onRestored?: () => void;
  formatSize: (bytes: number) => string;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [listing, setListing] = useState<TrashListing | null>(null);
  const [loadError, setLoadError] = useState("");
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [legacyPaths, setLegacyPaths] = useState<Record<string, string>>({});
  const [feedback, setFeedback] = useState<{ id: string; warning: boolean; message: string }>();

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const element = dialog.current;
    if (element?.showModal) element.showModal();
    else element?.setAttribute("open", "");
    return () => previousFocus?.focus();
  }, []);

  useEffect(() => {
    let disposed = false;
    setLoading(true);
    setLoadError("");
    invoke<TrashListing>("list_trash", { bucketId })
      .then((result) => { if (!disposed) setListing(result); })
      .catch((error) => { if (!disposed) setLoadError(String(error)); })
      .finally(() => { if (!disposed) setLoading(false); });
    return () => { disposed = true; };
  }, [bucketId, revision]);

  async function restore(entry: TrashEntry) {
    const id = entryId(entry);
    setBusy(id);
    setFeedback(undefined);
    try {
      const result = await invoke<TrashRestore>("restore_trash", {
        bucketId, source: entry.source, locator: entry.locator,
        legacyPath: entry.original_key ? undefined : legacyPaths[id]?.trim(),
      });
      if (result.status === "blocked") {
        const conflict = result.local && result.remote
          ? "A file already exists at the destination on this computer and in S3."
          : result.local ? "A file already exists at the destination on this computer."
            : "A file already exists at the destination in S3.";
        setFeedback({ id, warning: true, message: t(conflict) });
        return;
      }
      setFeedback({ id, warning: result.backup_retained, message: t(result.backup_retained
        ? "File restored. The backup was kept in the trash." : "File restored to the local folder and S3.") });
      if (!result.backup_retained) {
        setListing((current) => current && { ...current, entries: current.entries.filter((item) => entryId(item) !== id) });
      }
      onRestored?.();
    } catch (error) {
      setFeedback({ id, warning: true, message: t("Could not restore the file. {0}", String(error)) });
    } finally {
      setBusy(null);
    }
  }

  const location = `s3://${bucket}${prefix ? `/${prefix}` : ""}`;
  return createPortal(
    <dialog ref={dialog} className="delete-confirmation trash-dialog" aria-labelledby="trash-title"
      onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}>
      <header className="trash-heading">
        <div>
          <h2 id="trash-title">{t("Trash")}</h2>
          <span className="mono">{location}</span>
        </div>
        <button type="button" className="btn-compact" autoFocus disabled={!!busy} onClick={onClose}>{t("Close")}</button>
      </header>
      {loading && <p role="status">{t("Loading trash…")}</p>}
      {loadError && <p role="alert">{t("Could not load the trash. {0}", loadError)}</p>}
      {listing?.local_error && <p role="alert">{t("Local trash unavailable. {0}", listing.local_error)}</p>}
      {listing?.remote_error && <p role="alert">{t("S3 trash unavailable. {0}", listing.remote_error)}</p>}
      {listing?.entries.length === 0 && !loading && !loadError && !listing.local_error && !listing.remote_error &&
        <p>{t("The trash is empty.")}</p>}
      {feedback && !listing?.entries.some((entry) => entryId(entry) === feedback.id) && (
        <p role={feedback.warning ? "alert" : "status"}>{feedback.message}</p>
      )}
      {!!listing?.entries.length && <ul className="trash-list">{listing.entries.map((entry) => {
        const id = entryId(entry);
        return (
          <li key={id} className="trash-row">
            <div className="trash-file">
              <strong className="mono">{entry.original_key ?? entry.file_name}</strong>
              <span>
                {t(entry.source === "local" ? "Local copy" : "S3 copy")} ·{" "}
                {new Date(entry.deleted_at).toLocaleString(locale)} · {formatSize(entry.size_bytes)}
              </span>
              {!entry.original_key && (
                <label>
                  {t("Original path not recorded. Enter the destination path inside the sync folder.")}
                  <input type="text" placeholder={entry.file_name} value={legacyPaths[id] ?? ""} disabled={!!busy}
                    onChange={(event) => setLegacyPaths((current) => ({ ...current, [id]: event.target.value }))} />
                </label>
              )}
              {feedback?.id === id && <span role={feedback.warning ? "alert" : "status"}>{feedback.message}</span>}
            </div>
            <button type="button" className="btn-compact"
              disabled={loading || !!busy || (!entry.original_key && !legacyPaths[id]?.trim())}
              onClick={() => void restore(entry)}>
              {t(busy === id ? "Restoring…" : "Restore")}
            </button>
          </li>
        );
      })}</ul>}
      <button type="button" className="btn-compact" disabled={loading || !!busy} onClick={() => setRevision((value) => value + 1)}>
        {t("Refresh")}
      </button>
    </dialog>,
    document.body,
  );
}
