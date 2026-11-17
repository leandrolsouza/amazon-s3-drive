use super::*;
use crate::models::S3ObjectInfo;

#[derive(Clone, Copy)]
pub(super) enum SyncDirection {
    Bidirectional,
    UploadOnly,
    DownloadOnly,
}

impl SyncDirection {
    pub(super) fn from_config(config: &AppConfig) -> Self {
        if config.sync_direction.eq_ignore_ascii_case("UploadOnly") {
            Self::UploadOnly
        } else if config.sync_direction.eq_ignore_ascii_case("DownloadOnly") {
            Self::DownloadOnly
        } else {
            Self::Bidirectional
        }
    }

    pub(super) fn is_upload_only(self) -> bool {
        matches!(self, Self::UploadOnly)
    }
}

#[derive(Clone, Copy)]
enum LocalFile<'a> {
    Missing,
    Present { hash: Option<&'a str> },
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Ignore,
    Download,
    RegisterIdentical,
    PreserveConflict,
    MarkSynced,
    ReviewDeletion { other_side_changed: bool },
    MoveToTrash,
    Forget,
}

fn decide(
    direction: SyncDirection,
    previous: Option<&FileEntry>,
    local: LocalFile<'_>,
    remote: Option<&S3ObjectInfo>,
) -> Decision {
    use Decision::*;
    use LocalFile::*;

    match (previous, local, remote) {
        (None, _, None) => Ignore,
        (None, _, Some(_)) if direction.is_upload_only() => Ignore,
        (None, Missing, Some(_)) => Download,
        (None, Present { hash }, Some(remote)) if hash == Some(remote.etag.as_str()) => {
            RegisterIdentical
        }
        (None, Present { .. }, Some(_)) => PreserveConflict,
        (Some(_), _, None) if direction.is_upload_only() => Ignore,
        (Some(_), Missing, None) => Forget,
        (Some(_), Present { .. }, None) if matches!(direction, SyncDirection::DownloadOnly) => {
            MoveToTrash
        }
        (Some(previous), Present { hash }, None) => ReviewDeletion {
            other_side_changed: hash
                != previous
                    .local_hash
                    .as_deref()
                    .or(previous.last_sync_etag.as_deref()),
        },
        (Some(previous), Missing, Some(remote)) if previous.last_sync_etag.is_some() => {
            match direction {
                SyncDirection::UploadOnly => Ignore,
                SyncDirection::DownloadOnly => Download,
                SyncDirection::Bidirectional => ReviewDeletion {
                    other_side_changed: previous.last_sync_etag.as_deref() != Some(&remote.etag),
                },
            }
        }
        (Some(previous), _, Some(remote)) if !direction.is_upload_only() => {
            if ConflictResolver::detect_conflict(previous, remote) {
                PreserveConflict
            } else if previous.last_sync_etag.as_deref() != Some(&remote.etag) {
                Download
            } else {
                MarkSynced
            }
        }
        (Some(_), _, Some(_)) => MarkSynced,
    }
}

fn observe_tracked_edit(entry: &FileEntry, path: &Path) -> FileEntry {
    let mut observed = entry.clone();
    // O tamanho e a data evitam recalcular o hash em cada verificação do S3.
    if let Ok(metadata) = fs::metadata(path) {
        let size = metadata.len();
        let newer = metadata
            .modified()
            .ok()
            .map(DateTime::<Utc>::from)
            .zip(entry.last_sync_time)
            .is_none_or(|(modified, synced)| modified > synced);
        if (size != entry.size_bytes || newer)
            && compute_md5(path).ok().is_some_and(|hash| {
                entry
                    .local_hash
                    .as_deref()
                    .or(entry.last_sync_etag.as_deref())
                    .is_some_and(|reference| !hash.eq_ignore_ascii_case(reference))
            })
        {
            observed.local_modified_time = Some(Utc::now());
            observed.size_bytes = size;
        }
    }
    observed
}

