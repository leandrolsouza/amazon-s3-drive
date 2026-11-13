use super::*;
use crate::models::{ConflictEvent, DeletionReview};
use crate::sync_engine::EventEmitter;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

#[derive(Deserialize)]
pub struct BucketInput {
    bucket_name: String,
    aws_region: String,
    local_path: String,
    s3_prefix: Option<String>,
}

struct BucketProfile {
    id: i64,
    config: AppConfig,
    enabled: bool,
}

#[derive(Serialize)]
pub struct BucketSummary {
    id: i64,
    bucket_name: String,
    aws_region: String,
    local_path: String,
    s3_prefix: Option<String>,
    enabled: bool,
    running: bool,
    busy: bool,
    state: GlobalSyncState,
    error: Option<String>,
}

#[derive(Serialize)]
pub struct BucketDetails {
    files: Vec<FileStatus>,
    logs: Vec<String>,
    conflicts: Vec<ConflictEvent>,
}

#[derive(Default)]
struct BucketEvents {
    conflicts: Mutex<BTreeMap<String, ConflictEvent>>,
}

impl EventEmitter for BucketEvents {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        if event == "conflict:detected" {
            if let Ok(conflict) = serde_json::from_value::<ConflictEvent>(payload) {
                self.conflicts
                    .lock()
                    .unwrap()
                    .insert(conflict.conflict_copy_path.clone(), conflict);
            }
        }
    }
}

struct BucketRuntime {
    engine: Arc<SyncEngine>,
    db: Database,
    events: Arc<BucketEvents>,
    lifecycle: tokio::sync::Mutex<()>,
    error: Mutex<Option<String>>,
}

pub(crate) struct BucketCatalog {
    db: Database,
    app_dir: PathBuf,
    configuration: Mutex<()>,
    runtimes: Mutex<HashMap<i64, Arc<BucketRuntime>>>,
    pub(crate) lifecycle: tokio::sync::RwLock<()>,
}

