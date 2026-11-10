use crate::models::{ConflictError, ConflictInfo, FileEntry, S3ObjectInfo};
use crate::s3_client::S3Client;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ConflictResolver {
    local_base: PathBuf,
}

impl ConflictResolver {
    pub fn new(local_base: PathBuf) -> Self {
        Self { local_base }
    }

    pub fn local_base(&self) -> &Path {
        &self.local_base
    }

    /// Detecta conflito (Requisito 6.1, 6.4):
    /// local_modified_time != last_sync_time E s3.etag != last_sync_etag
    pub fn detect_conflict(file: &FileEntry, s3: &S3ObjectInfo) -> bool {
        let local_changed = file.local_modified_time != file.last_sync_time;
        let remote_changed = Some(s3.etag.clone()) != file.last_sync_etag;
        local_changed && remote_changed
    }

    /// Gera o caminho de cópia de conflito (Requisito 6.2):
    /// [nome_base](conflito-AAAA-MM-DD-HH-MM-SS).[extensão]
    pub fn conflict_path(original: &Path, timestamp: &DateTime<Utc>) -> PathBuf {
        let parent = original.parent().unwrap_or_else(|| Path::new(""));
        let file_name = original
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("arquivo");

        let ts_str = timestamp.format("%Y-%m-%d-%H-%M-%S").to_string();

        // Tratar extensões (incluindo compostas como .tar.gz ou simples como .docx)
        let new_name = if let Some(dot_idx) = file_name.find('.') {
            if dot_idx == 0 {
                // Arquivo oculto como .gitignore sem outra extensão
                if let Some(second_dot) = file_name[1..].find('.') {
                    let split_at = 1 + second_dot;
                    let stem = &file_name[..split_at];
                    let ext = &file_name[split_at + 1..];
                    format!("{}(conflito-{}).{}", stem, ts_str, ext)
                } else {
                    format!("{}(conflito-{})", file_name, ts_str)
                }
            } else {
                let stem = &file_name[..dot_idx];
                let ext = &file_name[dot_idx + 1..];
                format!("{}(conflito-{}).{}", stem, ts_str, ext)
            }
        } else {
            format!("{}(conflito-{})", file_name, ts_str)
        };

        if parent.as_os_str().is_empty() {
            PathBuf::from(new_name)
        } else {
            parent.join(new_name)
        }
    }

    /// Resolve conflito baixando a versão remota para `conflict_path` e preservando a versão local (Requisito 6.2, 6.3, 6.5)
    pub async fn resolve(
        &self,
        file: &FileEntry,
        s3: &S3ObjectInfo,
        s3_client: &S3Client,
    ) -> Result<ConflictInfo, ConflictError> {
        let local_path = PathBuf::from(&file.local_path);
        let file_name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&file.s3_key)
            .to_string();

        let conflict_copy_path = Self::conflict_path(&local_path, &s3.last_modified);

