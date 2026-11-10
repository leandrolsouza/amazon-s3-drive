use crate::conflict_resolver::ConflictResolver;
use crate::db::Database;
use crate::file_watcher::FileWatcher;
use crate::models::{
    AppConfig, ConflictEvent, ExtensionStat, FileEntry, FileStatusEvent, InitialCompleteEvent,
    LocalFsEvent, ObjectDetailedMetadata, S3Error, S3ObjectVersion, StorageAnalytics, SyncError,
    SyncErrorEvent, SyncOperation, SyncOperationType, SyncState, TransferProgressEvent,
};
use crate::network_detector::NetworkDetector;
use crate::queue_manager::QueueManager;
use crate::s3_client::{compute_md5, ensure_dir_exists, S3Client};
use crate::status_monitor::StatusMonitor;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex, RwLock,
};
use std::time::Duration;
use tokio::sync::mpsc;

pub trait EventEmitter: Send + Sync {
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

#[derive(Default, Clone)]
pub struct RecordingEmitter {
    pub events: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

impl EventEmitter for RecordingEmitter {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_string(), payload));
    }
}

pub struct TauriEmitter {
    pub app_handle: tauri::AppHandle,
}

impl EventEmitter for TauriEmitter {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(event, payload);
    }
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub backoff_factor: f64,
    pub jitter_range_ms: u64,
    pub download_fixed_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 1_000,
            backoff_factor: 2.0,
            jitter_range_ms: 500,
            download_fixed_delay_ms: 10_000,
        }
    }
}

impl RetryPolicy {
    pub fn fast_for_tests() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 5,
            backoff_factor: 2.0,
            jitter_range_ms: 2,
            download_fixed_delay_ms: 5,
        }
    }

    pub fn delay_for_attempt(&self, attempt: u32, is_download: bool) -> Duration {
        if is_download {
            Duration::from_millis(self.download_fixed_delay_ms)
        } else {
            let exp = (self.base_delay_ms as f64)
                * self.backoff_factor.powi(attempt.saturating_sub(1) as i32);
            Duration::from_millis(exp as u64)
        }
    }
}

/// Substitui "/" pelo separador do SO (Requisito 1.4)
pub fn s3_key_to_local_path(key: &str, base: &Path) -> PathBuf {
    let normalized = normalize_key(key);
    let mut result = base.to_path_buf();
    for part in normalized.split('/').filter(|s| !s.is_empty()) {
        result.push(part);
    }
    result
}

/// Substitui separador do SO por "/" e normaliza separadores consecutivos (Requisito 1.4)
pub fn local_path_to_s3_key(path: &Path, base: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    let components: Vec<String> = relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(os_str) => Some(os_str.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    normalize_key(&components.join("/"))
}

pub fn normalize_key(key: &str) -> String {
    key.replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// Aplica o prefixo S3 configurado a uma chave relativa.
/// Centraliza a conversão em todos os tipos de operação local (upload, exclusão e renomeação),
/// garantindo que nenhum evento produza uma chave sem o prefixo correto.
pub fn apply_s3_prefix(key: String, prefix: &Option<String>) -> String {
    if let Some(ref p) = prefix {
        let clean = p.trim_matches('/');
        if !clean.is_empty() && !key.starts_with(clean) {
            return format!("{}/{}", clean, key);
        }
    }
    key
}

/// Gera um identificador de identidade de sincronização a partir dos campos que definem
/// o destino e a origem dos arquivos. Usado para detectar troca de configuração entre sessões.
pub fn config_fingerprint(config: &AppConfig) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        config.bucket_name.trim(),
        config.local_path.trim(),
        config.s3_prefix.as_deref().unwrap_or("").trim_matches('/'),
        config.aws_region.trim(),
        config.endpoint_url.as_deref().unwrap_or("").trim_matches('/')
    )
}

/// Verifica se o arquivo corresponde a algum padrão glob de exclusão (Requisito 9.3, 9.4, 9.8)
pub fn is_excluded(path: &Path, patterns: &[String]) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| path.to_str().unwrap_or(""));

    if file_name.is_empty() {
        return false;
    }

    patterns.iter().any(|p| {
        glob::Pattern::new(p)
            .map(|pat| pat.matches(file_name))
            .unwrap_or(false)
    })
}

pub struct SyncEngine {
    config: Arc<RwLock<AppConfig>>,
    s3_client: Arc<RwLock<Option<S3Client>>>,
    db: Database,
    queue_manager: Arc<QueueManager>,
    status_monitor: Arc<StatusMonitor>,
    network_detector: NetworkDetector,
    emitter: Arc<dyn EventEmitter>,
    running: Arc<AtomicBool>,
    initial_sync_done: Arc<AtomicBool>,
    active_transfers: Arc<AtomicUsize>,
    retry_policy: RetryPolicy,
    watcher: Arc<Mutex<Option<FileWatcher>>>,
}

impl SyncEngine {
    pub fn new(
        config: AppConfig,
        s3_client: Option<S3Client>,
        db: Database,
        queue_manager: Arc<QueueManager>,
        status_monitor: Arc<StatusMonitor>,
        network_detector: NetworkDetector,
        emitter: Arc<dyn EventEmitter>,
    ) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            s3_client: Arc::new(RwLock::new(s3_client)),
            db,
            queue_manager,
            status_monitor,
            network_detector,
            emitter,
            running: Arc::new(AtomicBool::new(false)),
            initial_sync_done: Arc::new(AtomicBool::new(false)),
            active_transfers: Arc::new(AtomicUsize::new(0)),
            retry_policy: RetryPolicy::default(),
            watcher: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn set_s3_client(&self, client: S3Client) {
        *self.s3_client.write().unwrap() = Some(client);
    }

    pub fn reload_config(&self, new_config: AppConfig) {
        let old_fp = config_fingerprint(&self.config.read().unwrap());
        let new_fp = config_fingerprint(&new_config);
        *self.config.write().unwrap() = new_config;
        if old_fp != new_fp {
            // A identidade de sincronização mudou: descartar dados obsoletos do banco e
            // do monitor de estado para evitar que operações antigas afetem o novo destino.
            if let Err(e) = self.db.clear_all_sync_data() {
                eprintln!("Aviso: falha ao limpar dados de sincronização obsoletos: {e}");
            }
            self.status_monitor.clear_all();
        }
    }

    pub fn get_config(&self) -> AppConfig {
        self.config.read().unwrap().clone()
    }

    pub fn status_monitor(&self) -> Arc<StatusMonitor> {
        Arc::clone(&self.status_monitor)
    }

    pub fn queue_manager(&self) -> Arc<QueueManager> {
        Arc::clone(&self.queue_manager)
    }

