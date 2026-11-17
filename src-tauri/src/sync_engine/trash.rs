use super::*;
use crate::models::{TrashEntry, TrashListing, TrashRestore, TrashSource};
use crate::s3_client::transfer_temp_file;
use serde::Deserialize;

use super::deletion_review::TRASH;

const METADATA: &str = "entry.json";
const CONTENT: &str = "content";
const MAX_SINGLE_COPY_BYTES: u64 = 5_000_000_000;

#[derive(Serialize, Deserialize)]
struct LocalTrashMetadata {
    scope: String,
    original_key: String,
}

fn io_error(err: std::io::Error) -> SyncError {
    SyncError::Io(err.to_string())
}

fn trash_stamp() -> String {
    Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_default()
        .to_string()
}

fn deleted_at(stamp: &str) -> Option<DateTime<Utc>> {
    stamp
        .parse::<i64>()
        .ok()
        .map(DateTime::from_timestamp_nanos)
}

fn trash_prefix(cfg: &AppConfig) -> String {
    apply_s3_prefix(TRASH.to_owned(), &cfg.s3_prefix)
}

fn missing_entry() -> SyncError {
    SyncError::Conflict(
        tr(
            "This trash entry is missing or invalid.",
            "Este item da lixeira não está disponível ou é inválido.",
        )
        .into(),
    )
}

