import { useEffect, useRef } from "react";
import { useSyncStore } from "../store";
import { t } from "../i18n";

interface Props {
  keysToDelete: string[];
  busy: boolean;
  failed: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

export function DeleteConfirmation({ keysToDelete, busy, failed, onCancel, onConfirm }: Props) {
  const config = useSyncStore((state) => state.config);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const element = dialog.current!;
    if (element.showModal) element.showModal();
    else element.setAttribute("open", "");
    return () => { previousFocus?.focus(); };
  }, []);

  return (
    <dialog ref={dialog} className="delete-confirmation" aria-labelledby="delete-title" aria-describedby="delete-description"
      onCancel={(event) => { event.preventDefault(); if (!busy) onCancel(); }}>
      <h2 id="delete-title">{t("Delete") + " "}{keysToDelete.length} {t("file(s)?")}</h2>
      <p id="delete-description">{t("The files will be deleted from the local folder, and their removal from the S3 bucket will be queued. This action cannot be undone in the app.")}</p>
      <p>{t("Folder:") + " "}{config?.local_path || t("Configured folder")}<br />{t("Destination:") + " "}{config?.bucket_name ? `s3://${config.bucket_name}/${config.s3_prefix || ""}` : t("Configured bucket")}</p>
      <ul>{keysToDelete.map((key) => <li key={key}>{key}</li>)}</ul>
      {failed && <p role="alert">{t("Could not complete deletion. Some files may already have been removed. Check the status before trying again.")}</p>}
      <div className="conflict-actions">
        <button type="button" className="btn-compact" autoFocus disabled={busy} onClick={onCancel}>{t("Cancel")}</button>
        <button type="button" className="btn-compact btn-danger-compact" disabled={busy} onClick={onConfirm}>
          {busy ? t("Deleting…") : t("Delete from local folder and S3")}
        </button>
      </div>
    </dialog>
  );
}
