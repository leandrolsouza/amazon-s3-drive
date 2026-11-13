use amazon_s3_drive_lib::{
    db::Database,
    models::{AppConfig, FileEntry, LocalFsEvent, SyncOperation, SyncOperationType, SyncState},
    network_detector::NetworkDetector,
    queue_manager::QueueManager,
    s3_client::{compute_md5_bytes, MockObject, MockS3Backend, S3Client},
    status_monitor::StatusMonitor,
    sync_engine::{RecordingEmitter, RetryPolicy, SyncEngine},
};
use chrono::Utc;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use tempfile::{tempdir, TempDir};

struct Fixture {
    dir: TempDir,
    db: Database,
    client: S3Client,
    remote: Arc<Mutex<MockS3Backend>>,
    config: AppConfig,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir().unwrap();
        let db = Database::new(&dir.path().join("state.sqlite")).unwrap();
        let (client, remote) = S3Client::new_mock("test", Some(db.clone()));
        let local = dir.path().join("files");
        fs::create_dir(&local).unwrap();
        let config = AppConfig {
            local_path: local.to_string_lossy().into(),
            bucket_name: "test".into(),
            s3_prefix: Some("documents".into()),
            ..AppConfig::default()
        };
        Self {
            dir,
            db,
            client,
            remote,
            config,
        }
    }
    fn engine(&self) -> Arc<SyncEngine> {
        self.engine_with_network(NetworkDetector::new())
    }

    fn engine_with_network(&self, network: NetworkDetector) -> Arc<SyncEngine> {
        let db = Database::new(&self.dir.path().join("state.sqlite")).unwrap();
        Arc::new(
            SyncEngine::new(
                self.config.clone(),
                Some(self.client.clone()),
                db.clone(),
                Arc::new(QueueManager::new(&db)),
                Arc::new(StatusMonitor::default()),
                network,
                Arc::new(RecordingEmitter::default()),
            )
            .with_retry_policy(RetryPolicy::fast_for_tests()),
        )
    }
    fn local(&self) -> std::path::PathBuf {
        self.dir.path().join("files/test.txt")
    }
    fn remote_write(&self, bytes: &[u8]) {
        self.remote.lock().unwrap().objects.insert(
            "documents/test.txt".into(),
            MockObject {
                bytes: bytes.into(),
                etag: compute_md5_bytes(bytes),
                last_modified: Utc::now(),
            },
        );
    }
    fn synced(&self) {
        fs::write(self.local(), b"original").unwrap();
        self.remote_write(b"original");
        let now = Utc::now();
        self.db
            .upsert_file_entry(&FileEntry {
                s3_key: "documents/test.txt".into(),
                local_path: self.local().to_string_lossy().into(),
                sync_state: SyncState::Synced,
                last_sync_etag: Some(compute_md5_bytes(b"original")),
                local_hash: Some(compute_md5_bytes(b"original")),
                last_sync_time: Some(now),
                local_modified_time: Some(now),
                size_bytes: 8,
            })
            .unwrap();
    }
}

