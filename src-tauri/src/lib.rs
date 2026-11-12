mod buckets;
pub mod config_parser;
pub mod conflict_resolver;
mod costs;
pub mod credentials_manager;
pub mod db;
pub mod file_watcher;
pub mod models;
pub mod network_detector;
pub mod queue_manager;
pub mod s3_client;
mod site_publisher;
pub mod status_monitor;
pub mod sync_engine;
pub mod tray;

use crate::config_parser::ConfigParser;
use crate::credentials_manager::CredentialsManager;
use crate::db::Database;
use crate::models::{
    AppConfig, AwsCredentials, FileStatus, GlobalSyncState, ObjectDetailedMetadata, QueueStats,
    S3ObjectVersion, StorageAnalytics, ValidationError,
};
use crate::network_detector::NetworkDetector;
use crate::queue_manager::QueueManager;
use crate::s3_client::S3Client;
use crate::status_monitor::StatusMonitor;
use crate::sync_engine::{s3_key_to_local_path, SyncEngine, TauriEmitter};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Manager, State};

pub struct AppState {
    pub sync_engine: Arc<SyncEngine>,
    pub creds_manager: Arc<CredentialsManager>,
    pub config_parser: Arc<ConfigParser>,
    pub db: Database,
    buckets: Arc<buckets::BucketCatalog>,
    primary_lifecycle: tokio::sync::Mutex<()>,
    site_publication: tokio::sync::Mutex<()>,
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    if !state.config_parser.exists() {
        state
            .config_parser
            .create_empty()
            .map_err(|e| e.to_string())?;
    }
    state.config_parser.load().map_err(|e| e.to_string())
}

#[tauri::command]
fn validate_config(config: AppConfig) -> Vec<ValidationError> {
    match ConfigParser::validate(&config) {
        Ok(()) => Vec::new(),
        Err(errs) => errs,
    }
}