    pub fn active_transfers_count(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.active_transfers)
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        if let Ok(val) = serde_json::to_value(payload) {
            self.emitter.emit_json(event, val);
        }
    }

    fn emit_void(&self, event: &str) {
        self.emitter.emit_json(event, serde_json::Value::Null);
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), SyncError> {
        let cfg = self.get_config();
        if cfg.local_path.trim().is_empty() {
            return Err(SyncError::Config(
                "Caminho da pasta local não configurado.".to_string(),
            ));
        }

        let local_dir = PathBuf::from(&cfg.local_path);
        // Requisito 1.1 e 1.6: criar Pasta_Local se não existir; falhar se inválida/inacessível
        fs::create_dir_all(&local_dir).map_err(|e| {
            SyncError::Io(format!(
                "Não foi possível criar ou acessar a pasta local '{}': {}",
                local_dir.display(),
                e
            ))
        })?;

        // Requisito 1.7 e 4.6: testar acesso ao bucket antes de iniciar
        let client_opt = self.s3_client.read().unwrap().clone();
        if let Some(client) = client_opt {
            client.test_access().await.map_err(|e| {
                self.emit_void("credentials:invalid");
                SyncError::Credentials(format!(
                    "Falha ao acessar o bucket S3 '{}': {}",
                    cfg.bucket_name, e
                ))
            })?;
        } else {
            return Err(SyncError::Credentials(
                "Cliente S3 / credenciais não configurados.".to_string(),
            ));
        }

        // Verificar se a identidade de configuração mudou desde a última sessão.
        // Se mudou, descartar dados obsoletos antes de iniciar a sincronização
        // (cobre o cenário de cold-start com banco de dados de uma sessão anterior).
        let current_fp = config_fingerprint(&cfg);
        let stored_fp = self.db.get_config_fingerprint().ok().flatten();
        if stored_fp.as_deref() != Some(current_fp.as_str()) {
            let _ = self.db.clear_all_sync_data();
            let _ = self.db.set_config_fingerprint(&current_fp);
            self.status_monitor.clear_all();
        }

        self.running.store(true, Ordering::SeqCst);

        // Configurar callback de conectividade para emitir eventos e drenar fila ao reconectar (Requisito 8.3)
        let engine_for_net = Arc::clone(self);
        self.network_detector.on_state_change(move |online| {
            if online {
                engine_for_net.emit_void("network:online");
                let eng = Arc::clone(&engine_for_net);
                tokio::spawn(async move {
                    let _ = eng.poll_remote_changes().await;
                    let _ = eng.process_queue().await;
                });
            } else {
                engine_for_net.emit_void("network:offline");
            }
        });

        // Iniciar FileWatcher
        let (fs_tx, mut fs_rx) = mpsc::channel::<LocalFsEvent>(256);
        let mut fw = FileWatcher::new(local_dir, fs_tx)?;
        fw.start()?;
        *self.watcher.lock().unwrap() = Some(fw);

        // Primeira varredura inicial: remota + local (Requisito 1.2, 10.1)
        self.poll_remote_changes().await?;
        self.process_queue().await?;
        self.scan_local_directory().await?;
        self.process_queue().await?;

        // Task de eventos locais
        let engine_for_fs = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(ev) = fs_rx.recv().await {
                if !engine_for_fs.running.load(Ordering::SeqCst) {
                    break;
                }
                let _ = engine_for_fs.process_local_event(ev).await;
                if engine_for_fs.network_detector.is_online() {
                    let _ = engine_for_fs.process_queue().await;
                }
            }
        });

        // Task de polling remoto periódico (Requisito 3.4)
        let engine_for_poll = Arc::clone(self);
        tokio::spawn(async move {
            while engine_for_poll.running.load(Ordering::SeqCst) {
                let interval = engine_for_poll
                    .get_config()
                    .sync_interval_secs
                    .clamp(1, 60);
                tokio::time::sleep(Duration::from_secs(interval)).await;
                if !engine_for_poll.running.load(Ordering::SeqCst) {
                    break;
                }
                if engine_for_poll.network_detector.is_online() {
                    let _ = engine_for_poll.poll_remote_changes().await;
                    let _ = engine_for_poll.process_queue().await;
                }
            }
        });

        Ok(())
    }

    /// Shutdown gracioso: para watcher e aguarda transferências ativas por até 60s (Requisito 1.5)
    pub async fn stop(&self) -> Result<(), SyncError> {
        self.running.store(false, Ordering::SeqCst);
        self.network_detector.stop();

        if let Some(mut fw) = self.watcher.lock().unwrap().take() {
            fw.stop();
        }

        let _ = tokio::time::timeout(Duration::from_secs(60), self.drain_active_transfers()).await;
        Ok(())
    }

    pub async fn drain_active_transfers(&self) {
        while self.active_transfers.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn enqueue_and_notify(&self, op: SyncOperation) -> Result<Option<i64>, SyncError> {
        match self.queue_manager.enqueue(op.clone())? {
            Some(id) => {
                self.status_monitor
                    .set_state(&op.s3_key, SyncState::Pending);
                self.emit(
                    "sync:file-status-changed",
                    &FileStatusEvent {
                        path: op.s3_key,
                        state: SyncState::Pending,
                    },
                );
                Ok(Some(id))
            }
            None => {
                self.emit_void("queue:full");
                Err(SyncError::QueueFull)
            }
        }
    }

    pub async fn process_local_event(&self, event: LocalFsEvent) -> Result<(), SyncError> {
        let cfg = self.get_config();
        if cfg.sync_direction.eq_ignore_ascii_case("DownloadOnly") {
            return Ok(());
        }
        let base = PathBuf::from(&cfg.local_path);

        match event {
            LocalFsEvent::Created(path) | LocalFsEvent::Modified(path) => {
                if path.is_dir() || is_excluded(&path, &cfg.exclusion_patterns) {
                    return Ok(());
                }
                // Ignorar arquivos temporários de download ou cópias de conflito geradas
                if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                    return Ok(());
                }

                let s3_key = apply_s3_prefix(local_path_to_s3_key(&path, &base), &cfg.s3_prefix);

                // Se o arquivo local já está registrado com o mesmo MD5 (ex: recém-baixado do S3), ignorar evento
                if let Ok(Some(entry)) = self.db.get_file_entry_by_key(&s3_key) {
                    if let (Some(ref sync_etag), Ok(local_md5)) =
                        (&entry.last_sync_etag, compute_md5(&path))
                    {
                        if sync_etag.eq_ignore_ascii_case(&local_md5) {
                            return Ok(());
                        }
                    }
                }

                let op = SyncOperation {
                    operation_type: SyncOperationType::Upload,
                    s3_key,
                    local_path: path.to_string_lossy().to_string(),
                    extra: None,
                };
                self.enqueue_and_notify(op)?;
            }
            LocalFsEvent::Deleted(path) => {
                if is_excluded(&path, &cfg.exclusion_patterns) {
                    return Ok(());
                }
                let s3_key = apply_s3_prefix(local_path_to_s3_key(&path, &base), &cfg.s3_prefix);
                let op = SyncOperation {
                    operation_type: SyncOperationType::Delete,
                    s3_key,
                    local_path: path.to_string_lossy().to_string(),
                    extra: None,
                };
                self.enqueue_and_notify(op)?;
            }
            LocalFsEvent::Renamed { from, to } => {
                if is_excluded(&to, &cfg.exclusion_patterns) {
                    return Ok(());
                }
                let from_key = apply_s3_prefix(local_path_to_s3_key(&from, &base), &cfg.s3_prefix);
                let to_key = apply_s3_prefix(local_path_to_s3_key(&to, &base), &cfg.s3_prefix);
                let op = SyncOperation {
                    operation_type: SyncOperationType::Rename,
                    s3_key: from_key,
                    local_path: to.to_string_lossy().to_string(),
                    extra: Some(to_key),
                };
                self.enqueue_and_notify(op)?;
            }
        }

        Ok(())
    }

    pub async fn poll_remote_changes(&self) -> Result<(), SyncError> {
        if !self.network_detector.is_online() {
            return Ok(());
        }

        let cfg = self.get_config();
        let is_upload_only = cfg.sync_direction.eq_ignore_ascii_case("UploadOnly");
        let base = PathBuf::from(&cfg.local_path);
        let s3_client = self
            .s3_client
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| SyncError::Credentials("S3Client not initialized".to_string()))?;

        let remote_objects = match s3_client.list_objects(cfg.s3_prefix.as_deref()).await {
            Ok(objs) => objs,
            Err(S3Error::Auth(msg)) => {
                self.emit_void("credentials:expired");
                return Err(SyncError::Credentials(msg));
            }
            Err(e) => return Err(SyncError::from(e)),
        };

        let existing_entries = self
            .db
            .list_file_entries()
            .map_err(|e| SyncError::Queue(e.to_string()))?;

        let mut seen_keys = HashSet::new();

        for s3_obj in &remote_objects {
            let local_path = s3_key_to_local_path(&s3_obj.key, &base);
            if is_excluded(&local_path, &cfg.exclusion_patterns) {
                continue;
            }
            // Ignorar marcadores de pasta do S3 (chaves terminadas em '/') criando apenas o diretório local
            if s3_obj.key.ends_with('/') {
                if !is_upload_only {
                    let _ = ensure_dir_exists(&local_path);
                }
                continue;
            }
            seen_keys.insert(s3_obj.key.clone());

            let existing = existing_entries.iter().find(|e| e.s3_key == s3_obj.key);
            match existing {
                None => {
                    if !is_upload_only {
                        if local_path.exists() {
                            // Arquivo existe localmente mas não está registrado no BD.
                            // Verificar se o conteúdo já coincide com o remoto antes de sobrescrever.
                            if let Ok(local_md5) = compute_md5(&local_path) {
                                if local_md5 == s3_obj.etag {
                                    // Conteúdo idêntico: apenas registrar sem fazer download
                                    let size = fs::metadata(&local_path)
                                        .map(|m| m.len())
                                        .unwrap_or(s3_obj.size);
                                    let now = Utc::now();
                                    let entry = FileEntry {
                                        s3_key: s3_obj.key.clone(),
                                        local_path: local_path.to_string_lossy().to_string(),
                                        sync_state: SyncState::Synced,
                                        last_sync_etag: Some(s3_obj.etag.clone()),
                                        last_sync_time: Some(now),
                                        local_modified_time: Some(now),
                                        size_bytes: size,
                                    };
                                    let _ = self.db.upsert_file_entry(&entry);
                                    self.status_monitor
                                        .set_file_status(&s3_obj.key, SyncState::Synced, size);
                                    continue;
                                }
                            }
                            // Conteúdos diferem: tratar como conflito para não sobrescrever dados locais
                            let local_modified = fs::metadata(&local_path)
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .map(DateTime::<Utc>::from)
                                .unwrap_or_else(Utc::now);
                            let size_bytes =
                                fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0);
                            let synthetic_entry = FileEntry {
                                s3_key: s3_obj.key.clone(),
                                local_path: local_path.to_string_lossy().to_string(),
                                sync_state: SyncState::Pending,
                                last_sync_etag: None,
                                last_sync_time: None,
                                local_modified_time: Some(local_modified),
                                size_bytes,
                            };
                            let resolver = ConflictResolver::new(base.clone());
                            match resolver.resolve(&synthetic_entry, s3_obj, &s3_client).await {
                                Ok(conflict_info) => {
                                    let event = ConflictEvent::from(&conflict_info);
                                    self.emit("conflict:detected", &event);
                                }
                                Err(err) => {
                                    let err_str = err.to_string();
                                    self.status_monitor
                                        .set_state(&s3_obj.key, SyncState::Error(err_str.clone()));
                                    self.emit(
                                        "sync:error",
                                        &SyncErrorEvent {
                                            file: s3_obj.key.clone(),
                                            error: err_str,
                                            retry_count: 3,
                                        },
                                    );
                                }
                            }
                        } else {
                            // Objeto novo no S3, sem arquivo local — download normal
                            let op = SyncOperation {
                                operation_type: SyncOperationType::Download,
                                s3_key: s3_obj.key.clone(),
                                local_path: local_path.to_string_lossy().to_string(),
                                extra: None,
                            };
                            self.enqueue_and_notify(op)?;
                        }
                    }
                }
                Some(entry) => {
                    // Atualizar local_modified_time se o arquivo local mudou em disco
                    let mut updated_entry = entry.clone();
                    if local_path.exists() {
                        if let Ok(file_md5) = compute_md5(&local_path) {
                            if Some(file_md5) != entry.last_sync_etag {
                                updated_entry.local_modified_time = Some(Utc::now());
                                if let Ok(meta) = fs::metadata(&local_path) {
                                    updated_entry.size_bytes = meta.len();
                                }
                            }
                        }
                    }

                    if !is_upload_only && ConflictResolver::detect_conflict(&updated_entry, s3_obj) {
                        let resolver = ConflictResolver::new(base.clone());
                        match resolver.resolve(&updated_entry, s3_obj, &s3_client).await {
                            Ok(conflict_info) => {
                                let event = ConflictEvent::from(&conflict_info);
                                self.emit("conflict:detected", &event);
                            }
                            Err(err) => {
                                let err_str = err.to_string();
                                self.status_monitor
                                    .set_state(&s3_obj.key, SyncState::Error(err_str.clone()));
                                self.emit(
                                    "sync:error",
                                    &SyncErrorEvent {
                                        file: s3_obj.key.clone(),
                                        error: err_str,
                                        retry_count: 3,
                                    },
                                );
                            }
                        }
                    } else if !is_upload_only && Some(s3_obj.etag.clone()) != entry.last_sync_etag {
                        // Modificado apenas remotamente -> Download
                        let op = SyncOperation {
                            operation_type: SyncOperationType::Download,
                            s3_key: s3_obj.key.clone(),
                            local_path: local_path.to_string_lossy().to_string(),
                            extra: None,
                        };
                        self.enqueue_and_notify(op)?;
                    } else {
                        // Já sincronizado e íntegro -> garantir que está visível no StatusMonitor
                        self.status_monitor.set_file_status(
                            &s3_obj.key,
                            SyncState::Synced,
                            s3_obj.size,
                        );
                    }
                }
            }
        }

        // Objetos que existiam no file_registry mas foram removidos do S3 (Requisito 3.3)
        if !is_upload_only {
            for entry in &existing_entries {
                if !seen_keys.contains(&entry.s3_key) {
                    let local_path = PathBuf::from(&entry.local_path);
                    if is_excluded(&local_path, &cfg.exclusion_patterns) {
                        continue;
                    }
                    if local_path.exists() {
                        let _ = fs::remove_file(&local_path);
                    }
                    let _ = self.db.delete_file_entry(&entry.s3_key);
                    self.status_monitor.remove_state(&entry.s3_key);
                }
            }
        }

        Ok(())
    }

    pub async fn process_queue(self: &Arc<Self>) -> Result<(), SyncError> {
        if !self.network_detector.is_online() {
            return Ok(());
        }

        let s3_client = self
            .s3_client
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| SyncError::Credentials("S3Client not initialized".to_string()))?;

        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let concurrency = cfg.max_concurrent_transfers.clamp(1, 16);
        let synced_in_batch = Arc::new(AtomicUsize::new(0));

        loop {
            if !self.network_detector.is_online() {
                break;
            }

            let mut batch = Vec::with_capacity(concurrency);
            for _ in 0..concurrency {
                match self.queue_manager.dequeue_next()? {
                    Some(item) => batch.push(item),
                    None => break,
                }
            }

            if batch.is_empty() {
                break;
            }

            let mut handles = Vec::with_capacity(batch.len());
            for item in batch {
                let engine = Arc::clone(self);
                let client = s3_client.clone();
                let base_dir = base.clone();
                let synced_counter = Arc::clone(&synced_in_batch);

                handles.push(tokio::spawn(async move {
                    engine.active_transfers.fetch_add(1, Ordering::SeqCst);
                    let s3_key = item.operation.s3_key.clone();

                    engine.status_monitor.set_state(&s3_key, SyncState::Syncing);
                    engine.emit(
                        "sync:file-status-changed",
                        &FileStatusEvent {
                            path: s3_key.clone(),
                            state: SyncState::Syncing,
                        },
                    );

                    let is_download =
                        matches!(item.operation.operation_type, SyncOperationType::Download);
                    let mut last_error = String::new();
                    let mut succeeded = false;

                    for attempt in 1..=engine.retry_policy.max_attempts {
                        let exec_res = engine
                            .execute_single_operation(&item.operation, &client, &base_dir)
                            .await;

                        match exec_res {
                            Ok(()) => {
                                succeeded = true;
                                break;
                            }
                            Err(SyncError::Credentials(msg)) => {
                                engine.emit_void("credentials:expired");
                                last_error = msg;
                                break;
                            }
                            Err(e) => {
                                last_error = e.to_string();
                                if attempt < engine.retry_policy.max_attempts {
                                    let delay = engine
                                        .retry_policy
                                        .delay_for_attempt(attempt, is_download);
                                    tokio::time::sleep(delay).await;
                                }
                            }
                        }
                    }

                    if succeeded {
                        let _ = engine.queue_manager.mark_completed(item.id);
                        engine.status_monitor.set_state(&s3_key, SyncState::Synced);
                        engine.emit(
                            "sync:file-status-changed",
                            &FileStatusEvent {
                                path: s3_key,
                                state: SyncState::Synced,
                            },
                        );
                        synced_counter.fetch_add(1, Ordering::SeqCst);
                    } else {
                        for _ in 0..engine.retry_policy.max_attempts {
                            let _ = engine.queue_manager.mark_failed(item.id, &last_error);
                        }
                        engine
                            .status_monitor
                            .set_state(&s3_key, SyncState::Error(last_error.clone()));
                        engine.emit(
                            "sync:file-status-changed",
                            &FileStatusEvent {
                                path: s3_key.clone(),
                                state: SyncState::Error(last_error.clone()),
                            },
                        );
                        engine.emit(
                            "sync:error",
                            &SyncErrorEvent {
                                file: s3_key,
                                error: last_error,
                                retry_count: engine.retry_policy.max_attempts,
                            },
                        );
                    }

                    engine.active_transfers.fetch_sub(1, Ordering::SeqCst);
                }));
            }

            for h in handles {
                let _ = h.await;
            }
        }

        if !self.initial_sync_done.swap(true, Ordering::SeqCst) {
            self.emit(
                "sync:initial-complete",
                &InitialCompleteEvent {
                    files_synced: synced_in_batch.load(Ordering::SeqCst),
                },
            );
        }

        Ok(())
    }

    async fn execute_single_operation(
        &self,
        op: &SyncOperation,
        s3_client: &S3Client,
        base: &Path,
    ) -> Result<(), SyncError> {
        match op.operation_type {
            SyncOperationType::Upload => {
                let path = PathBuf::from(&op.local_path);
                if !path.exists() {
                    return Ok(());
                }

                // Verificar conflito antes de enviar (Requisito 6.1, 8.4)
                if let Ok(Some(mut entry)) = self.db.get_file_entry_by_key(&op.s3_key) {
                    if let Ok(remote_info) = s3_client.head_object(&op.s3_key).await {
                        if let Ok(local_md5) = compute_md5(&path) {
                            if Some(local_md5) != entry.last_sync_etag {
                                entry.local_modified_time = Some(Utc::now());
                            }
                        }
                        if ConflictResolver::detect_conflict(&entry, &remote_info) {
                            let resolver = ConflictResolver::new(base.to_path_buf());
                            let info = resolver.resolve(&entry, &remote_info, s3_client).await?;
                            let ev = ConflictEvent::from(&info);
                            self.emit("conflict:detected", &ev);
                        }
                    }
                }

                let meta = fs::metadata(&path).map_err(|e| SyncError::Io(e.to_string()))?;
                let file_size = meta.len();

                self.emit(
                    "sync:transfer-progress",
                    &TransferProgressEvent {
                        path: op.s3_key.clone(),
                        bytes_transferred: 0,
                        total_bytes: file_size,
                        percent: 0,
                    },
                );

                let etag = if S3Client::requires_multipart(file_size) {
                    let resume = self
                        .db
                        .get_multipart_state(&op.s3_key)
                        .ok()
                        .flatten();
                    let key_for_ev = op.s3_key.clone();
                    s3_client
                        .upload_multipart_with_progress(&op.s3_key, &path, resume, |sent, total| {
                            let pct = if total > 0 {
                                ((sent.saturating_mul(100)) / total).min(100) as u8
                            } else {
                                100
                            };
                            self.emit(
                                "sync:transfer-progress",
                                &TransferProgressEvent {
                                    path: key_for_ev.clone(),
                                    bytes_transferred: sent,
                                    total_bytes: total,
                                    percent: pct,
                                },
                            );
                        })
                        .await?
                } else {
                    let res = s3_client.upload_file(&op.s3_key, &path).await?;
                    self.emit(
                        "sync:transfer-progress",
                        &TransferProgressEvent {
                            path: op.s3_key.clone(),
                            bytes_transferred: file_size,
                            total_bytes: file_size,
                            percent: 100,
                        },
                    );
                    res
                };

                let now = Utc::now();
                let entry = FileEntry {
                    s3_key: op.s3_key.clone(),
                    local_path: op.local_path.clone(),
                    sync_state: SyncState::Synced,
                    last_sync_etag: Some(etag),
                    last_sync_time: Some(now),
                    local_modified_time: Some(now),
                    size_bytes: file_size,
                };
                let _ = self.db.upsert_file_entry(&entry);
                self.status_monitor
                    .set_file_status(&op.s3_key, SyncState::Synced, file_size);
                Ok(())
            }
            SyncOperationType::Download => {
                let dest = PathBuf::from(&op.local_path);
                self.emit(
                    "sync:transfer-progress",
                    &TransferProgressEvent {
                        path: op.s3_key.clone(),
                        bytes_transferred: 0,
                        total_bytes: 0,
                        percent: 15,
                    },
                );
                let etag = s3_client.download_file(&op.s3_key, &dest).await?;
                let size_bytes = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                self.emit(
                    "sync:transfer-progress",
                    &TransferProgressEvent {
                        path: op.s3_key.clone(),
                        bytes_transferred: size_bytes,
                        total_bytes: size_bytes,
                        percent: 100,
                    },
                );
                let now = Utc::now();
                let entry = FileEntry {
                    s3_key: op.s3_key.clone(),
                    local_path: op.local_path.clone(),
                    sync_state: SyncState::Synced,
                    last_sync_etag: Some(etag),
                    last_sync_time: Some(now),
                    local_modified_time: Some(now),
                    size_bytes,
                };
                let _ = self.db.upsert_file_entry(&entry);
                self.status_monitor
                    .set_file_status(&op.s3_key, SyncState::Synced, size_bytes);
                Ok(())
            }
            SyncOperationType::Delete => {
                s3_client.delete_object(&op.s3_key).await?;
                let _ = self.db.delete_file_entry(&op.s3_key);
                Ok(())
            }
            SyncOperationType::Rename | SyncOperationType::Move => {
                let dst_key = op.extra.as_deref().ok_or_else(|| {
                    SyncError::Io("Missing destination key for Rename/Move".to_string())
                })?;

                // Obter ETag da origem para propagar ao registro do destino (cópia preserva o mesmo conteúdo)
                let src_etag = self
                    .db
                    .get_file_entry_by_key(&op.s3_key)
                    .ok()
                    .flatten()
                    .and_then(|e| e.last_sync_etag);

                // Copiar para o novo caminho com fallback para upload (Requisito 2.3, 2.4)
                let dst_etag: Option<String> =
                    match s3_client.copy_object(&op.s3_key, dst_key).await {
                        Ok(_) => src_etag,
                        Err(_) => {
                            let path = PathBuf::from(&op.local_path);
                            if path.exists() {
                                // Fallback: fazer upload do arquivo local para o destino
                                Some(s3_client.upload_file(dst_key, &path).await?)
                            } else {
                                // Nem cópia nem upload possíveis — abortar sem excluir a origem
                                return Err(SyncError::Io(format!(
                                    "Falha ao renomear '{}': cópia S3 falhou e arquivo local '{}' não existe",
                                    op.s3_key, op.local_path
                                )));
                            }
                        }
                    };

                // Requisito 2.9: excluir a origem somente após destino confirmado
                if let Err(e) = s3_client.delete_object(&op.s3_key).await {
                    return Err(SyncError::S3(format!(
                        "Objeto copiado para '{}', mas falha ao excluir original '{}': {}",
                        dst_key, op.s3_key, e
                    )));
                }

                let _ = self.db.delete_file_entry(&op.s3_key);

                // Registrar destino no banco para evitar re-download no próximo poll
                let now = Utc::now();
                let size_bytes = PathBuf::from(&op.local_path)
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let new_entry = FileEntry {
                    s3_key: dst_key.to_string(),
                    local_path: op.local_path.clone(),
                    sync_state: SyncState::Synced,
                    last_sync_etag: dst_etag,
                    last_sync_time: Some(now),
                    local_modified_time: Some(now),
                    size_bytes,
                };
                let _ = self.db.upsert_file_entry(&new_entry);
                Ok(())
            }
        }
    }

    /// Resolve conflito a partir da escolha do usuário na notificação (Requisito 10.3)
    pub fn resolve_conflict_action(
        &self,
        strategy: &str,
        local_path: &str,
        conflict_path: &str,
    ) -> Result<(), SyncError> {
        match strategy {
            "local" => {
                // Manter versão local: remover cópia de conflito
                if Path::new(conflict_path).exists() {
                    fs::remove_file(conflict_path).map_err(|e| SyncError::Io(e.to_string()))?;
                }
                Ok(())
            }
            "remote" => {
                // Manter versão remota: substituir arquivo local pela cópia de conflito
                if Path::new(conflict_path).exists() {
                    if Path::new(local_path).exists() {
                        fs::remove_file(local_path).map_err(|e| SyncError::Io(e.to_string()))?;
                    }
                    fs::rename(conflict_path, local_path)
                        .map_err(|e| SyncError::Io(e.to_string()))?;
                }
                Ok(())
            }
            "both" => {
                // Manter ambas: ambas já estão salvas na Pasta_Local
                Ok(())
            }
            other => Err(SyncError::Conflict(format!(
                "Estratégia de resolução desconhecida: {}",
                other
            ))),
        }
    }

    /// Varre recursivamente a pasta local para detectar arquivos adicionados/modificados enquanto o app estava fechado
    pub async fn scan_local_directory(&self) -> Result<(), SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        if !base.exists() {
            return Ok(());
        }

        let mut stack = vec![base];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry_res in entries {
                let entry = match entry_res {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let path = entry.path();
                if path.is_dir() {
                    if !is_excluded(&path, &cfg.exclusion_patterns) {
                        stack.push(path);
                    }
                } else if path.is_file() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.contains("(conflito-") || name.ends_with(".tmp") {
                        continue;
                    }
                    let _ = self
                        .process_local_event(LocalFsEvent::Modified(path))
                        .await;
                }
            }
        }
        Ok(())
    }

    /// Reenfileira um arquivo específico (ex: em estado de Erro) e processa a fila
    pub async fn retry_file(self: &Arc<Self>, s3_key: &str) -> Result<(), SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let local_path = s3_key_to_local_path(s3_key, &base);

        let op_type = if local_path.exists() {
            match self.db.get_file_entry_by_key(s3_key) {
                Ok(Some(_)) => SyncOperationType::Upload,
                _ => {
                    // Se o objeto existe no S3 mas não no registro, ou foi criado localmente
                    let s3_client_opt = self.s3_client.read().unwrap().clone();
                    if let Some(client) = s3_client_opt {
                        if client.head_object(s3_key).await.is_ok() {
                            SyncOperationType::Download
                        } else {
                            SyncOperationType::Upload
                        }
                    } else {
                        SyncOperationType::Upload
                    }
                }
            }
        } else {
            SyncOperationType::Download
        };

        let op = SyncOperation {
            operation_type: op_type,
            s3_key: s3_key.to_string(),
            local_path: local_path.to_string_lossy().to_string(),
            extra: None,
        };

        self.enqueue_and_notify(op)?;
        self.process_queue().await
    }

    /// Reenfileira todos os arquivos atualmente em estado de Erro e processa a fila em lote
    pub async fn retry_all_failed(self: &Arc<Self>) -> Result<usize, SyncError> {
        let errored_keys: Vec<String> = self
            .status_monitor
            .get_all()
            .into_iter()
            .filter(|f| matches!(f.state, SyncState::Error(_)))
            .map(|f| f.path)
            .collect();

        if errored_keys.is_empty() {
            return Ok(0);
        }

        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let mut count = 0usize;

        for s3_key in errored_keys {
            let local_path = s3_key_to_local_path(&s3_key, &base);
            let op_type = if local_path.exists() {
                SyncOperationType::Upload
            } else {
                SyncOperationType::Download
            };
            let op = SyncOperation {
                operation_type: op_type,
                s3_key,
                local_path: local_path.to_string_lossy().to_string(),
                extra: None,
            };
            if self.enqueue_and_notify(op)?.is_some() {
                count += 1;
            }
        }

        self.process_queue().await?;
        Ok(count)
    }

    /// Gera um link pré-assinado temporário (Presigned URL) para compartilhamento de um objeto S3
    pub async fn presign_object_url(
        &self,
        s3_key: &str,
        expires_in_secs: u64,
    ) -> Result<String, SyncError> {
        let client_opt = self.s3_client.read().unwrap().clone();
        let client = client_opt.ok_or_else(|| {
            SyncError::Credentials("Cliente S3 não inicializado. Inicie a sincronização primeiro.".to_string())
        })?;
        client
            .generate_presigned_url(s3_key, expires_in_secs)
            .await
            .map_err(SyncError::from)
    }

    /// Importa arquivos externos (Drag & Drop ou seletor) para dentro da pasta mapeada e enfileira upload imediato
    pub async fn import_external_files(
        self: &Arc<Self>,
        source_paths: &[String],
        target_subfolder: Option<&str>,
    ) -> Result<usize, SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        if cfg.local_path.trim().is_empty() {
            return Err(SyncError::Config(
                "Pasta local não configurada.".to_string(),
            ));
        }

        let target_dir = if let Some(sub) = target_subfolder {
            let trimmed = sub.trim().trim_matches('/');
            if trimmed.is_empty() {
                base.clone()
            } else {
                s3_key_to_local_path(trimmed, &base)
            }
        } else {
            base.clone()
        };

        ensure_dir_exists(&target_dir)?;

        let mut imported = 0usize;
        for src_str in source_paths {
            let src = PathBuf::from(src_str);
            if !src.exists() || !src.is_file() {
                continue;
            }
            let Some(file_name) = src.file_name() else {
                continue;
            };
            let dest = target_dir.join(file_name);
            if src != dest {
                fs::copy(&src, &dest).map_err(|e| SyncError::Io(e.to_string()))?;
            }
            self.process_local_event(LocalFsEvent::Created(dest)).await?;
            imported += 1;
        }

        if imported > 0 && self.network_detector.is_online() {
            let _ = self.process_queue().await;
        }

        Ok(imported)
    }

    /// Retorna metadados técnicos completos de um objeto (SQLite + arquivo local + configuração S3)
    pub fn get_object_metadata(&self, s3_key: &str) -> Result<ObjectDetailedMetadata, SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let local_path = s3_key_to_local_path(s3_key, &base);
        let local_exists = local_path.exists() && local_path.is_file();

        let db_entry = self
            .db
            .get_file_entry_by_key(s3_key)
            .ok()
            .flatten();

        let size_bytes = if local_exists {
            fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0)
        } else {
            db_entry.as_ref().map(|e| e.size_bytes).unwrap_or(0)
        };

        let local_md5 = if local_exists && size_bytes <= 100 * 1024 * 1024 {
            compute_md5(&local_path).ok()
        } else {
            None
        };

        Ok(ObjectDetailedMetadata {
            s3_key: s3_key.to_string(),
            local_path: local_path.to_string_lossy().to_string(),
            local_exists,
            size_bytes,
            etag: db_entry.as_ref().and_then(|e| e.last_sync_etag.clone()),
            local_md5,
            last_sync_time: db_entry
                .as_ref()
                .and_then(|e| e.last_sync_time.map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())),
            local_modified_time: db_entry
                .as_ref()
                .and_then(|e| e.local_modified_time.map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())),
            storage_class: cfg.storage_class,
            server_side_encryption: cfg.server_side_encryption,
        })
    }

    /// Lista as versões remotas disponíveis para um objeto no bucket S3
    pub async fn list_versions_for_key(
        &self,
        s3_key: &str,
    ) -> Result<Vec<S3ObjectVersion>, SyncError> {
        let client_opt = self.s3_client.read().unwrap().clone();
        let client = client_opt.ok_or_else(|| {
            SyncError::Credentials(
                "Cliente S3 não inicializado. Inicie a sincronização primeiro.".to_string(),
            )
        })?;
        client
            .list_object_versions(s3_key)
            .await
            .map_err(SyncError::from)
    }

    /// Restaura uma versão específica (`version_id`) de um objeto do S3 para a pasta local
    pub async fn restore_version_for_key(
        &self,
        s3_key: &str,
        version_id: &str,
    ) -> Result<(), SyncError> {
        let client_opt = self.s3_client.read().unwrap().clone();
        let client = client_opt.ok_or_else(|| {
            SyncError::Credentials(
                "Cliente S3 não inicializado. Inicie a sincronização primeiro.".to_string(),
            )
        })?;
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let dest = s3_key_to_local_path(s3_key, &base);

        let etag = client
            .download_object_version(s3_key, version_id, &dest)
            .await?;
        let size_bytes = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let now = Utc::now();
        let entry = FileEntry {
            s3_key: s3_key.to_string(),
            local_path: dest.to_string_lossy().to_string(),
            sync_state: SyncState::Synced,
            last_sync_etag: Some(etag),
            last_sync_time: Some(now),
            local_modified_time: Some(now),
            size_bytes,
        };
        let _ = self.db.upsert_file_entry(&entry);
        self.status_monitor
            .set_file_status(s3_key, SyncState::Synced, size_bytes);
        self.emit(
            "sync:file-status-changed",
            &FileStatusEvent {
                path: s3_key.to_string(),
                state: SyncState::Synced,
            },
        );
        Ok(())
    }

    /// Cria uma nova pasta na estrutura local mapeada
    pub fn create_local_subfolder(
        &self,
        parent_prefix: Option<&str>,
        folder_name: &str,
    ) -> Result<String, SyncError> {
        let clean_name = folder_name.trim().trim_matches('/').trim_matches('\\');
        if clean_name.is_empty() || clean_name == "." || clean_name == ".." {
            return Err(SyncError::Io("Nome de pasta inválido.".to_string()));
        }
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        if cfg.local_path.trim().is_empty() {
            return Err(SyncError::Config(
                "Pasta local não configurada.".to_string(),
            ));
        }

        let rel_key = match parent_prefix {
            Some(p) if !p.trim().is_empty() => {
                format!("{}/{}", p.trim().trim_matches('/'), clean_name)
            }
            _ => clean_name.to_string(),
        };
        let folder_path = s3_key_to_local_path(&rel_key, &base);
        ensure_dir_exists(&folder_path)?;
        Ok(format!("{}/", rel_key))
    }

    /// Exclui um objeto localmente e enfileira a remoção remota no S3
    pub async fn delete_file_and_sync(self: &Arc<Self>, s3_key: &str) -> Result<(), SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let local_path = s3_key_to_local_path(s3_key, &base);

        if local_path.exists() && local_path.is_file() {
            let _ = fs::remove_file(&local_path);
        }

        let op = SyncOperation {
            operation_type: SyncOperationType::Delete,
            s3_key: s3_key.to_string(),
            local_path: local_path.to_string_lossy().to_string(),
            extra: None,
        };
        self.enqueue_and_notify(op)?;
        if self.network_detector.is_online() {
            let _ = self.process_queue().await;
        }
        self.status_monitor.remove_state(s3_key);
        Ok(())
    }

    /// Renomeia ou move um arquivo localmente e sincroniza a operação no S3 (Copy/Upload + Delete antigo)
    pub async fn rename_or_move_file(
        self: &Arc<Self>,
        old_s3_key: &str,
        new_s3_key: &str,
    ) -> Result<String, SyncError> {
        let normalized_new = new_s3_key
            .trim()
            .replace('\\', "/")
            .trim_matches('/')
            .to_string();
        if normalized_new.is_empty() || normalized_new.contains("..") {
            return Err(SyncError::Io("Nova chave S3 inválida.".to_string()));
        }
        if normalized_new == old_s3_key {
            return Ok(normalized_new);
        }

        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let old_local = s3_key_to_local_path(old_s3_key, &base);
        let new_local = s3_key_to_local_path(&normalized_new, &base);

        if let Some(parent) = new_local.parent() {
            ensure_dir_exists(parent)?;
        }

        let mut size_bytes = 0u64;
        if old_local.exists() {
            size_bytes = fs::metadata(&old_local).map(|m| m.len()).unwrap_or(0);
            fs::rename(&old_local, &new_local).map_err(|e| SyncError::Io(e.to_string()))?;
        }

        let op = SyncOperation {
            operation_type: SyncOperationType::Move,
            s3_key: old_s3_key.to_string(),
            local_path: new_local.to_string_lossy().to_string(),
            extra: Some(normalized_new.clone()),
        };
        self.enqueue_and_notify(op)?;

        if self.network_detector.is_online() {
            let _ = self.process_queue().await;
        }

        self.status_monitor.remove_state(old_s3_key);
        self.status_monitor
            .set_file_status(&normalized_new, SyncState::Synced, size_bytes);
        self.emit(
            "sync:file-status-changed",
            &FileStatusEvent {
                path: normalized_new.clone(),
                state: SyncState::Synced,
            },
        );

        Ok(normalized_new)
    }

    /// Exclui múltiplos objetos em lote localmente e no bucket S3
    pub async fn batch_delete_files(
        self: &Arc<Self>,
        s3_keys: &[String],
    ) -> Result<usize, SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let mut count = 0usize;

        for s3_key in s3_keys {
            let local_path = s3_key_to_local_path(s3_key, &base);
            if local_path.exists() && local_path.is_file() {
                let _ = fs::remove_file(&local_path);
            }
            let op = SyncOperation {
                operation_type: SyncOperationType::Delete,
                s3_key: s3_key.clone(),
                local_path: local_path.to_string_lossy().to_string(),
                extra: None,
            };
            if self.enqueue_and_notify(op)?.is_some() {
                count += 1;
            }
            self.status_monitor.remove_state(s3_key);
        }

        if count > 0 && self.network_detector.is_online() {
            let _ = self.process_queue().await;
        }
        Ok(count)
    }

    /// Reenfileira múltiplos objetos selecionados para sincronização
    pub async fn batch_resync_files(
        self: &Arc<Self>,
        s3_keys: &[String],
    ) -> Result<usize, SyncError> {
        let cfg = self.get_config();
        let base = PathBuf::from(&cfg.local_path);
        let mut count = 0usize;

        for s3_key in s3_keys {
            let local_path = s3_key_to_local_path(s3_key, &base);
            let op_type = if local_path.exists() {
                SyncOperationType::Upload
            } else {
                SyncOperationType::Download
            };
            let op = SyncOperation {
                operation_type: op_type,
                s3_key: s3_key.clone(),
                local_path: local_path.to_string_lossy().to_string(),
                extra: None,
            };
            if self.enqueue_and_notify(op)?.is_some() {
                count += 1;
            }
        }

        if count > 0 && self.network_detector.is_online() {
            let _ = self.process_queue().await;
        }
        Ok(count)
    }

    /// Gera links pré-assinados (Presigned URLs) em lote para múltiplos objetos
    pub async fn batch_presign_urls(
        &self,
        s3_keys: &[String],
        expires_in_secs: u64,
    ) -> Result<Vec<String>, SyncError> {
        let client_opt = self.s3_client.read().unwrap().clone();
        let client = client_opt.ok_or_else(|| {
            SyncError::Credentials(
                "Cliente S3 não inicializado. Inicie a sincronização primeiro.".to_string(),
            )
        })?;
        let mut urls = Vec::with_capacity(s3_keys.len());
        for key in s3_keys {
            let url = client.generate_presigned_url(key, expires_in_secs).await?;
            urls.push(url);
        }
        Ok(urls)
    }

    /// Calcula métricas consolidadas de armazenamento e estimativa de custo mensal S3
    pub fn compute_storage_analytics(&self) -> StorageAnalytics {
        let cfg = self.get_config();
        let all = self.status_monitor.get_all();

        let total_files = all.len();
        let mut total_bytes = 0u64;
        let mut synced_files = 0usize;
        let mut pending_or_syncing_files = 0usize;
        let mut error_files = 0usize;
        let mut ext_map: HashMap<String, (usize, u64)> = HashMap::new();

        for item in &all {
            total_bytes = total_bytes.saturating_add(item.size_bytes);
            match item.state {
                SyncState::Synced => synced_files += 1,
                SyncState::Pending | SyncState::Syncing => pending_or_syncing_files += 1,
                SyncState::Error(_) => error_files += 1,
            }

            let ext = Path::new(&item.path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| format!(".{}", s.to_ascii_lowercase()))
                .unwrap_or_else(|| "(sem ext)".to_string());
            let entry = ext_map.entry(ext).or_insert((0, 0));
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(item.size_bytes);
        }

        let mut top_extensions: Vec<ExtensionStat> = ext_map
            .into_iter()
            .map(|(extension, (count, bytes))| ExtensionStat {
                extension,
                count,
                bytes,
            })
            .collect();
        top_extensions.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        top_extensions.truncate(5);

        let gb = (total_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
        let rate_per_gb = match cfg.storage_class.as_str() {
            "STANDARD_IA" => 0.0125,
            "GLACIER_IR" => 0.004,
            "INTELLIGENT_TIERING" => 0.023,
            _ => 0.023,
        };
        let estimated_monthly_usd = (gb * rate_per_gb * 10000.0).round() / 10000.0;

        StorageAnalytics {
            total_files,
            total_bytes,
            synced_files,
            pending_or_syncing_files,
            error_files,
            storage_class: cfg.storage_class,
            estimated_monthly_usd,
            top_extensions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_s3_key() -> impl Strategy<Value = String> {
        prop::collection::vec("[a-zA-Z0-9][a-zA-Z0-9._-]{0,14}", 1..=5)
            .prop_map(|parts| parts.join("/"))
    }

    fn arb_glob_pattern() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("*.tmp".to_string()),
            Just("*.log".to_string()),
            Just(".DS_Store".to_string()),
            Just("~$*".to_string()),
            Just("*.bak".to_string()),
        ]
    }

    // Feature: amazon-s3-drive, Property 2: S3 key <-> local path round-trip
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn s3_key_local_path_roundtrip(key in arb_s3_key()) {
            let base = Path::new("/base/drive");
            let local = s3_key_to_local_path(&key, base);
            let restored = local_path_to_s3_key(&local, base);
            prop_assert_eq!(normalize_key(&key), restored);
        }
    }

    // Feature: amazon-s3-drive, Property 8: Exclusion pattern filtering
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn excluded_files_not_synced(
            filename in "[a-zA-Z0-9_-]{1,20}(\\.(tmp|log|txt|pdf|bak))?",
            patterns in prop::collection::vec(arb_glob_pattern(), 1..=5),
        ) {
            let matches_any = patterns.iter().any(|p| {
                glob::Pattern::new(p).map(|pat| pat.matches(&filename)).unwrap_or(false)
            });
            let path = PathBuf::from(format!("/drive/sub/{}", filename));
            prop_assert_eq!(is_excluded(&path, &patterns), matches_any);
        }
    }

    #[tokio::test]
    async fn test_process_local_event_ignores_excluded_file() {
        let db = Database::new_in_memory().unwrap();
        let qm = Arc::new(QueueManager::new(&db));
        let sm = Arc::new(StatusMonitor::default());
        let nd = NetworkDetector::new();
        let emitter = Arc::new(RecordingEmitter::default());

        let mut cfg = AppConfig::default();
        cfg.local_path = "/drive".to_string();
        cfg.exclusion_patterns = vec!["*.tmp".to_string(), ".DS_Store".to_string()];

        let engine = SyncEngine::new(cfg, None, db, Arc::clone(&qm), sm, nd, emitter);

        engine
            .process_local_event(LocalFsEvent::Created(PathBuf::from("/drive/cache.tmp")))
            .await
            .unwrap();
        assert_eq!(qm.count_pending().unwrap(), 0);

        engine
            .process_local_event(LocalFsEvent::Created(PathBuf::from("/drive/valid.txt")))
            .await
            .unwrap();
        assert_eq!(qm.count_pending().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_stop_waits_for_active_transfers() {
        let db = Database::new_in_memory().unwrap();
        let qm = Arc::new(QueueManager::new(&db));
        let sm = Arc::new(StatusMonitor::default());
        let nd = NetworkDetector::new();
        let emitter = Arc::new(RecordingEmitter::default());
        let engine = Arc::new(SyncEngine::new(
            AppConfig::default(),
            None,
            db,
            qm,
            sm,
            nd,
            emitter,
        ));

        // Simular transferência em andamento que termina após 60ms
        let active = engine.active_transfers_count();
        active.store(1, Ordering::SeqCst);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            active.store(0, Ordering::SeqCst);
        });

        engine.stop().await.unwrap();
        assert_eq!(engine.active_transfers_count().load(Ordering::SeqCst), 0);
    }
}
