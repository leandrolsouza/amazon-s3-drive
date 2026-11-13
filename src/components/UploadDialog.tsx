import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSyncStore } from "../store";
import { t, locale } from "../i18n";

const NEW_FOLDER = "/";

interface Props {
  currentFolder: string;
  initialPaths: string[];
  onCancel: () => void;
  onImported: (count: number) => void;
}

export function UploadDialog({ currentFolder, initialPaths, onCancel, onImported }: Props) {
  const config = useSyncStore((state) => state.config);
  const files = useSyncStore((state) => state.fileStatuses);
  const prefix = config?.s3_prefix?.trim().replace(/^\/+|\/+$/g, "") ?? "";
  const relativeFolder = (folder: string) => {
    const normalized = folder.replace(/\/+$/, "");
    if (prefix && normalized === prefix) return "";
    return prefix && normalized.startsWith(prefix + "/") ? normalized.slice(prefix.length + 1) : normalized;
  };
  const [folder, setFolder] = useState(() => relativeFolder(currentFolder));
  const [newFolder, setNewFolder] = useState("");
  const [paths, setPaths] = useState(() => [...new Set(initialPaths)]);
  const [pending, setPending] = useState<"selecting" | "importing" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const element = dialog.current!;
    if (element.showModal) element.showModal();
    else element.setAttribute("open", "");
    return () => {
      if (element.close) element.close();
      else element.removeAttribute("open");
      previousFocus?.focus();
    };
  }, []);

  const folders = new Set([relativeFolder(currentFolder), folder]);
  for (const { path } of files) {
    const parts = relativeFolder(path).split("/");
    for (let index = 1; index < parts.length; index++) {
      folders.add(parts.slice(0, index).join("/"));
    }
  }
  folders.delete("");
  folders.delete(NEW_FOLDER);
  const creatingFolder = folder === NEW_FOLDER;
  const targetFolder = creatingFolder ? newFolder.trim() : folder;
  const validFolder = !creatingFolder || targetFolder.split("/").every((part) =>
    part.length > 0 && part === part.trim() && !part.endsWith(".") && !/[<>:"\\|?*]/.test(part),
  );
  const destination = [prefix, targetFolder].filter(Boolean).join("/");
  const canUpload = Boolean(config?.bucket_name.trim() && config.local_path.trim()) &&
    config?.sync_direction?.toLowerCase() !== "downloadonly";

  async function chooseFiles() {
    setPending("selecting");
    setError(null);
    try {
      const selected = await invoke<string[]>("open_files_dialog");
      if (selected?.length) setPaths((previous) => [...new Set([...previous, ...selected])]);
    } catch {
      setError(t("Could not open the file picker."));
    } finally {
      setPending(null);
    }
  }

  async function upload() {
    if (pending || !canUpload || !validFolder || paths.length === 0) return;
    setPending("importing");
    setError(null);
    try {
      const count = await invoke<number>("import_files_to_drive", {
        paths,
        targetSubfolder: targetFolder || null,
      });
      if (count > 0) onImported(count);
      else setError(t("No files were imported. Check that the files are still available."));
    } catch {
      setError(t("Could not finish importing. Check the object list before trying again."));
    } finally {
      setPending(null);
    }
  }

  return (
    <dialog ref={dialog} className="delete-confirmation upload-dialog" aria-labelledby="upload-title"
      onCancel={(event) => { event.preventDefault(); if (!pending) onCancel(); }}>
      <h2 id="upload-title">{t("Upload files")}</h2>
      <div className="upload-fields">
        <label htmlFor="upload-folder">{t("Destination folder")}</label>
        <select id="upload-folder" value={folder} disabled={Boolean(pending)} onChange={(event) => setFolder(event.target.value)}>
          <option value="">{t("Sync root")}</option>
          {[...folders].sort((a, b) => a.localeCompare(b, locale)).map((name) => <option key={name}>{name}</option>)}
          <option value={NEW_FOLDER}>{t("New folder…")}</option>
        </select>
        {creatingFolder && (
          <>
            <label htmlFor="upload-new-folder">{t("New folder path")}</label>
            <input id="upload-new-folder" value={newFolder} placeholder={t("E.g. documents/2026")}
              disabled={Boolean(pending)} onChange={(event) => setNewFolder(event.target.value)} />
            {targetFolder && !validFolder && <p role="alert" className="field-error">{t("Enter a valid relative path, using / between folders.")}</p>}
          </>
        )}
        {config?.bucket_name && validFolder && <small className="upload-destination">s3://{config.bucket_name}/{destination}</small>}
        <div>{t("Storage class") + " "}<strong>{config?.storage_class || "STANDARD"}</strong> <small>{t("· set in Settings")}</small></div>
        <button type="button" className="btn-compact" autoFocus disabled={Boolean(pending)} onClick={chooseFiles}>
          {paths.length ? t("Add files") : t("Select files")}
        </button>
        {paths.length > 0 && (
          <ul aria-label={t("Selected files")}>
            {paths.map((path) => {
              const name = path.split(/[\\/]/).pop() || path;
              return <li key={path}><span>{name}</span><button type="button" className="row-act-btn" disabled={Boolean(pending)}
                aria-label={t("Remove {0}", name)} onClick={() => setPaths((previous) => previous.filter((entry) => entry !== path))}>{t("Remove")}</button></li>;
            })}
          </ul>
        )}
        {!canUpload && <p role="status">{t("Configure a destination with uploads enabled to continue.")}</p>}
        {error && <p role="alert" className="field-error">{error}</p>}
      </div>
      <div className="upload-actions">
        <button type="button" className="btn-compact" disabled={Boolean(pending)} onClick={onCancel}>{t("Cancel")}</button>
        <button type="button" className="btn-primary-compact" disabled={!canUpload || !validFolder || !paths.length || Boolean(pending)} onClick={upload}>
          {pending === "importing" ? t("Importing…") : t("Confirm upload")}
        </button>
      </div>
    </dialog>
  );
}
