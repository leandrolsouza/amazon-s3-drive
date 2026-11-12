import { useEffect, useRef } from "react";
import { useSyncStore } from "../store";

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
      <h2 id="delete-title">Excluir {keysToDelete.length} arquivo(s)?</h2>
      <p id="delete-description">Os arquivos serão excluídos da pasta local e a remoção será enfileirada no bucket S3. Esta ação não pode ser desfeita pelo aplicativo.</p>
      <p>Pasta: {config?.local_path || "Pasta configurada"}<br />Destino: {config?.bucket_name ? `s3://${config.bucket_name}/${config.s3_prefix || ""}` : "Bucket configurado"}</p>
      <ul>{keysToDelete.map((key) => <li key={key}>{key}</li>)}</ul>
      {failed && <p role="alert">Não foi possível concluir a exclusão. Alguns arquivos podem já ter sido removidos. Verifique o status antes de tentar novamente.</p>}
      <div className="conflict-actions">
        <button type="button" className="btn-compact" autoFocus disabled={busy} onClick={onCancel}>Cancelar</button>
        <button type="button" className="btn-compact btn-danger-compact" disabled={busy} onClick={onConfirm}>
          {busy ? "Excluindo…" : "Excluir da pasta local e do S3"}
        </button>
      </div>
    </dialog>
  );
}