impl SyncEngine {
    pub(super) async fn reconcile_file(
        &self,
        cfg: &AppConfig,
        client: &S3Client,
        key: &str,
        path: &Path,
        previous: Option<&FileEntry>,
        remote: Option<&S3ObjectInfo>,
    ) -> Result<(), SyncError> {
        let direction = SyncDirection::from_config(cfg);
        if previous.is_none() && direction.is_upload_only() {
            return Ok(());
        }
        let exists = path
            .try_exists()
            .map_err(|err| SyncError::Io(err.to_string()))?;
        let local_hash = if exists {
            match (previous, remote) {
                (None, Some(_)) => compute_md5(path).ok(),
                (Some(_), None) if matches!(direction, SyncDirection::Bidirectional) => {
                    Some(compute_md5(path)?)
                }
                _ => None,
            }
        } else {
            None
        };
        let local = if exists {
            LocalFile::Present {
                hash: local_hash.as_deref(),
            }
        } else {
            LocalFile::Missing
        };
        if remote.is_some()
            && previous.is_some_and(|entry| exists || entry.last_sync_etag.is_none())
            && self.has_deletion_review(key)?
        {
            self.clear_deletion_review(key)?;
        }
        let edited = previous
            .filter(|_| exists && remote.is_some() && !direction.is_upload_only())
            .map(|entry| observe_tracked_edit(entry, path));
        let observed = edited.as_ref().or(previous);

        match (decide(direction, observed, local, remote), previous, remote) {
            (Decision::Download, _, _) => {
                self.enqueue_and_notify(SyncOperation {
                    operation_type: SyncOperationType::Download,
                    s3_key: key.to_owned(),
                    local_path: path.to_string_lossy().into_owned(),
                    extra: None,
                })?;
            }
            (Decision::RegisterIdentical, _, Some(remote)) => {
                let size = fs::metadata(path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(remote.size);
                let now = Utc::now();
                let entry = FileEntry {
                    s3_key: key.to_owned(),
                    local_path: path.to_string_lossy().into_owned(),
                    sync_state: SyncState::Synced,
                    last_sync_etag: Some(remote.etag.clone()),
                    local_hash,
                    last_sync_time: Some(now),
                    local_modified_time: Some(now),
                    size_bytes: size,
                };
                let _ = self.db.upsert_file_entry(&entry);
                self.status_monitor
                    .set_file_status(key, SyncState::Synced, size);
            }
            (Decision::PreserveConflict, _, Some(remote)) => {
                let synthetic;
                let entry = if let Some(entry) = observed {
                    entry
                } else {
                    let metadata = fs::metadata(path).ok();
                    synthetic = FileEntry {
                        local_modified_time: Some(
                            metadata
                                .as_ref()
                                .and_then(|m| m.modified().ok())
                                .map(DateTime::<Utc>::from)
                                .unwrap_or_else(Utc::now),
                        ),
                        size_bytes: metadata.map_or(0, |m| m.len()),
                        ..Self::delete_review_entry(key, &path.to_string_lossy())
                    };
                    &synthetic
                };
                let resolver = ConflictResolver::new(PathBuf::from(&cfg.local_path));
                match resolver.resolve(entry, remote, client).await {
                    Ok(info) => self.record_conflict(key, &info),
                    Err(err) => {
                        let error = err.to_string();
                        self.status_monitor
                            .set_state(key, SyncState::Error(error.clone()));
                        self.emit(
                            "sync:error",
                            &SyncErrorEvent {
                                file: key.to_owned(),
                                error,
                                retry_count: 3,
                            },
                        );
                    }
                }
            }
            (Decision::MarkSynced, _, Some(remote)) => {
                self.status_monitor
                    .set_file_status(key, SyncState::Synced, remote.size);
            }
            (Decision::ReviewDeletion { other_side_changed }, Some(previous), _) => {
                self.save_deletion_review(previous, remote, local_hash, false, other_side_changed)?;
            }
            (action @ (Decision::MoveToTrash | Decision::Forget), _, _) => {
                if action == Decision::MoveToTrash {
                    self.move_to_local_trash(cfg, key, path)?;
                }
                self.clear_deletion_review(key)?;
                let _ = self.db.delete_file_entry(key);
                self.status_monitor.remove_state(key);
            }
            (Decision::Ignore, _, _) => {}
            _ => unreachable!("the decision must match the snapshot that produced it"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{decide, Decision, LocalFile, SyncDirection};
    use crate::models::{FileEntry, S3ObjectInfo, SyncState};
    use chrono::DateTime;

    fn previous() -> FileEntry {
        let time = DateTime::from_timestamp(1, 0);
        FileEntry {
            s3_key: "docs/file.txt".into(),
            local_path: "docs/file.txt".into(),
            sync_state: SyncState::Synced,
            last_sync_etag: Some("old-etag".into()),
            local_hash: Some("old-hash".into()),
            last_sync_time: time,
            local_modified_time: time,
            size_bytes: 1,
        }
    }

    fn remote(etag: &str) -> S3ObjectInfo {
        S3ObjectInfo {
            key: "docs/file.txt".into(),
            etag: etag.into(),
            last_modified: DateTime::from_timestamp(2, 0).unwrap(),
            size: 2,
        }
    }

    #[test]
    fn local_deletion_and_remote_edit_require_conflict_review() {
        let previous = previous();
        for (etag, other_side_changed) in [("old-etag", false), ("new-etag", true)] {
            assert_eq!(
                decide(
                    SyncDirection::Bidirectional,
                    Some(&previous),
                    LocalFile::Missing,
                    Some(&remote(etag))
                ),
                Decision::ReviewDeletion { other_side_changed },
            );
        }
    }

    #[test]
    fn remote_deletion_compares_against_the_local_hash() {
        let mut previous = previous();
        for (hash, other_side_changed) in
            [("old-hash", false), ("old-etag", true), ("new-hash", true)]
        {
            assert_eq!(
                decide(
                    SyncDirection::Bidirectional,
                    Some(&previous),
                    LocalFile::Present { hash: Some(hash) },
                    None
                ),
                Decision::ReviewDeletion { other_side_changed },
            );
        }
        previous.local_hash = None;
        assert_eq!(
            decide(
                SyncDirection::Bidirectional,
                Some(&previous),
                LocalFile::Present {
                    hash: Some("old-etag")
                },
                None
            ),
            Decision::ReviewDeletion {
                other_side_changed: false
            },
        );
        assert_eq!(
            decide(
                SyncDirection::Bidirectional,
                Some(&previous),
                LocalFile::Missing,
                None
            ),
            Decision::Forget,
        );
    }

    #[test]
    fn files_present_on_both_sides_distinguish_edits_from_matches() {
        let direction = SyncDirection::Bidirectional;
        let mut previous = previous();
        let local = LocalFile::Present {
            hash: Some("old-hash"),
        };
        let identical = remote("old-hash");
        let old = remote("old-etag");
        let edited = remote("new-etag");
        assert_eq!(
            decide(direction, None, local, Some(&identical)),
            Decision::RegisterIdentical
        );
        assert_eq!(
            decide(direction, None, local, Some(&edited)),
            Decision::PreserveConflict
        );
        assert_eq!(
            decide(direction, None, LocalFile::Missing, Some(&edited)),
            Decision::Download
        );
        assert_eq!(
            decide(direction, Some(&previous), local, Some(&old)),
            Decision::MarkSynced
        );
        assert_eq!(
            decide(direction, Some(&previous), local, Some(&edited)),
            Decision::Download
        );
        previous.local_modified_time = DateTime::from_timestamp(3, 0);
        assert_eq!(
            decide(direction, Some(&previous), local, Some(&edited)),
            Decision::PreserveConflict
        );
    }

    #[test]
    fn directional_modes_preserve_their_deletion_rules() {
        let previous = previous();
        let remote = remote("new-etag");
        let local = LocalFile::Present {
            hash: Some("new-hash"),
        };
        for (direction, missing_local, missing_remote) in [
            (
                SyncDirection::UploadOnly,
                Decision::Ignore,
                Decision::Ignore,
            ),
            (
                SyncDirection::DownloadOnly,
                Decision::Download,
                Decision::MoveToTrash,
            ),
        ] {
            assert_eq!(
                decide(
                    direction,
                    Some(&previous),
                    LocalFile::Missing,
                    Some(&remote)
                ),
                missing_local
            );
            assert_eq!(
                decide(direction, Some(&previous), local, None),
                missing_remote
            );
        }
    }
}
