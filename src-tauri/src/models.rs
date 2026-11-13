use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_max_concurrent_transfers() -> usize {
    4
}

fn default_storage_class() -> String {
    "STANDARD".to_string()
}

fn default_sync_direction() -> String {
    "Bidirectional".to_string()
}

// === Configuração da Aplicação ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub bucket_name: String,
    pub local_path: String,
    pub s3_prefix: Option<String>,
    pub aws_region: String,
    pub sync_interval_secs: u64,
    pub exclusion_patterns: Vec<String>,
    pub notifications: NotificationSettings,
    pub use_iam_role: bool,
    #[serde(default)]
    pub endpoint_url: Option<String>,
    #[serde(default)]
    pub force_path_style: bool,
    #[serde(default = "default_max_concurrent_transfers")]
    pub max_concurrent_transfers: usize,
    #[serde(default = "default_storage_class")]
    pub storage_class: String,
    #[serde(default)]
    pub server_side_encryption: bool,
    #[serde(default)]
    pub bandwidth_limit_kbps: u64,
    #[serde(default)]
    pub launch_on_startup: bool,
    #[serde(default = "default_sync_direction")]
    pub sync_direction: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            bucket_name: String::new(),
            local_path: String::new(),
            s3_prefix: None,
            aws_region: String::new(),
            sync_interval_secs: 60,
            exclusion_patterns: Vec::new(),
            notifications: NotificationSettings::default(),
            use_iam_role: false,
            endpoint_url: None,
            force_path_style: false,
            max_concurrent_transfers: 4,
            storage_class: default_storage_class(),
            server_side_encryption: false,
            bandwidth_limit_kbps: 0,
            launch_on_startup: false,
            sync_direction: default_sync_direction(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationSettings {
    pub sync_complete_enabled: bool,
    pub conflict_enabled: bool,
    pub error_enabled: bool, // sempre ativo conforme Requisito 10.4
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            sync_complete_enabled: true,
            conflict_enabled: true,
            error_enabled: true,
        }
    }
}

// === Credenciais AWS (jamais serializado para arquivo em disco; apenas para OS keyring e Tauri IPC) ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub region: String,
    pub use_iam_role: bool,
}

// === Estado de Sincronização por Arquivo ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "message")]
pub enum SyncState {
    Pending,
    Syncing,
    Synced,
    Error(String),
}

// === Registro de Arquivo (fonte da verdade para detecção de conflitos) ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub s3_key: String,
    pub local_path: String,
    pub sync_state: SyncState,
    /// ETag retornado pelo S3 na última sincronização bem-sucedida.
    /// Usado exclusivamente para detectar mudanças no lado remoto.
    pub last_sync_etag: Option<String>,
    /// MD5 do arquivo local na última sincronização bem-sucedida.
    /// Separado de `last_sync_etag` para que a detecção de mudanças locais não seja
    /// afetada por ETags de uploads multipart (formato `hash-N`) ou objetos com SSE-KMS.
    #[serde(default)]
    pub local_hash: Option<String>,
    pub last_sync_time: Option<DateTime<Utc>>,
    pub local_modified_time: Option<DateTime<Utc>>,
    pub size_bytes: u64,
}

// === Metadados de Objeto S3 ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct S3ObjectInfo {
    pub key: String,
    pub etag: String, // sem aspas, normalizado
    pub size: u64,
    pub last_modified: DateTime<Utc>,
}

// === Operação de Sincronização ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SyncOperationType {
    Upload,
    Download,
    Delete,
    Rename,
    Move,
}

impl SyncOperationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncOperationType::Upload => "Upload",
            SyncOperationType::Download => "Download",
            SyncOperationType::Delete => "Delete",
            SyncOperationType::Rename => "Rename",
            SyncOperationType::Move => "Move",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Upload" => Some(SyncOperationType::Upload),
            "Download" => Some(SyncOperationType::Download),
            "Delete" => Some(SyncOperationType::Delete),
            "Rename" => Some(SyncOperationType::Rename),
            "Move" => Some(SyncOperationType::Move),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncOperation {
    pub operation_type: SyncOperationType,
    pub s3_key: String,
    pub local_path: String,
    pub extra: Option<String>, // destino para Rename/Move ou ETag esperado para Delete
}

// === Item da Fila Persistida ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueueItem {
    pub id: i64,
    pub operation: SyncOperation,
    pub created_at: DateTime<Utc>,
    pub retry_count: u32,
    pub error_message: Option<String>,
}

// === Informações de Conflito ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeletionReview {
    pub s3_key: String,
    pub local_path: String,
    pub deleted_side: String,
    pub other_side_changed: bool,
    pub remote_etag: Option<String>,
    #[serde(default)]
    pub queued: bool,
    pub local_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConflictInfo {
    pub file_name: String,
    pub local_path: String,
    pub conflict_copy_path: String,
    pub local_modified: DateTime<Utc>,
    pub remote_modified: DateTime<Utc>,
    pub local_size_bytes: u64,
    pub remote_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConflictEvent {
    pub file_name: String,
    pub local_path: String,
    pub conflict_copy_path: String,
    pub local_modified: String, // "AAAA-MM-DD HH:MM:SS UTC"
    pub remote_modified: String,
    pub local_size_bytes: u64,
    pub remote_size_bytes: u64,
}

impl From<&ConflictInfo> for ConflictEvent {
    fn from(info: &ConflictInfo) -> Self {
        Self {
            file_name: info.file_name.clone(),
            local_path: info.local_path.clone(),
            conflict_copy_path: info.conflict_copy_path.clone(),
            local_modified: info
                .local_modified
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
            remote_modified: info
                .remote_modified
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
            local_size_bytes: info.local_size_bytes,
            remote_size_bytes: info.remote_size_bytes,
        }
    }
}

// === Estado de Retomada de Multipart Upload ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MultipartState {
    pub s3_key: String,
    pub upload_id: String,
    pub local_path: String,
    pub created_at: DateTime<Utc>,
    pub completed_parts: Vec<CompletedPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletedPart {
    pub part_number: i32,
    pub etag: String,
    pub byte_offset: u64,
    pub size: u64,
}

/// Intervalo de bytes para uma parte de multipart upload
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartRange {
    pub part_number: i32,
    pub offset: u64,
    pub size: u64,
}

// === Estado Global de Sincronização ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum GlobalSyncState {
    UpToDate,
    Syncing,
    Error,
    Offline,
}

