import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DeletionReview } from "../types";

export function DeletionReviewPanel({ bucketId, paused = false }: { bucketId?: number; paused?: boolean } = {}) {
  const [reviews, setReviews] = useState<DeletionReview[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let mounted = true;
    const refresh = async () => {
      try {
        const pending = bucketId === undefined
          ? await invoke<DeletionReview[]>("get_deletion_reviews")
          : await invoke<DeletionReview[]>("get_bucket_deletions", { id: bucketId });
        if (mounted && Array.isArray(pending)) setReviews(pending);
      } catch { /* O próximo ciclo tenta novamente. */ }
    };
    void refresh();
    const timer = setInterval(refresh, 2000);
    return () => { mounted = false; clearInterval(timer); };
  }, [bucketId]);
  const resolve = async (items: DeletionReview[], restore: boolean) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      for (const item of items) {
        if (bucketId === undefined) {
          await invoke("resolve_deletion", { review: item, restore });
        } else {
          await invoke("resolve_bucket_deletion", { id: bucketId, review: item, restore });
        }
        setReviews((current) => current.filter((r) => r.s3_key !== item.s3_key));
      }
    } catch (cause) {
      setError(`Não foi possível concluir. ${String(cause)}`);
    } finally { setBusy(false); }
  };
  if (!reviews.length) return null;
  return <div className="notification-card notification-warning deletion-review-panel" role="region" aria-label="Revisar exclusões">
    <strong>{reviews.length} exclusão(ões) aguardando sua decisão</strong>
    <p>Os arquivos não serão removidos nem restaurados automaticamente. Antes de excluir, uma cópia será preservada em .s3-drive-trash no destino da exclusão.</p>
    {paused && <p>Inicie a sincronização deste bucket para concluir a revisão.</p>}
    {error && <p role="alert">{error}</p>}
    <ul>{reviews.map((item) => <li key={item.s3_key}>
      <p>{item.s3_key} — excluído {item.deleted_side === "local" ? "no computador" : "no S3"}.
        {item.queued ? " Confirme a revisão remota antes de excluir." : item.other_side_changed && " Conflito: a outra versão foi editada."}</p>
      <button disabled={busy || paused} onClick={() => void resolve([item], true)}>Restaurar arquivo</button>{" "}
      <button disabled={busy || paused} onClick={() => void resolve([item], false)}>Confirmar exclusão</button>
    </li>)}</ul>
    {reviews.length > 1 && <div className="conflict-actions">
      <button disabled={busy || paused} onClick={() => void resolve(reviews, true)}>Restaurar todos</button>
      <button disabled={busy || paused || reviews.every((r) => r.other_side_changed)} onClick={() => void resolve(reviews.filter((r) => !r.other_side_changed), false)}>
        Confirmar exclusões sem conflito
      </button>
    </div>}
  </div>;
}
