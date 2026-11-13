import { t } from "./i18n";

export interface NotificationSettings {
  sync_complete_enabled: boolean;
  conflict_enabled: boolean;
  error_enabled: boolean;
}

export interface AppConfig {
  bucket_name: string;
  local_path: string;
  s3_prefix: string | null;
  aws_region: string;
  sync_interval_secs: number;
  exclusion_patterns: string[];
  notifications: NotificationSettings;
  use_iam_role: boolean;
  endpoint_url?: string | null;
  force_path_style?: boolean;
  max_concurrent_transfers?: number;
  storage_class?: string;
  server_side_encryption?: boolean;
  bandwidth_limit_kbps?: number;
  launch_on_startup?: boolean;
  sync_direction?: "Bidirectional" | "UploadOnly" | "DownloadOnly" | string;
}

export interface AwsCredentials {
  access_key_id: string;
  secret_access_key: string;
  session_token?: string;
  region: string;
  use_iam_role: boolean;
}

export type SyncState =
  | { type: "Pending" }
  | { type: "Syncing" }
  | { type: "Synced" }
  | { type: "Error"; message: string };

export type GlobalSyncState = "UpToDate" | "Syncing" | "Error" | "Offline";

export interface FileStatus {
  path: string;
  state: SyncState;
  last_modified: string | null;
  size_bytes: number;
}

export interface ConflictEvent {
  file_name: string;
  local_path: string;
  conflict_copy_path: string;
  local_modified: string; // "AAAA-MM-DD HH:MM:SS UTC"
  remote_modified: string;
  local_size_bytes: number;
  remote_size_bytes: number;
}

export interface ValidationError {
  field: string;
  reason: string;
}

export interface QueueStats {
  pending_count: number;
  is_full: boolean;
}

export interface FileStatusEvent {
  path: string;
  state: SyncState;
}

export interface SyncErrorEvent {
  file: string;
  error: string;
  retry_count: number;
}

export interface InitialCompleteEvent {
  files_synced: number;
}

export interface TransferProgressEvent {
  path: string;
  operation: "Upload" | "Download";
  bytes_transferred: number;
  total_bytes: number;
  percent: number;
}

export interface ObjectDetailedMetadata {
  s3_key: string;
  local_path: string;
  local_exists: boolean;
  size_bytes: number;
  etag: string | null;
  local_md5: string | null;
  last_sync_time: string | null;
  local_modified_time: string | null;
  storage_class: string;
  server_side_encryption: boolean;
}

export interface S3ObjectVersion {
  version_id: string;
  is_latest: boolean;
  last_modified: string;
  size_bytes: number;
  etag: string;
}

export interface ExtensionStat {
  extension: string;
  count: number;
  bytes: number;
}

export interface StorageAnalytics {
  total_files: number;
  total_bytes: number;
  synced_files: number;
  pending_or_syncing_files: number;
  error_files: number;
  storage_class: string;
  estimated_monthly_usd: number;
  top_extensions: ExtensionStat[];
}

export function validateCredentialsFormat(creds: AwsCredentials): ValidationError[] {
  const errors: ValidationError[] = [];

  if (!creds.use_iam_role) {
    if (!/^[A-Z0-9]{20}$/.test(creds.access_key_id)) {
      errors.push({
        field: "access_key_id",
        reason:
          t("Access Key ID must contain exactly 20 uppercase alphanumeric characters."),
      });
    }

    if (Array.from(creds.secret_access_key).length !== 40) {
      errors.push({
        field: "secret_access_key",
        reason: t("Secret Access Key must contain exactly 40 characters."),
      });
    }
  }

  if (!/^[a-z]{2}-[a-z]+-\d$/.test(creds.region)) {
    errors.push({
      field: "region",
      reason: t("Invalid AWS region (e.g. us-east-1, sa-east-1)."),
    });
  }

  return errors;
}

export function computeFrontendGlobalState(
  fileStatuses: FileStatus[],
  isOnline: boolean
): GlobalSyncState {
  if (!isOnline) {
    return "Offline";
  }
  const hasError = fileStatuses.some((f) => f.state.type === "Error");
  if (hasError) {
    return "Error";
  }
  const hasSyncing = fileStatuses.some(
    (f) => f.state.type === "Syncing" || f.state.type === "Pending"
  );
  if (hasSyncing) {
    return "Syncing";
  }
  return "UpToDate";
}
export interface DeletionReview {
  s3_key: string;
  local_path: string;
  deleted_side: "local" | "remote";
  other_side_changed: boolean;
  remote_etag: string | null;
  queued?: boolean;
  local_hash: string | null;
}
