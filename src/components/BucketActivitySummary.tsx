import { locale, t } from "../i18n";
import type { ConflictEvent, DeletionReview, FileStatus } from "../types";

export function filesNeedingAttention(
  files: FileStatus[], conflicts: ConflictEvent[], reviews: DeletionReview[],
) {
  const keys = new Set(files.filter((file) => file.state.type === "Error").map((file) => file.path));
  for (const conflict of conflicts) {
    keys.add(conflict.s3_key || conflict.local_path);
  }
  for (const review of reviews) {
    keys.add(review.s3_key);
  }
  return keys;
}

export function BucketActivitySummary({
  bucket, prefix, lastSync, activeCount, attentionCount, filter, onFilter,
}: {
  bucket: string;
  prefix: string | null;
  lastSync?: string | null;
  activeCount: number;
  attentionCount: number;
  filter: string;
  onFilter: (filter: "Active" | "Attention") => void;
}) {
  const location = `s3://${bucket}${prefix ? `/${prefix}` : ""}`;
  const timestamp = lastSync ? new Date(lastSync) : null;
  const lastSyncLabel = timestamp && !Number.isNaN(timestamp.getTime())
    ? timestamp.toLocaleString(locale)
    : t(lastSync === null ? "No recorded sync" : "Unavailable");

  return (
    <section className="bucket-activity-summary" aria-label={t("Bucket activity summary")}>
      <div className="bucket-activity-location">
        <strong className="mono truncate" title={location}>{location}</strong>
        <span>{t("Last recorded file sync: {0}", lastSyncLabel)}</span>
      </div>
      <div className="segmented-filter">
        <button type="button" className={`seg-btn ${filter === "Active" ? "seg-active" : ""}`}
          aria-pressed={filter === "Active"} onClick={() => onFilter("Active")}>
          {t("In progress ({0})", activeCount)}
        </button>
        <button type="button" className={`seg-btn ${filter === "Attention" ? "seg-active" : ""}`}
          aria-pressed={filter === "Attention"} onClick={() => onFilter("Attention")}>
          {t("Needs attention ({0})", attentionCount)}
        </button>
      </div>
    </section>
  );
}