#[tokio::test]
async fn queued_deletion_keeps_the_approved_etag_across_restart() {
    for changed in [false, true] {
        let mut f = Fixture::new();
        f.config.sync_direction = "UploadOnly".into();
        f.synced();
        let network = NetworkDetector::new();
        network.set_online(false);
        let engine = f.engine_with_network(network);
        engine
            .delete_file_and_sync("documents/test.txt")
            .await
            .unwrap();
        let pending = engine.queue_manager().get_all_pending_ordered().unwrap();
        assert_eq!(
            pending[0].operation.extra.as_deref(),
            Some(compute_md5_bytes(b"original").as_str())
        );
        drop(engine);
        if changed {
            f.remote_write(b"new version");
        }

        let restarted = f.engine();
        restarted.poll_remote_changes().await.unwrap();
        restarted.process_queue().await.unwrap();
        restarted.poll_remote_changes().await.unwrap();

        if changed {
            assert_eq!(
                f.remote.lock().unwrap().objects["documents/test.txt"].bytes,
                b"new version"
            );
            let review = restarted.deletion_reviews().unwrap().remove(0);
            assert!(review.queued && review.other_side_changed);
            restarted.resolve_deletion(&review, true).await.unwrap();
            assert_eq!(fs::read(f.local()).unwrap(), b"new version");
        } else {
            assert!(!f
                .remote
                .lock()
                .unwrap()
                .objects
                .contains_key("documents/test.txt"));
            assert!(restarted.deletion_reviews().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn legacy_delete_without_etag_goes_to_review() {
    let f = Fixture::new();
    f.remote_write(b"remote version");
    let engine = f.engine();
    engine
        .queue_manager()
        .enqueue(SyncOperation {
            operation_type: SyncOperationType::Delete,
            s3_key: "documents/test.txt".into(),
            local_path: f.local().to_string_lossy().into_owned(),
            extra: None,
        })
        .unwrap();
    engine.process_queue().await.unwrap();
    assert!(f
        .remote
        .lock()
        .unwrap()
        .objects
        .contains_key("documents/test.txt"));
    assert!(engine.deletion_reviews().unwrap()[0].queued);
}

#[tokio::test]
async fn offline_deletion_survives_restart_and_can_restore_without_prefix_duplication() {
    let f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    engine.process_queue().await.unwrap();
    assert!(!f.local().exists());
    let reviews = engine.deletion_reviews().unwrap();
    assert_eq!(reviews.len(), 1);
    assert!(!reviews[0].other_side_changed);
    drop(engine);
    let restarted = f.engine();
    assert_eq!(restarted.deletion_reviews().unwrap(), reviews);
    restarted.resolve_deletion(&reviews[0], true).await.unwrap();
    assert_eq!(fs::read(f.local()).unwrap(), b"original");
    assert!(!f.dir.path().join("files/documents").exists());
    restarted.poll_remote_changes().await.unwrap();
    assert!(restarted.deletion_reviews().unwrap().is_empty());
}

#[tokio::test]
async fn confirmed_deletion_preserves_remote_copy_and_does_not_redownload_trash() {
    let f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    let review = engine.deletion_reviews().unwrap().remove(0);
    engine.resolve_deletion(&review, false).await.unwrap();
    {
        let remote = f.remote.lock().unwrap();
        assert!(!remote.objects.contains_key("documents/test.txt"));
        assert!(remote
            .objects
            .iter()
            .any(|(key, obj)| key.starts_with("documents/.s3-drive-trash/")
                && obj.bytes == b"original"));
    }
    engine.poll_remote_changes().await.unwrap();
    engine.scan_local_directory().await.unwrap();
    assert_eq!(engine.queue_manager().count_pending().unwrap(), 0);
    assert!(!f.local().exists());
    assert!(f
        .db
        .get_file_entry_by_key("documents/test.txt")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_versus_remote_edit_requires_review_and_stale_decisions_are_rejected() {
    let f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    let old = engine.deletion_reviews().unwrap().remove(0);
    f.remote_write(b"edited");
    assert!(engine.resolve_deletion(&old, false).await.is_err());
    engine.poll_remote_changes().await.unwrap();
    let current = engine.deletion_reviews().unwrap().remove(0);
    assert!(current.other_side_changed);
    assert!(engine.resolve_deletion(&old, false).await.is_err());
    engine.resolve_deletion(&current, true).await.unwrap();
    assert_eq!(fs::read(f.local()).unwrap(), b"edited");
}

#[tokio::test]
async fn remote_deletion_preserves_local_edit_and_restore_uploads_it() {
    let f = Fixture::new();
    f.synced();
    f.remote.lock().unwrap().objects.clear();
    fs::write(f.local(), b"edited locally").unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    engine.scan_local_directory().await.unwrap();
    assert_eq!(engine.queue_manager().count_pending().unwrap(), 0);
    let review = engine.deletion_reviews().unwrap().remove(0);
    assert_eq!(review.deleted_side, "remote");
    assert!(review.other_side_changed);
    engine.resolve_deletion(&review, true).await.unwrap();
    assert_eq!(
        f.remote.lock().unwrap().objects["documents/test.txt"].bytes,
        b"edited locally"
    );
}

#[tokio::test]
async fn confirmed_remote_deletion_moves_local_version_to_recovery_folder() {
    let f = Fixture::new();
    f.synced();
    f.remote.lock().unwrap().objects.clear();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    let review = engine.deletion_reviews().unwrap().remove(0);
    engine.resolve_deletion(&review, false).await.unwrap();
    assert!(!f.local().exists());
    let folder = fs::read_dir(f.dir.path().join("files/.s3-drive-trash"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(folder.join("test.txt")).unwrap(), b"original");
    engine
        .process_local_event(LocalFsEvent::Renamed {
            from: f.local(),
            to: folder.join("test.txt"),
        })
        .await
        .unwrap();
    engine
        .process_local_event(LocalFsEvent::Created(folder.join("test.txt")))
        .await
        .unwrap();
    engine.scan_local_directory().await.unwrap();
    assert_eq!(engine.queue_manager().count_pending().unwrap(), 0);
}

#[tokio::test]
async fn inaccessible_root_and_failed_remote_listing_never_infer_deletions() {
    let f = Fixture::new();
    f.synced();
    let engine = f.engine();
    fs::rename(
        f.dir.path().join("files"),
        f.dir.path().join("disconnected"),
    )
    .unwrap();
    assert!(engine.poll_remote_changes().await.is_err());
    assert!(engine.deletion_reviews().unwrap().is_empty());
    fs::rename(
        f.dir.path().join("disconnected"),
        f.dir.path().join("files"),
    )
    .unwrap();
    f.remote.lock().unwrap().fail_auth = true;
    assert!(engine.poll_remote_changes().await.is_err());
    assert!(engine.deletion_reviews().unwrap().is_empty());
    assert!(f.local().exists());
}

#[tokio::test]
async fn exclusions_and_directional_modes_are_respected() {
    let mut f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    f.config.exclusion_patterns = vec!["*.txt".into()];
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    assert!(engine.deletion_reviews().unwrap().is_empty());
    f.config.exclusion_patterns.clear();
    f.config.sync_direction = "UploadOnly".into();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    assert!(engine.deletion_reviews().unwrap().is_empty());
    f.config.sync_direction = "DownloadOnly".into();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    engine.process_queue().await.unwrap();
    assert_eq!(fs::read(f.local()).unwrap(), b"original");
    assert!(engine.deletion_reviews().unwrap().is_empty());
}

#[tokio::test]
async fn failed_delete_keeps_review_and_history_for_retry() {
    let f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    let review = engine.deletion_reviews().unwrap().remove(0);
    f.remote.lock().unwrap().fail_delete = true;
    assert!(engine.resolve_deletion(&review, false).await.is_err());
    assert_eq!(engine.deletion_reviews().unwrap(), vec![review.clone()]);
    assert!(f
        .db
        .get_file_entry_by_key("documents/test.txt")
        .unwrap()
        .is_some());
    f.remote.lock().unwrap().fail_delete = false;
    engine.resolve_deletion(&review, false).await.unwrap();
}

#[tokio::test]
async fn new_remote_file_downloads_but_known_online_deletion_asks_for_review() {
    let f = Fixture::new();
    f.remote_write(b"new");
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    engine.process_queue().await.unwrap();
    assert_eq!(fs::read(f.local()).unwrap(), b"new");
    fs::remove_file(f.local()).unwrap();
    engine
        .process_local_event(LocalFsEvent::Deleted(f.local()))
        .await
        .unwrap();
    assert_eq!(engine.deletion_reviews().unwrap().len(), 1);
    assert!(f
        .remote
        .lock()
        .unwrap()
        .objects
        .contains_key("documents/test.txt"));
}

#[tokio::test]
async fn both_deleted_cleans_registry_and_conditional_delete_protects_new_version() {
    let f = Fixture::new();
    f.synced();
    fs::remove_file(f.local()).unwrap();
    f.remote.lock().unwrap().objects.clear();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    assert!(engine.deletion_reviews().unwrap().is_empty());
    assert!(f
        .db
        .get_file_entry_by_key("documents/test.txt")
        .unwrap()
        .is_none());
    f.remote_write(b"new version");
    assert!(f
        .client
        .delete_object_if_match("documents/test.txt", &compute_md5_bytes(b"original"))
        .await
        .is_err());
    assert!(f
        .remote
        .lock()
        .unwrap()
        .objects
        .contains_key("documents/test.txt"));
}

#[tokio::test]
async fn deleting_all_files_keeps_every_remote_object_until_individual_decisions() {
    let f = Fixture::new();
    f.synced();
    let mut second =
        f.db.get_file_entry_by_key("documents/test.txt")
            .unwrap()
            .unwrap();
    second.s3_key = "documents/second.txt".into();
    second.local_path = f
        .dir
        .path()
        .join("files/second.txt")
        .to_string_lossy()
        .into();
    f.db.upsert_file_entry(&second).unwrap();
    {
        let mut remote = f.remote.lock().unwrap();
        let object = remote.objects["documents/test.txt"].clone();
        remote.objects.insert("documents/second.txt".into(), object);
    }
    fs::remove_file(f.local()).unwrap();
    let engine = f.engine();
    engine.poll_remote_changes().await.unwrap();
    engine.process_queue().await.unwrap();
    assert_eq!(engine.deletion_reviews().unwrap().len(), 2);
    assert_eq!(f.remote.lock().unwrap().objects.len(), 2);
    assert!(!f.local().exists());
    for review in engine.deletion_reviews().unwrap() {
        engine.resolve_deletion(&review, true).await.unwrap();
    }
    assert!(engine.deletion_reviews().unwrap().is_empty());
    assert!(f.local().exists());
    assert!(f.dir.path().join("files/second.txt").exists());
}

#[tokio::test]
async fn previously_synced_missing_root_is_not_recreated_on_start() {
    let f = Fixture::new();
    f.synced();
    f.db.set_config_fingerprint(&amazon_s3_drive_lib::sync_engine::config_fingerprint(
        &f.config,
    ))
    .unwrap();
    fs::rename(
        f.dir.path().join("files"),
        f.dir.path().join("disconnected"),
    )
    .unwrap();
    let engine = f.engine();
    assert!(engine.start().await.is_err());
    assert!(!f.dir.path().join("files").exists());
    assert!(engine.deletion_reviews().unwrap().is_empty());
}