        // Verificar se é possível criar/escrever no destino da cópia de conflito
        if let Some(parent) = conflict_copy_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ConflictError::CannotSaveConflictCopy {
                        file: file_name.clone(),
                        reason: e.to_string(),
                    }
                })?;
            }
        }

        s3_client
            .download_file(&s3.key, &conflict_copy_path)
            .await
            .map_err(|e| ConflictError::CannotSaveConflictCopy {
                file: file_name.clone(),
                reason: e.to_string(),
            })?;

        let local_modified = file.local_modified_time.unwrap_or_else(Utc::now);

        Ok(ConflictInfo {
            file_name,
            local_path: file.local_path.clone(),
            conflict_copy_path: conflict_copy_path.to_string_lossy().to_string(),
            local_modified,
            remote_modified: s3.last_modified,
            local_size_bytes: file.size_bytes,
            remote_size_bytes: s3.size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SyncState;
    use chrono::TimeZone;
    use proptest::prelude::*;

    fn arb_datetime() -> impl Strategy<Value = DateTime<Utc>> {
        (1_600_000_000i64..=1_900_000_000i64)
            .prop_map(|secs| Utc.timestamp_opt(secs, 0).single().unwrap())
    }

    fn arb_file_entry() -> impl Strategy<Value = FileEntry> {
        (
            "[a-z]{3,10}\\.[a-z]{2,4}",
            prop::option::of("[a-f0-9]{8}"),
            prop::option::of(arb_datetime()),
            prop::option::of(arb_datetime()),
            1u64..=1_000_000u64,
        )
            .prop_map(
                |(name, last_sync_etag, last_sync_time, local_modified_time, size_bytes)| {
                    FileEntry {
                        s3_key: name.clone(),
                        local_path: format!("/tmp/{}", name),
                        sync_state: SyncState::Synced,
                        last_sync_etag,
                        last_sync_time,
                        local_modified_time,
                        size_bytes,
                    }
                },
            )
    }

    fn arb_s3_object_info() -> impl Strategy<Value = S3ObjectInfo> {
        (
            "[a-z]{3,10}\\.[a-z]{2,4}",
            "[a-f0-9]{8}",
            1u64..=1_000_000u64,
            arb_datetime(),
        )
            .prop_map(|(key, etag, size, last_modified)| S3ObjectInfo {
                key,
                etag,
                size,
                last_modified,
            })
    }

    fn arb_file_path() -> impl Strategy<Value = PathBuf> {
        ("[a-zA-Z0-9_-]{1,20}", prop::option::of("[a-z]{2,4}")).prop_map(|(stem, ext)| match ext {
            Some(e) => PathBuf::from(format!("/base/{}.{}", stem, e)),
            None => PathBuf::from(format!("/base/{}", stem)),
        })
    }

    // Feature: amazon-s3-drive, Property 6: Conflict detection condition
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn conflict_detected_iff_both_conditions(
            file in arb_file_entry(),
            s3 in arb_s3_object_info(),
        ) {
            let local_changed = file.local_modified_time != file.last_sync_time;
            let remote_changed = Some(s3.etag.clone()) != file.last_sync_etag;
            let expected = local_changed && remote_changed;
            prop_assert_eq!(ConflictResolver::detect_conflict(&file, &s3), expected);
        }
    }

    // Feature: amazon-s3-drive, Property 7: Conflict resolution output correctness
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn conflict_path_format(
            path in arb_file_path(),
            dt in arb_datetime(),
        ) {
            let result = ConflictResolver::conflict_path(&path, &dt);
            let name = result.file_name().unwrap().to_str().unwrap();
            let formatted = dt.format("%Y-%m-%d-%H-%M-%S");
            let expected_pattern = format!("(conflito-{})", formatted);
            prop_assert!(
                name.contains(&expected_pattern),
                "conflict path '{}' missing pattern '{}'",
                name,
                expected_pattern
            );
        }
    }

    #[test]
    fn test_conflict_path_without_extension_and_compound_extension() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 14, 30, 0).unwrap();

        let no_ext = Path::new("/docs/Makefile");
        let cp_no_ext = ConflictResolver::conflict_path(no_ext, &dt);
        assert_eq!(
            cp_no_ext.file_name().unwrap().to_str().unwrap(),
            "Makefile(conflito-2024-01-15-14-30-00)"
        );

        let compound = Path::new("/docs/archive.tar.gz");
        let cp_compound = ConflictResolver::conflict_path(compound, &dt);
        assert_eq!(
            cp_compound.file_name().unwrap().to_str().unwrap(),
            "archive(conflito-2024-01-15-14-30-00).tar.gz"
        );
    }

    #[tokio::test]
    async fn test_resolve_fails_when_conflict_copy_cannot_be_saved() {
        let (s3_client, _mock) = S3Client::new_mock("test-bucket", None);
        // Como a chave não existe no mock S3, o download da cópia de conflito falha
        // preservando o arquivo local e retornando CannotSaveConflictCopy (Requisito 6.5)
        let dir = tempfile::tempdir().unwrap();
        let local_file = dir.path().join("doc.txt");
        std::fs::write(&local_file, b"conteudo local preservado").unwrap();

        let entry = FileEntry {
            s3_key: "doc.txt".to_string(),
            local_path: local_file.to_string_lossy().to_string(),
            sync_state: SyncState::Synced,
            last_sync_etag: Some("old-etag".to_string()),
            last_sync_time: Some(Utc::now()),
            local_modified_time: Some(Utc::now()),
            size_bytes: 25,
        };
        let s3_info = S3ObjectInfo {
            key: "doc.txt".to_string(),
            etag: "new-etag".to_string(),
            size: 100,
            last_modified: Utc::now(),
        };

        let resolver = ConflictResolver::new(dir.path().to_path_buf());
        let res = resolver.resolve(&entry, &s3_info, &s3_client).await;
        assert!(matches!(
            res,
            Err(ConflictError::CannotSaveConflictCopy { .. })
        ));
        // Versão local deve continuar intacta
        assert_eq!(
            std::fs::read_to_string(&local_file).unwrap(),
            "conteudo local preservado"
        );
    }
}
