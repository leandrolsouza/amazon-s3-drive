import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { DeletionReview } from "../types";
import { t } from "../i18n";

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
      setError(t("Could not complete the operation. {0}", String(cause)));
    } finally { setBusy(false); }
  };
  if (!reviews.length) return null;
  return <div className="notification-card notification-warning deletion-review-panel" role="region" aria-label={t("Review deletions")}>
    <strong>{reviews.length} {t("deletion(s) awaiting review")}</strong>
    <p>{t("Files will not be removed or restored automatically. Before deletion, a copy will be saved in .s3-drive-trash on the side where deletion takes place.")}</p>
    {paused && <p>{t("Start syncing this bucket to complete the review.")}</p>}
    {error && <p role="alert">{error}</p>}
    <ul>{reviews.map((item) => <li key={item.s3_key}>
      <p>{item.s3_key} {t("— deleted") + " "}{item.deleted_side === "local" ? t("on this computer") : t("in S3")}.
        {item.queued ? t(" Confirm the remote revision before deleting.") : item.other_side_changed && t(" Conflict: the other version was edited.")}</p>
      <button disabled={busy || paused} onClick={() => void resolve([item], true)}>{t("Restore file")}</button>{" "}
      <button disabled={busy || paused} onClick={() => void resolve([item], false)}>{t("Confirm deletion")}</button>
    </li>)}</ul>
    {reviews.length > 1 && <div className="conflict-actions">
      <button disabled={busy || paused} onClick={() => void resolve(reviews, true)}>{t("Restore all")}</button>
      <button disabled={busy || paused || reviews.every((r) => r.other_side_changed)} onClick={() => void resolve(reviews.filter((r) => !r.other_side_changed), false)}>
        {t("Confirm deletions without conflicts")}
      </button>
    </div>}
  </div>;
}
