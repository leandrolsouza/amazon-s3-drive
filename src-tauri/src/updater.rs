use crate::language::tr;
use crate::{start_engine, AppState};
use std::{sync::Arc, time::Duration};
use tauri::{
    utils::{config::BundleType, platform::bundle_type},
    AppHandle, State,
};
use tauri_plugin_updater::{Update, UpdaterExt};

async fn available_update(app: &AppHandle) -> Result<Option<Update>, String> {
    let supported = matches!(
        bundle_type(),
        Some(BundleType::Msi | BundleType::Nsis | BundleType::App | BundleType::AppImage)
    );
    let configured = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    if cfg!(debug_assertions) || !supported || !configured {
        return Ok(None);
    }

    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|err| err.to_string())?;
    updater.check().await.map_err(|err| err.to_string())
}

#[tauri::command]
pub(crate) async fn check_app_update(app: AppHandle) -> Result<Option<String>, String> {
    Ok(available_update(&app).await?.map(|update| update.version))
}

#[tauri::command]
pub(crate) async fn install_app_update(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), String> {
    let _installation = state
        .update_installation
        .try_lock()
        .map_err(|_| tr("An update is already in progress.", "Já existe uma atualização em andamento."))?;
    let mut update = available_update(&app)
        .await?
        .filter(|update| update.version == version)
        .ok_or(tr("The advertised version changed. Reopen the app to check for updates.", "A versão anunciada mudou. Abra o aplicativo novamente para verificar."))?;
    if update.download_url.scheme() != "https" {
        return Err(tr("The update package must be downloaded over HTTPS.", "O pacote de atualização deve ser baixado via HTTPS.").into());
    }
    update.timeout = Some(Duration::from_secs(300));
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|err| err.to_string())?;

    let _site = state.site_publication.lock().await;
    let _primary = state.primary_lifecycle.lock().await;
    let _buckets = state.buckets.lifecycle.write().await;
    let primary_running = state.sync_engine.is_running();
    let result = async {
        state
            .sync_engine
            .stop()
            .await
            .map_err(|err| err.to_string())?;
        state.buckets.stop_for_update().await?;
        update.install(bytes).map_err(|err| err.to_string())
    }
    .await;

    if let Err(mut error) = result {
        if primary_running {
            let config = state.sync_engine.get_config();
            if let Err(err) =
                start_engine(&state.sync_engine, &state.creds_manager, config, &state.db).await
            {
                error.push_str(&crate::language::localized_format!(" Could not resume the primary bucket: {err}", " Falha ao retomar o bucket principal: {err}"));
            }
        }
        if let Err(err) = state.buckets.restore_enabled(
            Arc::clone(&state.config_parser),
            Arc::clone(&state.creds_manager),
        ) {
            error.push_str(&crate::language::localized_format!(" Could not resume the additional buckets: {err}", " Falha ao retomar os buckets adicionais: {err}"));
        }
        return Err(error);
    }

    // No Windows, o instalador do Tauri encerra e reabre o aplicativo.
    #[cfg(not(target_os = "windows"))]
    app.restart();
    #[cfg(target_os = "windows")]
    Ok(())
}