impl BucketCatalog {
    pub(crate) fn new(db: Database, app_dir: PathBuf) -> Result<Self, rusqlite::Error> {
        db.connection().lock().unwrap().execute_batch(
            "CREATE TABLE IF NOT EXISTS bucket_profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                config TEXT NOT NULL,
                enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
            );",
        )?;
        Ok(Self {
            db,
            app_dir,
            configuration: Mutex::new(()),
            runtimes: Mutex::new(HashMap::new()),
            lifecycle: tokio::sync::RwLock::new(()),
        })
    }

    pub(crate) fn lock_configuration(&self) -> MutexGuard<'_, ()> {
        self.configuration.lock().unwrap()
    }

    fn profiles(&self) -> Result<Vec<BucketProfile>, String> {
        let connection = self.db.connection();
        let conn = connection.lock().unwrap();
        let mut query = conn
            .prepare("SELECT id, config, enabled FROM bucket_profiles ORDER BY id")
            .map_err(|err| err.to_string())?;
        let rows = query
            .query_map([], |row| {
                Ok((row.get(0)?, row.get::<_, String>(1)?, row.get(2)?))
            })
            .map_err(|err| err.to_string())?;
        rows.map(|row| {
            let (id, json, enabled) = row.map_err(|err| err.to_string())?;
            let config = serde_json::from_str(&json).map_err(|err| err.to_string())?;
            Ok(BucketProfile {
                id,
                config,
                enabled,
            })
        })
        .collect()
    }

    fn profile(&self, id: i64) -> Result<BucketProfile, String> {
        self.profiles()?
            .into_iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| "Bucket de sincronização não encontrado.".into())
    }

    fn insert(&self, config: &AppConfig) -> Result<i64, String> {
        let json = serde_json::to_string(config).map_err(|err| err.to_string())?;
        let connection = self.db.connection();
        let conn = connection.lock().unwrap();
        conn.execute(
            "INSERT INTO bucket_profiles (config, enabled) VALUES (?1, 0)",
            [json],
        )
        .map_err(|err| err.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    fn runtime(&self, profile: &BucketProfile) -> Result<Arc<BucketRuntime>, String> {
        let mut runtimes = self.runtimes.lock().unwrap();
        if let Some(runtime) = runtimes.get(&profile.id) {
            return Ok(Arc::clone(runtime));
        }

        let dir = self.app_dir.join("buckets").join(profile.id.to_string());
        std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
        let db = Database::new(&dir.join("sync.db")).map_err(|err| err.to_string())?;
        let network = NetworkDetector::new();
        let monitor = Arc::new(StatusMonitor::new(Some(network.clone()), Some(dir)));
        let events = Arc::new(BucketEvents::default());
        let engine = Arc::new(SyncEngine::new(
            profile.config.clone(),
            None,
            db.clone(),
            Arc::new(QueueManager::new(&db)),
            monitor,
            network,
            events.clone(),
        ));
        let runtime = Arc::new(BucketRuntime {
            engine,
            db,
            events,
            lifecycle: tokio::sync::Mutex::new(()),
            error: Mutex::new(None),
        });
        runtimes.insert(profile.id, Arc::clone(&runtime));
        Ok(runtime)
    }

    pub(crate) fn check_primary(&self, config: &AppConfig) -> Result<(), String> {
        for profile in self.profiles()? {
            check_disjoint(config, &profile.config)?;
        }
        Ok(())
    }

    fn check_additional(
        &self,
        config: &AppConfig,
        primary: &AppConfig,
        id: Option<i64>,
    ) -> Result<(), String> {
        if paths_overlap(Path::new(&config.local_path), &self.app_dir)? {
            return Err("Escolha uma pasta fora do diretório de dados do aplicativo.".into());
        }
        check_disjoint(config, primary)?;
        for profile in self
            .profiles()?
            .into_iter()
            .filter(|profile| Some(profile.id) != id)
        {
            check_disjoint(config, &profile.config)?;
        }
        Ok(())
    }

    async fn set_running(
        &self,
        id: i64,
        enabled: bool,
        restore: bool,
        parser: &ConfigParser,
        credentials: &CredentialsManager,
    ) -> Result<(), String> {
        let _catalog = self.lifecycle.read().await;
        let profile = self.profile(id)?;
        let runtime = self.runtime(&profile)?;
        let _lifecycle = runtime.lifecycle.lock().await;
        if restore && !self.profile(id)?.enabled {
            return Ok(());
        }
        let result = async {
            if !restore {
                self.db
                    .connection()
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE bucket_profiles SET enabled = ?1 WHERE id = ?2",
                        rusqlite::params![enabled, id],
                    )
                    .map_err(|err| err.to_string())?;
            }
            if !enabled {
                return runtime.engine.stop().await.map_err(|err| err.to_string());
            }
            if runtime.engine.is_running() {
                return Ok(());
            }
            let mut config = profile.config;
            {
                let _configuration = self.lock_configuration();
                let primary = parser.load().map_err(|err| err.to_string())?;
                self.check_additional(&config, &primary, Some(id))?;
                config.use_iam_role = primary.use_iam_role;
            }
            start_engine(&runtime.engine, credentials, config, &runtime.db).await
        }
        .await;
        *runtime.error.lock().unwrap() = result.as_ref().err().cloned();
        result
    }

    pub(crate) async fn stop_for_update(&self) -> Result<(), String> {
        let runtimes: Vec<_> = self.runtimes.lock().unwrap().values().cloned().collect();
        for runtime in runtimes {
            runtime.engine.stop().await.map_err(|err| err.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn restore_enabled(
        self: &Arc<Self>,
        parser: Arc<ConfigParser>,
        credentials: Arc<CredentialsManager>,
    ) -> Result<(), String> {
        for profile in self
            .profiles()?
            .into_iter()
            .filter(|profile| profile.enabled)
        {
            let catalog = Arc::clone(self);
            let parser = Arc::clone(&parser);
            let credentials = Arc::clone(&credentials);
            tauri::async_runtime::spawn(async move {
                let _ = catalog
                    .set_running(profile.id, true, true, &parser, &credentials)
                    .await;
            });
        }
        Ok(())
    }
}

fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            // Resolve aliases in existing ancestors even if the selected folder is new.
            if std::fs::symlink_metadata(path).is_ok() {
                return Err("A pasta de sincronização aponta para um local indisponível.".into());
            }
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .ok_or_else(|| "A pasta de sincronização está indisponível.".to_string())?;
            let mut resolved = resolved_path(parent)?;
            match path.components().next_back() {
                Some(std::path::Component::ParentDir) => {
                    resolved.pop();
                }
                Some(std::path::Component::Normal(name)) => resolved.push(name),
                _ => {}
            }
            Ok(resolved)
        }
        Err(err) => Err(format!(
            "Não foi possível verificar a pasta de sincronização. {err}"
        )),
    }
}

