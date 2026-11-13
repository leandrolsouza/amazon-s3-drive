use crate::language::tr;
use super::*;
use crate::models::DeletionReview;
use rusqlite::params;

pub(super) const TRASH: &str = ".s3-drive-trash";

impl SyncEngine {
    pub fn deletion_reviews(&self) -> Result<Vec<DeletionReview>, SyncError> {
        let conn = self.db.connection();
        let conn = conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT payload FROM deletion_reviews ORDER BY s3_key")
            .map_err(|e| SyncError::Queue(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| SyncError::Queue(e.to_string()))?;
        rows.map(|row| {
            let json = row.map_err(|e| SyncError::Queue(e.to_string()))?;
            serde_json::from_str(&json).map_err(|e| SyncError::Queue(e.to_string()))
        })
        .collect()
    }

    pub(super) fn review_deletion(
        &self,
        entry: &FileEntry,
        remote: Option<&crate::models::S3ObjectInfo>,
        queued: bool,
    ) -> Result<(), SyncError> {
        let local_hash = if Path::new(&entry.local_path)
            .try_exists()
            .map_err(|e| SyncError::Io(e.to_string()))?
        {
            Some(compute_md5(Path::new(&entry.local_path))?)
        } else {
            None
        };
        let review = DeletionReview {
            s3_key: entry.s3_key.clone(),
            local_path: entry.local_path.clone(),
            deleted_side: if remote.is_some() { "local" } else { "remote" }.into(),
            queued,
            other_side_changed: queued
                || match remote {
                    Some(obj) => entry.last_sync_etag.as_ref() != Some(&obj.etag),
                    None => {
                        local_hash.as_ref()
                            != entry.local_hash.as_ref().or(entry.last_sync_etag.as_ref())
                    }
                },
            remote_etag: remote.map(|obj| obj.etag.clone()),
            local_hash,
        };
        self.db
            .connection()
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO deletion_reviews (s3_key, payload) VALUES (?1, ?2)",
                params![
                    review.s3_key,
                    serde_json::to_string(&review).map_err(|e| SyncError::Queue(e.to_string()))?
                ],
            )
            .map_err(|e| SyncError::Queue(e.to_string()))?;
        self.status_monitor
            .set_state(&entry.s3_key, SyncState::Pending);
        Ok(())
    }

    pub(super) fn clear_deletion_review(&self, key: &str) -> Result<(), SyncError> {
        self.db
            .connection()
            .lock()
            .unwrap()
            .execute("DELETE FROM deletion_reviews WHERE s3_key = ?1", [key])
            .map_err(|e| SyncError::Queue(e.to_string()))?;
        Ok(())
    }

    pub(super) fn has_deletion_review(&self, key: &str) -> Result<bool, SyncError> {
        self.db
            .connection()
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM deletion_reviews WHERE s3_key = ?1)",
                [key],
                |row| row.get(0),
            )
            .map_err(|e| SyncError::Queue(e.to_string()))
    }

    /// A decisão é revalidada contra os dois lados antes de tocar nos dados.
    pub async fn resolve_deletion(
        &self,
        expected: &DeletionReview,
        restore: bool,
    ) -> Result<(), SyncError> {
        let _guard = self.reconciliation_lock.lock().await;
        let key = expected.s3_key.as_str();
        let cfg = self.get_config();
        let relative_key = scoped_key(key, &cfg)?;
        if !expected.queued && !cfg.sync_direction.eq_ignore_ascii_case("Bidirectional") {
            return Err(SyncError::Conflict(
                tr("The sync direction changed. Review the settings.", "A direção da sincronização mudou. Revise a configuração.").into(),
            ));
        }
        fs::read_dir(&cfg.local_path).map_err(|e| SyncError::Io(e.to_string()))?;
        let review = self
            .deletion_reviews()?
            .into_iter()
            .find(|r| r.s3_key == key)
            .ok_or_else(|| SyncError::Conflict(tr("The deletion is no longer pending.", "Exclusão não está mais pendente.").into()))?;
        if &review != expected {
            return Err(SyncError::Conflict(
                tr("The review changed. Check the updated details before confirming.", "A revisão mudou. Confira os dados atualizados antes de confirmar.").into(),
            ));
        }
        if !path_within_sync_folder(Path::new(&review.local_path), Path::new(&cfg.local_path))
            || is_excluded(Path::new(&review.local_path), &cfg.exclusion_patterns)
        {
            return Err(SyncError::Conflict(
                tr("The file is outside the synced folder or excluded by the settings.", "Arquivo fora da pasta sincronizada ou excluído pela configuração.").into(),
            ));
        }
        let client = self
            .s3_client
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| SyncError::Credentials(tr("The S3 client has not been initialized.", "Cliente S3 não inicializado.").into()))?;
        let remote = client
            .list_objects(cfg.s3_prefix.as_deref())
            .await?
            .into_iter()
            .find(|o| o.key == key);
        let path = Path::new(&review.local_path);
        let local_hash = if path
            .try_exists()
            .map_err(|e| SyncError::Io(e.to_string()))?
        {
            Some(compute_md5(path)?)
        } else {
            None
        };
        if remote.as_ref().map(|o| &o.etag) != review.remote_etag.as_ref()
            || local_hash != review.local_hash
        {
            return Err(SyncError::Conflict(tr("The file changed since the review. Wait for the next scan and review it again.", "O arquivo mudou desde a revisão. Aguarde a próxima verificação e revise novamente.").into()));
        }
        let stamp = Utc::now().timestamp_nanos_opt().unwrap_or_default();
        if restore {
            let op = SyncOperation {
                operation_type: if review.deleted_side == "local" {
                    SyncOperationType::Download
                } else {
                    SyncOperationType::Upload
                },
                s3_key: key.into(),
                local_path: review.local_path.clone(),
                extra: Some("deletion-review".into()),
            };
            self.execute_single_operation(&op, &client, Path::new(&cfg.local_path))
                .await?;
        } else {
            // Preservar o conteúdo antes da exclusão; falhar sem remover se a cópia falhar.
            if review.deleted_side == "local" {
                let trash_key =
                    apply_s3_prefix(format!("{TRASH}/{stamp}/{relative_key}"), &cfg.s3_prefix);
                client.copy_object(key, &trash_key).await?;
                if client.head_object(key).await?.etag
                    != review.remote_etag.clone().unwrap_or_default()
                {
                    return Err(SyncError::Conflict(
                        tr("The remote version changed. Deletion was canceled.", "A versão remota mudou; exclusão cancelada.").into(),
                    ));
                }
                client
                    .delete_object_if_match(key, review.remote_etag.as_deref().unwrap_or_default())
                    .await?;
            } else {
                let trash_dir = Path::new(&cfg.local_path)
                    .join(TRASH)
                    .join(stamp.to_string());
                require_local_path(&trash_dir, Path::new(&cfg.local_path))?;
                fs::create_dir_all(&trash_dir).map_err(|e| SyncError::Io(e.to_string()))?;
                let name = path
                    .file_name()
                    .ok_or_else(|| SyncError::Io(tr("Invalid name.", "Nome inválido.").into()))?;
                let trash_path = trash_dir.join(name);
                require_local_path(&trash_path, Path::new(&cfg.local_path))?;
                fs::rename(path, trash_path).map_err(|e| SyncError::Io(e.to_string()))?;
            }
            self.db
                .delete_file_entry(key)
                .map_err(|e| SyncError::Queue(e.to_string()))?;
            self.status_monitor.remove_state(key);
        }
        self.clear_deletion_review(key)?;
        Ok(())
    }
}