fn regular_file(path: &Path) -> std::io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !is_link(&meta) => Ok(Some(meta)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

impl SyncEngine {
    pub(super) fn move_to_local_trash(
        &self,
        cfg: &AppConfig,
        key: &str,
        path: &Path,
    ) -> Result<(), SyncError> {
        let base = Path::new(&cfg.local_path);
        require_local_path(path, base)?;
        if regular_file(path).map_err(io_error)?.is_none() {
            return Err(SyncError::Conflict(
                tr(
                    "Only regular files can be moved to the trash.",
                    "Apenas arquivos comuns podem ser movidos para a lixeira.",
                )
                .into(),
            ));
        }
        let root = base.join(TRASH);
        require_local_path(&root, base)?;
        fs::create_dir_all(&root).map_err(io_error)?;
        let folder = root.join(trash_stamp());
        fs::create_dir(&folder).map_err(io_error)?;
        let content = folder.join(CONTENT);
        let metadata = folder.join(METADATA);
        let result = (|| {
            let name = path.file_name().ok_or_else(missing_entry)?;
            let stored_path = content.join(name);
            fs::create_dir(&content).map_err(io_error)?;
            let record = LocalTrashMetadata {
                scope: config_fingerprint(cfg),
                original_key: key.to_owned(),
            };
            let bytes =
                serde_json::to_vec(&record).map_err(|err| SyncError::Io(err.to_string()))?;
            fs::write(&metadata, bytes).map_err(io_error)?;
            fs::rename(path, stored_path).map_err(io_error)
        })();
        if result.is_err() {
            let _ = fs::remove_file(metadata);
            let _ = fs::remove_dir(content);
            let _ = fs::remove_dir(folder);
        }
        result
    }

    pub(super) async fn trash_remote_revision(
        &self,
        cfg: &AppConfig,
        client: &S3Client,
        key: &str,
        etag: &str,
    ) -> Result<(), S3Error> {
        let relative = scoped_key(key, cfg).map_err(|err| S3Error::Sdk(err.to_string()))?;
        if relative.ends_with('/') {
            return client.delete_object_if_match(key, etag).await;
        }
        let backup = format!("{}/{}/{relative}", trash_prefix(cfg), trash_stamp());
        let remote = client.head_object(key).await?;
        if remote.etag != etag.trim_matches('"') {
            return Err(S3Error::PreconditionFailed);
        }
        if remote.size > MAX_SINGLE_COPY_BYTES {
            let base = Path::new(&cfg.local_path);
            require_local_path(base, base).map_err(|err| S3Error::Io(err.to_string()))?;
            let (temp, downloaded_etag) = client.download_to_temp(key, &base.join(TRASH)).await?;
            if downloaded_etag != remote.etag {
                return Err(S3Error::PreconditionFailed);
            }
            client.upload_if_absent(&backup, temp.path()).await?;
        } else {
            client
                .copy_object_matching(key, &backup, Some(etag))
                .await?;
        }
        client.delete_object_if_match(key, etag).await?;
        Ok(())
    }

    fn local_trash_entry(cfg: &AppConfig, locator: &str) -> Result<Option<TrashEntry>, SyncError> {
        let base = Path::new(&cfg.local_path);
        let root = base.join(TRASH);
        let parts: Vec<_> = locator.split('/').collect();
        let (stamp, name, has_metadata) = match parts.as_slice() {
            [stamp, name] => (*stamp, *name, false),
            [stamp, CONTENT, name] => (*stamp, *name, true),
            _ => return Ok(None),
        };
        let Some(deleted_at) = deleted_at(stamp) else {
            return Ok(None);
        };
        let Ok(path) = safe_relative_path(locator, &root) else {
            return Ok(None);
        };
        if !path_within_sync_folder(&path, base) {
            return Ok(None);
        }
        let Some(meta) = regular_file(&path).map_err(io_error)? else {
            return Ok(None);
        };
        let original_key = if has_metadata {
            let metadata = root.join(stamp).join(METADATA);
            require_local_path(&metadata, base)?;
            let Some(meta) = regular_file(&metadata).map_err(io_error)? else {
                return Ok(None);
            };
            if meta.len() > 32 * 1024 {
                return Ok(None);
            }
            let bytes = fs::read(metadata).map_err(io_error)?;
            let Ok(record) = serde_json::from_slice::<LocalTrashMetadata>(&bytes) else {
                return Ok(None);
            };
            if record.scope != config_fingerprint(cfg)
                || scoped_key(&record.original_key, cfg).is_err()
            {
                return Ok(None);
            }
            Some(record.original_key)
        } else {
            None
        };
        Ok(Some(TrashEntry {
            source: TrashSource::Local,
            locator: locator.into(),
            original_key,
            file_name: name.into(),
            deleted_at,
            size_bytes: meta.len(),
        }))
    }

    fn local_trash_entries(cfg: &AppConfig) -> Result<Vec<TrashEntry>, SyncError> {
        let base = Path::new(&cfg.local_path);
        let root = base.join(TRASH);
        require_local_path(&root, base)?;
        let folders = match fs::read_dir(&root) {
            Ok(folders) => folders,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_error(err)),
        };
        let mut entries = Vec::new();
        for folder in folders {
            let folder = folder.map_err(io_error)?;
            let Ok(stamp) = folder.file_name().into_string() else {
                continue;
            };
            if deleted_at(&stamp).is_none()
                || !folder.file_type().map_err(io_error)?.is_dir()
                || !path_within_sync_folder(&folder.path(), base)
            {
                continue;
            }
            let content = folder.path().join(CONTENT);
            let manifest = folder.path().join(METADATA);
            let managed = fs::symlink_metadata(&manifest).is_ok() && content.is_dir();
            let (directory, prefix) = if managed {
                (content, format!("{stamp}/{CONTENT}"))
            } else {
                (folder.path(), stamp)
            };
            if !path_within_sync_folder(&directory, base) {
                continue;
            }
            for item in fs::read_dir(directory).map_err(io_error)? {
                let Ok(name) = item.map_err(io_error)?.file_name().into_string() else {
                    continue;
                };
                if let Some(entry) = Self::local_trash_entry(cfg, &format!("{prefix}/{name}"))? {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    fn remote_trash_entry(cfg: &AppConfig, locator: &str, size: u64) -> Option<TrashEntry> {
        let prefix = format!("{}/", trash_prefix(cfg));
        let (stamp, relative) = locator.strip_prefix(&prefix)?.split_once('/')?;
        let name = relative
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())?;
        safe_relative_path(relative, Path::new(&cfg.local_path)).ok()?;
        Some(TrashEntry {
            source: TrashSource::Remote,
            locator: locator.into(),
            original_key: Some(apply_s3_prefix(relative.to_owned(), &cfg.s3_prefix)),
            file_name: name.into(),
            deleted_at: deleted_at(stamp)?,
            size_bytes: size,
        })
    }

    pub async fn list_trash(&self) -> TrashListing {
        let cfg = self.get_config();
        let mut listing = TrashListing::default();
        match Self::local_trash_entries(&cfg) {
            Ok(entries) => listing.entries = entries,
            Err(err) => listing.local_error = Some(err.to_string()),
        }
        let remote = async {
            let (_, client) = self.configured_s3()?;
            Ok::<_, SyncError>(client.list_objects(Some(&trash_prefix(&cfg))).await?)
        }
        .await;
        match remote {
            Ok(objects) => listing.entries.extend(
                objects
                    .into_iter()
                    .filter_map(|item| Self::remote_trash_entry(&cfg, &item.key, item.size)),
            ),
            Err(err) => listing.remote_error = Some(err.to_string()),
        }
        listing.entries.sort_by(|a, b| {
            b.deleted_at
                .cmp(&a.deleted_at)
                .then(a.locator.cmp(&b.locator))
        });
        listing
    }

    pub async fn restore_trash(
        &self,
        source: TrashSource,
        locator: &str,
        legacy_path: Option<&str>,
    ) -> Result<TrashRestore, SyncError> {
        let _guard = self.reconciliation_lock.lock().await;
        let (cfg, client) = self.configured_s3()?;
        let base = Path::new(&cfg.local_path);
        let entry = match source {
            TrashSource::Local => Self::local_trash_entry(&cfg, locator)?,
            TrashSource::Remote => Self::remote_trash_entry(&cfg, locator, 0),
        }
        .ok_or_else(missing_entry)?;
        let key = match entry.original_key {
            Some(key) => key,
            None => {
                let relative = legacy_path
                    .filter(|p| !p.ends_with('/'))
                    .ok_or_else(missing_entry)?;
                safe_relative_path(relative, base)?;
                apply_s3_prefix(relative.to_owned(), &cfg.s3_prefix)
            }
        };
        let path = self.scoped_local_path(&key, &cfg)?;
        if is_excluded(&path, &cfg.exclusion_patterns) {
            return Err(SyncError::Conflict(
                tr(
                    "The destination is excluded from sync.",
                    "O destino está excluído da sincronização.",
                )
                .into(),
            ));
        }
        let local = path.try_exists().map_err(io_error)?;
        let remote = match client.head_object(&key).await {
            Ok(_) => true,
            Err(S3Error::ObjectNotFound) => false,
            Err(err) => return Err(err.into()),
        };
        if local || remote {
            return Ok(TrashRestore::Blocked { local, remote });
        }
        if self.has_deletion_review(&key)? || self.queue_manager.has_unfinished_operation(&key)? {
            return Err(SyncError::Conflict(tr("Complete pending or failed sync operations for this file before restoring it.", "Conclua as operações de sincronização pendentes ou com erro deste arquivo antes de restaurá-lo.").into()));
        }
        let parent = path.parent().ok_or_else(missing_entry)?;
        ensure_dir_exists(parent)?;
        require_local_path(&path, base)?;
        let local_copy = if source == TrashSource::Local {
            Some(safe_relative_path(locator, &base.join(TRASH))?)
        } else {
            None
        };
        let (temp, backup_etag) = match &local_copy {
            Some(local_copy) => {
                let mut input = fs::File::open(local_copy).map_err(io_error)?;
                let mut temp = transfer_temp_file(&path)?;
                std::io::copy(&mut input, temp.as_file_mut()).map_err(io_error)?;
                (temp, None)
            }
            None => {
                let (temp, etag) = client.download_to_temp(locator, &path).await?;
                (temp, Some(etag))
            }
        };
        let hash = compute_md5(temp.path())?;
        let size = temp.as_file().metadata().map_err(io_error)?.len();
        require_local_path(&path, base)?;
        if path.try_exists().map_err(io_error)? {
            return Ok(TrashRestore::Blocked {
                local: true,
                remote: false,
            });
        }
        let etag = match client.upload_if_absent(&key, temp.path()).await {
            Ok(etag) => etag,
            Err(S3Error::PreconditionFailed) => {
                return Ok(TrashRestore::Blocked {
                    local: false,
                    remote: true,
                })
            }
            Err(err) => return Err(err.into()),
        };
        require_local_path(&path, base)?;
        let file = temp.persist_noclobber(&path).map_err(|err| {
            SyncError::Io(crate::language::localized_format!(
                "Restored in S3, but could not create the local file. The trash copy was kept. {}", "Restaurado no S3, mas não foi possível criar o arquivo local. A cópia foi mantida na lixeira. {}",
                err.error
            ))
        })?;
        let now = Utc::now();
        self.db
            .upsert_file_entry(&FileEntry {
                s3_key: key.clone(),
                local_path: path.to_string_lossy().into_owned(),
                sync_state: SyncState::Synced,
                last_sync_etag: Some(etag),
                local_hash: Some(hash.clone()),
                last_sync_time: Some(now),
                local_modified_time: file
                    .metadata()
                    .ok()
                    .and_then(|meta| meta.modified().ok())
                    .map(DateTime::<Utc>::from),
                size_bytes: size,
            })
            .map_err(|err| SyncError::Queue(err.to_string()))?;
        self.status_monitor
            .set_file_status(&key, SyncState::Synced, size);
        self.emit(
            "sync:file-status-changed",
            &FileStatusEvent {
                path: key,
                state: SyncState::Synced,
            },
        );
        let backup_retained = match local_copy {
            Some(local_copy) => {
                let unchanged = path_within_sync_folder(&local_copy, base)
                    && compute_md5(&local_copy).is_ok_and(|current| current == hash);
                if unchanged && fs::remove_file(&local_copy).is_ok() {
                    if let Some(folder) = local_copy
                        .parent()
                        .filter(|_| locator.split('/').count() == 3)
                    {
                        if let Some(record) = folder.parent() {
                            let _ = fs::remove_file(record.join(METADATA));
                            let _ = fs::remove_dir(folder);
                            let _ = fs::remove_dir(record);
                        }
                    }
                    false
                } else {
                    true
                }
            }
            None => client
                .delete_object_if_match(locator, backup_etag.as_deref().unwrap_or_default())
                .await
                .is_err(),
        };
        Ok(TrashRestore::Restored { backup_retained })
    }
}