fn paths_overlap(first: &Path, second: &Path) -> Result<bool, String> {
    let first = resolved_path(first)?;
    let second = resolved_path(second)?;
    #[cfg(windows)]
    let (first, second) = (
        PathBuf::from(first.to_string_lossy().to_lowercase()),
        PathBuf::from(second.to_string_lossy().to_lowercase()),
    );
    Ok(first.starts_with(&second) || second.starts_with(&first))
}

fn check_disjoint(config: &AppConfig, other: &AppConfig) -> Result<(), String> {
    if config.bucket_name.trim() == other.bucket_name.trim() {
        return Err("Esse bucket já está configurado para sincronização.".into());
    }
    if !other.local_path.trim().is_empty()
        && paths_overlap(Path::new(&config.local_path), Path::new(&other.local_path))?
    {
        return Err("Escolha uma pasta local que não coincida com outra pasta sincronizada nem esteja dentro dela.".into());
    }
    Ok(())
}

fn validate(config: &AppConfig) -> Result<(), String> {
    ConfigParser::validate(config).map_err(|errors| {
        errors
            .into_iter()
            .map(|error| error.reason)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[tauri::command]
pub fn list_sync_buckets(state: State<'_, AppState>) -> Result<Vec<BucketSummary>, String> {
    state
        .buckets
        .profiles()?
        .into_iter()
        .map(|profile| {
            let config = &profile.config;
            let (running, busy, global, error) = match state.buckets.runtime(&profile) {
                Ok(runtime) => (
                    runtime.engine.is_running(),
                    runtime.lifecycle.try_lock().is_err(),
                    runtime.engine.status_monitor().global_state(),
                    runtime.error.lock().unwrap().clone(),
                ),
                Err(error) => (false, false, GlobalSyncState::Error, Some(error)),
            };
            Ok(BucketSummary {
                id: profile.id,
                bucket_name: config.bucket_name.clone(),
                aws_region: config.aws_region.clone(),
                local_path: config.local_path.clone(),
                s3_prefix: config.s3_prefix.clone(),
                enabled: profile.enabled,
                running,
                busy,
                state: global,
                error,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn discover_s3_buckets(
    state: State<'_, AppState>,
) -> Result<Vec<(String, Option<String>)>, String> {
    let config = state.config_parser.load().map_err(|err| err.to_string())?;
    sync_client(&state.creds_manager, &config, None)
        .await?
        .discover_buckets()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn create_s3_bucket(
    bucket_name: String,
    aws_region: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut config = state.config_parser.load().map_err(|err| err.to_string())?;
    if config
        .endpoint_url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
    {
        return Err(
            "A criação de buckets nesta tela está disponível para o endpoint padrão da AWS.".into(),
        );
    }
    config.bucket_name = bucket_name.trim().into();
    config.aws_region = aws_region.trim().into();
    config.local_path = ".".into();
    validate(&config)?;
    let client = sync_client(&state.creds_manager, &config, None).await?;
    client
        .create_private_bucket(&config.aws_region)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn add_sync_bucket(
    input: BucketInput,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    let primary = state.config_parser.load().map_err(|err| err.to_string())?;
    let mut config = AppConfig {
        bucket_name: input.bucket_name.trim().into(),
        aws_region: input.aws_region.trim().into(),
        local_path: input.local_path.trim().into(),
        s3_prefix: input
            .s3_prefix
            .map(|prefix| prefix.trim().trim_matches('/').to_owned())
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| format!("{prefix}/")),
        use_iam_role: primary.use_iam_role,
        endpoint_url: primary.endpoint_url.clone(),
        force_path_style: primary.force_path_style,
        ..AppConfig::default()
    };
    validate(&config)?;
    let local_path = Path::new(&config.local_path);
    if !local_path.is_absolute() {
        return Err("Selecione o caminho completo da pasta local.".into());
    }
    {
        let _configuration = state.buckets.lock_configuration();
        state.buckets.check_additional(&config, &primary, None)?;
    }
    let client = sync_client(&state.creds_manager, &config, None).await?;
    client
        .test_prefix_access(config.s3_prefix.as_deref())
        .await
        .map_err(|err| err.to_string())?;
    let _configuration = state.buckets.lock_configuration();
    let current = state.config_parser.load().map_err(|err| err.to_string())?;
    if primary.endpoint_url != current.endpoint_url
        || primary.use_iam_role != current.use_iam_role
        || primary.force_path_style != current.force_path_style
    {
        return Err("As configurações da conexão mudaram. Tente adicionar novamente.".into());
    }
    state.buckets.check_additional(&config, &current, None)?;
    std::fs::create_dir_all(local_path).map_err(|err| err.to_string())?;
    config.local_path = local_path
        .canonicalize()
        .map_err(|err| err.to_string())?
        .into_os_string()
        .into_string()
        .map_err(|_| "Caminho local inválido.".to_string())?;
    state.buckets.check_additional(&config, &current, None)?;
    state.buckets.insert(&config)
}

#[tauri::command]
pub async fn set_bucket_sync_enabled(
    id: i64,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .buckets
        .set_running(
            id,
            enabled,
            false,
            &state.config_parser,
            &state.creds_manager,
        )
        .await
}

#[tauri::command]
pub fn get_bucket_details(id: i64, state: State<'_, AppState>) -> Result<BucketDetails, String> {
    let runtime = state.buckets.runtime(&state.buckets.profile(id)?)?;
    let conflicts = runtime
        .events
        .conflicts
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    Ok(BucketDetails {
        files: runtime.engine.status_monitor().get_all(),
        logs: runtime.engine.status_monitor().read_logs(100),
        conflicts,
    })
}

#[tauri::command]
pub async fn resolve_bucket_deletion(
    id: i64,
    review: DeletionReview,
    restore: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let runtime = state.buckets.runtime(&state.buckets.profile(id)?)?;
    let _lifecycle = runtime.lifecycle.lock().await;
    if !runtime.engine.is_running() {
        return Err("Inicie a sincronização desse bucket para revisar a exclusão.".into());
    }
    runtime
        .engine
        .resolve_deletion(&review, restore)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn get_bucket_deletions(
    id: i64,
    state: State<'_, AppState>,
) -> Result<Vec<DeletionReview>, String> {
    state
        .buckets
        .runtime(&state.buckets.profile(id)?)?
        .engine
        .deletion_reviews()
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn resolve_bucket_conflict(
    id: i64,
    path: String,
    strategy: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let runtime = state.buckets.runtime(&state.buckets.profile(id)?)?;
    let _lifecycle = runtime.lifecycle.lock().await;
    let mut conflicts = runtime.events.conflicts.lock().unwrap();
    let conflict = conflicts
        .get(&path)
        .ok_or_else(|| "Conflito não encontrado.".to_string())?;
    runtime
        .engine
        .resolve_conflict_action(
            &strategy,
            &conflict.local_path,
            &conflict.conflict_copy_path,
        )
        .map_err(|err| err.to_string())?;
    conflicts.remove(&path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{SyncOperation, SyncOperationType};

    #[test]
    fn rejects_nested_folders_and_duplicate_buckets() {
        let root = tempfile::tempdir().unwrap();
        let primary = AppConfig {
            bucket_name: "primary".into(),
            local_path: root.path().join("primary").to_str().unwrap().into(),
            ..AppConfig::default()
        };
        let mut secondary = AppConfig {
            bucket_name: "secondary".into(),
            local_path: root
                .path()
                .join("primary")
                .join("nested")
                .to_str()
                .unwrap()
                .into(),
            ..AppConfig::default()
        };
        assert!(check_disjoint(&secondary, &primary).is_err());
        secondary.local_path = root.path().join("primary-other").to_str().unwrap().into();
        assert!(check_disjoint(&secondary, &primary).is_ok());
        secondary.bucket_name = primary.bucket_name.clone();
        assert!(check_disjoint(&secondary, &primary).is_err());
    }

    #[test]
    fn keeps_legacy_queue_and_isolates_identical_keys_across_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("sync.db");
        let legacy_db = Database::new(&db_path).unwrap();
        let operation = SyncOperation {
            operation_type: SyncOperationType::Upload,
            s3_key: "same.txt".into(),
            local_path: "same.txt".into(),
            extra: None,
        };
        let legacy_queue = QueueManager::new(&legacy_db);
        legacy_queue.enqueue(operation.clone()).unwrap();
        let catalog = BucketCatalog::new(legacy_db, dir.path().to_owned()).unwrap();
        let first = catalog
            .insert(&AppConfig {
                bucket_name: "first".into(),
                ..AppConfig::default()
            })
            .unwrap();
        let second = catalog
            .insert(&AppConfig {
                bucket_name: "second".into(),
                ..AppConfig::default()
            })
            .unwrap();
        let first_runtime = catalog.runtime(&catalog.profile(first).unwrap()).unwrap();
        let second_runtime = catalog.runtime(&catalog.profile(second).unwrap()).unwrap();
        let first_queue = first_runtime.engine.queue_manager();
        let second_queue = second_runtime.engine.queue_manager();
        first_queue.enqueue(operation.clone()).unwrap();
        second_queue.enqueue(operation).unwrap();
        let item = first_queue.dequeue_next().unwrap().unwrap();
        first_queue.mark_completed(item.id).unwrap();
        assert_eq!(first_queue.get_stats().unwrap().pending_count, 0);
        assert_eq!(second_queue.get_stats().unwrap().pending_count, 1);
        assert_eq!(legacy_queue.get_stats().unwrap().pending_count, 1);
        drop(catalog);

        let reopened =
            BucketCatalog::new(Database::new(&db_path).unwrap(), dir.path().to_owned()).unwrap();
        let second_profile = reopened.profile(second).unwrap();
        assert_eq!(second_profile.config.bucket_name, "second");
        assert!(!second_profile.enabled);
        let runtime = reopened.runtime(&second_profile).unwrap();
        assert_eq!(
            runtime
                .engine
                .queue_manager()
                .get_stats()
                .unwrap()
                .pending_count,
            1
        );
    }

    #[tokio::test]
    async fn synchronizes_two_buckets_and_pauses_only_one() {
        let root = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let catalog =
            BucketCatalog::new(Database::new_in_memory().unwrap(), root.path().join("data"))
                .unwrap();
        let mut runtimes = Vec::new();
        for name in ["first", "second"] {
            let folder = root.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            std::fs::write(folder.join("same.txt"), name).unwrap();
            let config = AppConfig {
                bucket_name: name.into(),
                local_path: folder.to_str().unwrap().into(),
                s3_prefix: Some("shared/".into()),
                aws_region: "sa-east-1".into(),
                endpoint_url: Some(endpoint.clone()),
                ..AppConfig::default()
            };
            let id = catalog.insert(&config).unwrap();
            let runtime = catalog.runtime(&catalog.profile(id).unwrap()).unwrap();
            let (client, remote) = S3Client::new_mock(name, Some(runtime.db.clone()));
            runtime.engine.set_s3_client(client);
            runtimes.push((runtime, remote));
        }
        let [(first, first_remote), (second, second_remote)] = runtimes.as_slice() else {
            panic!("Expected two configured buckets");
        };
        let (first_started, second_started) =
            tokio::join!(first.engine.start(), second.engine.start());
        first_started.unwrap();
        second_started.unwrap();
        assert_eq!(
            first_remote.lock().unwrap().objects["shared/same.txt"].bytes,
            b"first"
        );
        assert_eq!(
            second_remote.lock().unwrap().objects["shared/same.txt"].bytes,
            b"second"
        );

        first.engine.stop().await.unwrap();
        assert!(!first.engine.is_running());
        assert!(second.engine.is_running());
        first.engine.start().await.unwrap();
        assert!(first.engine.is_running());
        catalog
            .db
            .connection()
            .lock()
            .unwrap()
            .execute("UPDATE bucket_profiles SET enabled = 1", [])
            .unwrap();
        let _lifecycle = catalog.lifecycle.write().await;
        catalog.stop_for_update().await.unwrap();
        assert!(!first.engine.is_running());
        assert!(!second.engine.is_running());
        assert!(catalog
            .profiles()
            .unwrap()
            .iter()
            .all(|profile| profile.enabled));
    }
}