fn apply_os_autostart(enabled: bool) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let run_key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        if enabled {
            if let Ok(exe) = std::env::current_exe() {
                let exe_quoted = format!("\"{}\"", exe.display());
                let _ = std::process::Command::new("reg")
                    .args([
                        "add",
                        run_key,
                        "/v",
                        "AmazonS3Drive",
                        "/t",
                        "REG_SZ",
                        "/d",
                        &exe_quoted,
                        "/f",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .output();
            }
        } else {
            let _ = std::process::Command::new("reg")
                .args(["delete", run_key, "/v", "AmazonS3Drive", "/f"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
    }
}

#[tauri::command]
fn save_config(config: AppConfig, state: State<'_, AppState>) -> Result<(), String> {
    let _lifecycle = state.primary_lifecycle.try_lock()
        .map_err(|_| "Aguarde a mudança de estado da sincronização.".to_string())?;
    let _configuration = state.buckets.lock_configuration();
    if let Err(errs) = ConfigParser::validate(&config) {
        let msg = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }

    state.buckets.check_primary(&config)?;
    if state.sync_engine.is_running()
        && sync_engine::config_fingerprint(&config)
            != sync_engine::config_fingerprint(&state.sync_engine.get_config())
    {
        return Err("Pause a sincronização principal antes de mudar seu destino ou pasta.".into());
    }

    let local_dir = PathBuf::from(&config.local_path);
    if !local_dir.exists() {
        std::fs::create_dir_all(&local_dir).map_err(|e| {
            format!(
                "Caminho da pasta local inválido ou inacessível '{}': {}",
                config.local_path, e
            )
        })?;
    }

    state
        .config_parser
        .save(&config)
        .map_err(|e| e.to_string())?;
    apply_os_autostart(config.launch_on_startup);
    state.sync_engine.reload_config(config);
    Ok(())
}

#[tauri::command]
fn save_credentials(creds: AwsCredentials, state: State<'_, AppState>) -> Result<(), String> {
    if let Err(errs) = CredentialsManager::validate_format(&creds) {
        let msg = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }
    state.creds_manager.store(&creds).map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_connection(state: State<'_, AppState>) -> Result<(), String> {
    let config = state.config_parser.load().map_err(|e| e.to_string())?;
    let mut creds = if config.use_iam_role {
        AwsCredentials {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            session_token: None,
            region: config.aws_region.clone(),
            use_iam_role: true,
        }
    } else {
        state
            .creds_manager
            .load()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Credenciais AWS não configuradas.".to_string())?
    };
    creds.region = config.aws_region.clone();

    let client = S3Client::new_with_options(
        &creds,
        config.bucket_name,
        Some(state.db.clone()),
        config.endpoint_url,
        config.force_path_style,
    )
    .await
    .map_err(|e| e.to_string())?
    .with_storage_options(&config.storage_class, config.server_side_encryption);

    state
        .creds_manager
        .verify_access(&client)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_credentials(state: State<'_, AppState>) -> Result<(), String> {
    state.creds_manager.delete().map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_sync(state: State<'_, AppState>) -> Result<(), String> {
    let _lifecycle = state.primary_lifecycle.lock().await;
    if state.sync_engine.is_running() {
        return Ok(());
    }
    let config = state.config_parser.load().map_err(|e| e.to_string())?;
    {
        let _configuration = state.buckets.lock_configuration();
        state.buckets.check_primary(&config)?;
    }
    start_engine(&state.sync_engine, &state.creds_manager, config, &state.db).await
}

async fn start_engine(
    engine: &Arc<SyncEngine>,
    credentials: &CredentialsManager,
    config: AppConfig,
    db: &Database,
) -> Result<(), String> {
    if let Err(errs) = ConfigParser::validate(&config) {
        let msg = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }

    let s3_client = sync_client(credentials, &config, Some(db.clone())).await?;
    engine.reload_config(config);
    engine.set_s3_client(s3_client);

    if let Err(error) = engine.start().await {
        let _ = engine.stop().await;
        return Err(error.to_string());
    }
    Ok(())
}

async fn sync_client(
    credentials: &CredentialsManager,
    config: &AppConfig,
    db: Option<Database>,
) -> Result<S3Client, String> {
    let mut creds = if config.use_iam_role {
        AwsCredentials {
            access_key_id: String::new(),
            secret_access_key: String::new(),
            session_token: None,
            region: config.aws_region.clone(),
            use_iam_role: true,
        }
    } else {
        credentials
            .load()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "Credenciais AWS não configuradas.".to_string())?
    };

    creds.region = config.aws_region.clone();
    if let Err(errs) = CredentialsManager::validate_format(&creds) {
        let msg = errs
            .iter()
            .map(|e| format!("{}: {}", e.field, e.reason))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(msg);
    }

    let client = S3Client::new_with_options(
        &creds,
        config.bucket_name.clone(),
        db,
        config.endpoint_url.clone(),
        config.force_path_style,
    )
    .await
    .map_err(|e| e.to_string())?
    .with_storage_options(&config.storage_class, config.server_side_encryption)
    .with_bandwidth_limit(config.bandwidth_limit_kbps);

    Ok(client)
}

#[tauri::command]
async fn stop_sync(state: State<'_, AppState>) -> Result<(), String> {
    let _lifecycle = state.primary_lifecycle.lock().await;
    state.sync_engine.stop().await.map_err(|e| e.to_string())
}

#[tauri::command]
fn get_sync_status(state: State<'_, AppState>) -> Vec<FileStatus> {
    state.sync_engine.status_monitor().get_all()
}

#[tauri::command]
fn get_global_state(state: State<'_, AppState>) -> GlobalSyncState {
    state.sync_engine.status_monitor().global_state()
}

#[tauri::command]
fn get_sync_running(state: State<'_, AppState>) -> bool {
    state.sync_engine.is_running()
}

#[tauri::command]
fn get_queue_stats(state: State<'_, AppState>) -> Result<QueueStats, String> {
    state
        .sync_engine
        .queue_manager()
        .get_stats()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn resolve_conflict(
    strategy: String,
    local_path: String,
    conflict_path: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .sync_engine
        .resolve_conflict_action(&strategy, &local_path, &conflict_path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_deletion_reviews(
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::DeletionReview>, String> {
    state
        .sync_engine
        .deletion_reviews()
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn resolve_deletion(
    review: crate::models::DeletionReview,
    restore: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .sync_engine
        .resolve_deletion(&review, restore)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_path_dialog() -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$dlg = New-Object System.Windows.Forms.FolderBrowserDialog
$dlg.Description = "Selecione a pasta local para o Amazon S3 Drive"
$dlg.ShowNewFolderButton = $true
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $dlg.SelectedPath
}
"#;
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Sta", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|_| "Não foi possível abrir o seletor de pastas.")?;
        if !output.status.success() {
            return Err("Não foi possível abrir o seletor de pastas.".into());
        }
        let selected = String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        Ok((!selected.is_empty()).then_some(selected))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let default_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("S3Drive");
        Ok(Some(default_dir.to_string_lossy().to_string()))
    }
}

#[tauri::command]
fn open_local_folder(path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        std::fs::create_dir_all(&p)
            .map_err(|e| format!("Não foi possível criar/abrir a pasta '{}': {}", path, e))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&p)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn open_file_in_os(s3_key: String, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.sync_engine.get_config();
    let local_path = s3_key_to_local_path(&s3_key, &PathBuf::from(&cfg.local_path));
    if !local_path.exists() {
        return Err(format!(
            "O arquivo local ainda não existe em '{}'.",
            local_path.display()
        ));
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(&local_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&local_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&local_path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn reveal_file_in_folder(s3_key: String, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.sync_engine.get_config();
    let local_path = s3_key_to_local_path(&s3_key, &PathBuf::from(&cfg.local_path));
    #[cfg(target_os = "windows")]
    {
        if local_path.exists() {
            std::process::Command::new("explorer")
                .arg(format!("/select,{}", local_path.display()))
                .spawn()
                .map_err(|e| e.to_string())?;
        } else if let Some(parent) = local_path.parent() {
            std::process::Command::new("explorer")
                .arg(parent)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(parent) = local_path.parent() {
            let _ = open_local_folder(parent.to_string_lossy().to_string());
        }
    }
    Ok(())
}

#[tauri::command]
async fn retry_file_sync(s3_key: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .sync_engine
        .retry_file(&s3_key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn retry_all_errors(state: State<'_, AppState>) -> Result<usize, String> {
    state
        .sync_engine
        .retry_all_failed()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_presigned_url(
    s3_key: String,
    expires_in_secs: Option<u64>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .sync_engine
        .presign_object_url(&s3_key, expires_in_secs.unwrap_or(3600))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_sync_logs(max_lines: Option<usize>, state: State<'_, AppState>) -> Vec<String> {
    state
        .sync_engine
        .status_monitor()
        .read_logs(max_lines.unwrap_or(500))
}

#[tauri::command]
fn clear_sync_logs(state: State<'_, AppState>) -> Result<(), String> {
    state.sync_engine.status_monitor().clear_logs()
}

#[tauri::command]
fn open_files_dialog() -> Result<Vec<String>, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Title = "Selecione arquivo(s) para importar no Amazon S3 Drive"
$dlg.Multiselect = $true
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    $dlg.FileNames | ForEach-Object { Write-Output $_ }
}
"#;
        if let Ok(output) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Sta", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect();
            return Ok(files);
        }
    }
    Ok(Vec::new())
}

#[tauri::command]
async fn import_files_to_drive(
    paths: Vec<String>,
    target_subfolder: Option<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .sync_engine
        .import_external_files(&paths, target_subfolder.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_object_details(
    s3_key: String,
    state: State<'_, AppState>,
) -> Result<ObjectDetailedMetadata, String> {
    state
        .sync_engine
        .get_object_metadata(&s3_key)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_object_versions(
    s3_key: String,
    state: State<'_, AppState>,
) -> Result<Vec<S3ObjectVersion>, String> {
    state
        .sync_engine
        .list_versions_for_key(&s3_key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn restore_object_version(
    s3_key: String,
    version_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .sync_engine
        .restore_version_for_key(&s3_key, &version_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn create_subfolder(
    parent_prefix: Option<String>,
    folder_name: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .sync_engine
        .create_local_subfolder(parent_prefix.as_deref(), &folder_name)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn delete_object_sync(s3_key: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .sync_engine
        .delete_file_and_sync(&s3_key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn export_config_profile(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let cfg = state.config_parser.load().map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.SaveFileDialog
$dlg.Title = "Exportar Perfil de Configuração do Amazon S3 Drive"
$dlg.Filter = "JSON Profile (*.json)|*.json"
$dlg.FileName = "s3-drive-profile.json"
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $dlg.FileName
}
"#;
        if let Ok(output) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Sta", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !target.is_empty() {
                std::fs::write(&target, json.as_bytes()).map_err(|e| e.to_string())?;
                return Ok(Some(target));
            }
            return Ok(None);
        }
    }

    let fallback = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("s3-drive-profile.json");
    std::fs::write(&fallback, json.as_bytes()).map_err(|e| e.to_string())?;
    Ok(Some(fallback.to_string_lossy().to_string()))
}

#[tauri::command]
fn import_config_profile() -> Result<Option<AppConfig>, String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Title = "Importar Perfil de Configuração do Amazon S3 Drive"
$dlg.Filter = "JSON Profile (*.json)|*.json"
if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
    Write-Output $dlg.FileName
}
"#;
        if let Ok(output) = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Sta", "-Command", script])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            let source = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if source.is_empty() {
                return Ok(None);
            }
            let raw = std::fs::read_to_string(&source).map_err(|e| e.to_string())?;
            let mut imported: AppConfig =
                serde_json::from_str(&raw).map_err(|e| format!("JSON inválido: {}", e))?;
            imported.notifications.error_enabled = true;
            ConfigParser::validate(&imported).map_err(|errors| {
                errors
                    .iter()
                    .map(|error| error.reason.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            })?;
            return Ok(Some(imported));
        }
    }
    Ok(None)
}

#[tauri::command]
async fn rename_or_move_object(
    old_s3_key: String,
    new_s3_key: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .sync_engine
        .rename_or_move_file(&old_s3_key, &new_s3_key)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn batch_delete_objects(
    s3_keys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .sync_engine
        .batch_delete_files(&s3_keys)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn batch_resync_objects(
    s3_keys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    state
        .sync_engine
        .batch_resync_files(&s3_keys)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn batch_presigned_urls(
    s3_keys: Vec<String>,
    expires_in_secs: Option<u64>,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state
        .sync_engine
        .batch_presign_urls(&s3_keys, expires_in_secs.unwrap_or(3600))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_storage_analytics(state: State<'_, AppState>) -> Result<StorageAnalytics, String> {
    Ok(state.sync_engine.compute_storage_analytics())
}

#[tauri::command]
async fn get_s3_public_prices(state: State<'_, AppState>) -> Result<costs::PublicPrices, String> {
    let config = state.sync_engine.get_config();
    if config.endpoint_url.as_deref().is_some_and(|url| !url.trim().is_empty()) {
        return Err("Tarifas AWS indisponíveis para endpoints personalizados.".into());
    }
    costs::public_prices(&config.aws_region, &config.storage_class).await
}

#[tauri::command]
async fn get_s3_monthly_spend(state: State<'_, AppState>) -> Result<costs::MonthlySpend, String> {
    let config = state.sync_engine.get_config();
    if config.endpoint_url.as_deref().is_some_and(|url| !url.trim().is_empty()) {
        return Err("Faturamento AWS indisponível para endpoints personalizados.".into());
    }
    let creds = if config.use_iam_role {
        None
    } else {
        Some(
            state
                .creds_manager
                .load()
                .map_err(|_| "Credenciais AWS indisponíveis.")?
                .ok_or("Credenciais AWS não configuradas.")?,
        )
    };
    costs::monthly_spend(creds).await
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config_parser = Arc::new(ConfigParser::default_location());
            if !config_parser.exists() {
                let _ = config_parser.create_empty();
            }
            let initial_config = config_parser.load().unwrap_or_default();

            let app_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("amazon-s3-drive");
            let _ = std::fs::create_dir_all(&app_dir);
            let db_path = app_dir.join("sync.db");
            let db = Database::new(&db_path).expect("Failed to initialize SQLite database");

            let queue_manager = Arc::new(QueueManager::new(&db));
            let network_detector = NetworkDetector::new();
            let status_monitor = Arc::new(StatusMonitor::new(
                Some(network_detector.clone()),
                Some(app_dir.clone()),
            ));
            let creds_manager = Arc::new(CredentialsManager::new());

            let emitter = Arc::new(TauriEmitter {
                app_handle: app.handle().clone(),
            });

            let sync_engine = Arc::new(SyncEngine::new(
                initial_config,
                None,
                db.clone(),
                queue_manager,
                Arc::clone(&status_monitor),
                network_detector,
                emitter,
            ));

            let _ = tray::setup_system_tray(app.handle(), status_monitor);

            let buckets = Arc::new(buckets::BucketCatalog::new(db.clone(), app_dir)?);
            buckets.restore_enabled(Arc::clone(&config_parser), Arc::clone(&creds_manager))?;
            app.manage(AppState {
                sync_engine,
                creds_manager,
                config_parser,
                db,
                buckets,
                primary_lifecycle: tokio::sync::Mutex::new(()),
                site_publication: tokio::sync::Mutex::new(()),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            validate_config,
            save_credentials,
            test_connection,
            delete_credentials,
            start_sync,
            stop_sync,
            get_sync_status,
            get_global_state,
            get_sync_running,
            get_queue_stats,
            resolve_conflict,
            get_deletion_reviews,
            resolve_deletion,
            open_path_dialog,
            open_local_folder,
            open_file_in_os,
            reveal_file_in_folder,
            retry_file_sync,
            retry_all_errors,
            get_presigned_url,
            get_sync_logs,
            clear_sync_logs,
            open_files_dialog,
            import_files_to_drive,
            get_object_details,
            list_object_versions,
            restore_object_version,
            create_subfolder,
            delete_object_sync,
            export_config_profile,
            import_config_profile,
            rename_or_move_object,
            batch_delete_objects,
            batch_resync_objects,
            batch_presigned_urls,
            get_storage_analytics,
            get_s3_public_prices,
            get_s3_monthly_spend,
            buckets::list_sync_buckets,
            buckets::discover_s3_buckets,
            buckets::create_s3_bucket,
            buckets::add_sync_bucket,
            buckets::set_bucket_sync_enabled,
            buckets::get_bucket_details,
            buckets::get_bucket_deletions,
            buckets::resolve_bucket_deletion,
            buckets::resolve_bucket_conflict,
            site_publisher::list_published_sites,
            site_publisher::publish_site,
            site_publisher::list_site_releases,
            site_publisher::restore_site_release,
            site_publisher::sources::browse_site_source,
            site_publisher::sources::preview_site_source,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