// === Tipos Auxiliares de Resposta / Eventos ===
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationError {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStatus {
    pub path: String,
    pub state: SyncState,
    pub last_modified: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueStats {
    pub pending_count: usize,
    pub is_full: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStatusEvent {
    pub path: String,
    pub state: SyncState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncErrorEvent {
    pub file: String,
    pub error: String,
    pub retry_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitialCompleteEvent {
    pub files_synced: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferProgressEvent {
    pub path: String,
    pub operation: SyncOperationType,
    pub bytes_transferred: u64,
    pub total_bytes: u64,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectDetailedMetadata {
    pub s3_key: String,
    pub local_path: String,
    pub local_exists: bool,
    pub size_bytes: u64,
    pub etag: Option<String>,
    pub local_md5: Option<String>,
    pub last_sync_time: Option<String>,
    pub local_modified_time: Option<String>,
    pub storage_class: String,
    pub server_side_encryption: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct S3ObjectVersion {
    pub version_id: String,
    pub is_latest: bool,
    pub last_modified: String,
    pub size_bytes: u64,
    pub etag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionStat {
    pub extension: String,
    pub count: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StorageAnalytics {
    pub total_files: usize,
    pub total_bytes: u64,
    pub synced_files: usize,
    pub pending_or_syncing_files: usize,
    pub error_files: usize,
    pub storage_class: String,
    pub estimated_monthly_usd: f64,
    pub top_extensions: Vec<ExtensionStat>,
}

// === Eventos do Sistema de Arquivos Local ===
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalFsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

// === Erros de Domínio ===
#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum SyncError {
    #[error("S3 error: {0}")]
    S3(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Queue full — cannot enqueue operation")]
    QueueFull,
    #[error("Checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("Max retries exceeded for {file}: {reason}")]
    MaxRetries { file: String, reason: String },
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Credentials error: {0}")]
    Credentials(String),
    #[error("Conflict error: {0}")]
    Conflict(String),
    #[error("Watch error: {0}")]
    Watch(String),
    #[error("Queue error: {0}")]
    Queue(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum ConfigError {
    #[error("IO error reading/writing config: {0}")]
    Io(String),
    #[error("Malformed JSON in config file: {0}")]
    MalformedJson(String),
    #[error("Config file contains non-UTF-8 bytes: {0}")]
    InvalidUtf8(String),
    #[error("Invalid configuration: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum CredError {
    #[error("Keyring error: {0}")]
    Keyring(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Invalid credentials format: {0}")]
    InvalidFormat(String),
    #[error("S3 bucket access verification failed: {0}")]
    AccessDenied(String),
    #[error("Credentials expired or revoked")]
    Expired,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum QueueError {
    #[error("SQLite database error: {0}")]
    Database(String),
    #[error("Invalid queue data: {0}")]
    InvalidData(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum WatchError {
    #[error("File watcher error: {0}")]
    Notify(String),
    #[error("Invalid watch path: {0}")]
    InvalidPath(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum ConflictError {
    #[error("Cannot save conflict copy for '{file}': {reason}")]
    CannotSaveConflictCopy { file: String, reason: String },
    #[error("S3 download failed during conflict resolution: {0}")]
    S3Error(String),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error, Serialize, Deserialize)]
pub enum S3Error {
    #[error("Objeto S3 inexistente")]
    ObjectNotFound,
    #[error("{}", crate::language::tr("The S3 object revision changed", "A revisão do objeto S3 mudou"))]
    PreconditionFailed,
    #[error("AWS S3 SDK error: {0}")]
    Sdk(String),
    #[error("IO error during S3 operation: {0}")]
    Io(String),
    #[error("Checksum mismatch for '{file}': expected {expected}, received {received}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        received: String,
    },
    #[error("Authentication error: {0}")]
    Auth(String),
    #[error("Multipart upload expired or invalid: {0}")]
    MultipartInvalid(String),
}

impl From<S3Error> for SyncError {
    fn from(e: S3Error) -> Self {
        match e {
            S3Error::ChecksumMismatch {
                file,
                expected,
                received,
            } => SyncError::ChecksumMismatch {
                file,
                expected,
                actual: received,
            },
            other => SyncError::S3(other.to_string()),
        }
    }
}

impl From<QueueError> for SyncError {
    fn from(e: QueueError) -> Self {
        SyncError::Queue(e.to_string())
    }
}

impl From<ConfigError> for SyncError {
    fn from(e: ConfigError) -> Self {
        SyncError::Config(e.to_string())
    }
}

impl From<CredError> for SyncError {
    fn from(e: CredError) -> Self {
        SyncError::Credentials(e.to_string())
    }
}

impl From<WatchError> for SyncError {
    fn from(e: WatchError) -> Self {
        SyncError::Watch(e.to_string())
    }
}

impl From<ConflictError> for SyncError {
    fn from(e: ConflictError) -> Self {
        SyncError::Conflict(e.to_string())
    }
}
