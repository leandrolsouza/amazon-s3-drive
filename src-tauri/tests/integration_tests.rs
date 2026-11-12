use amazon_s3_drive_lib::credentials_manager::CredentialsManager;
use amazon_s3_drive_lib::db::Database;
use amazon_s3_drive_lib::models::{
    AppConfig, FileEntry, LocalFsEvent, S3Error, SyncOperation, SyncOperationType, SyncState,
};
use amazon_s3_drive_lib::network_detector::NetworkDetector;
use amazon_s3_drive_lib::queue_manager::QueueManager;
use amazon_s3_drive_lib::s3_client::{compute_md5_bytes, MockObject, S3Client};
use amazon_s3_drive_lib::status_monitor::StatusMonitor;
use amazon_s3_drive_lib::sync_engine::{RecordingEmitter, RetryPolicy, SyncEngine};
use chrono::{Duration, Utc};
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_small_file_upload_end_to_end() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let qm = Arc::new(QueueManager::new(&db));
    let nd = NetworkDetector::new();
    let sm = Arc::new(StatusMonitor::new(
        Some(nd.clone()),
        Some(dir.path().to_path_buf()),
    ));
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = Arc::new(
        SyncEngine::new(
            cfg,
            Some(s3_client),
            db,
            qm,
            sm.clone(),
            nd,
            emitter.clone(),
        )
        .with_retry_policy(RetryPolicy::fast_for_tests()),
    );

    let file_path = dir.path().join("hello.txt");
    let content = b"Hello Amazon S3 Drive < 100MB!";
    fs::write(&file_path, content).unwrap();

    engine
        .process_local_event(LocalFsEvent::Created(file_path))
        .await
        .unwrap();
    engine.process_queue().await.unwrap();

    let guard = mock_backend.lock().unwrap();
    let uploaded = guard
        .objects
        .get("hello.txt")
        .expect("File should be uploaded to S3");
    assert_eq!(uploaded.bytes, content);
    assert_eq!(uploaded.etag, compute_md5_bytes(content));
}

#[tokio::test]
async fn test_multipart_upload_over_100mb_with_part_verification() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    // Criar arquivo esparso/real de 101 MB (> 100 MB) para acionar multipart upload
    let file_path = dir.path().join("large_video.bin");
    let size: u64 = 101 * 1024 * 1024;
    {
        let f = fs::File::create(&file_path).unwrap();
        f.set_len(size).unwrap();
    }

    assert!(S3Client::requires_multipart(size));

    let etag = s3_client
        .upload_multipart("large_video.bin", &file_path, None)
        .await
        .unwrap();
    assert!(!etag.is_empty());

    let guard = mock_backend.lock().unwrap();
    assert_eq!(guard.multipart_uploads.len(), 1);
    let (key, completed_parts) = &guard.multipart_uploads[0];
    assert_eq!(key, "large_video.bin");
    // 101 MB / 8 MB = 12 partes de 8 MB (96 MB) + 1 parte de 5 MB = 13 partes
    assert_eq!(completed_parts.len(), 13);
    for part in &completed_parts[..completed_parts.len() - 1] {
        assert!(part.size >= 5 * 1024 * 1024);
    }
    let total_uploaded: u64 = completed_parts.iter().map(|p| p.size).sum();
    assert_eq!(total_uploaded, size);
}

#[tokio::test]
async fn test_download_with_valid_and_invalid_checksum() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db));

    let payload = b"Conteudo integro no S3".to_vec();
    let etag = compute_md5_bytes(&payload);
    {
        let mut guard = mock_backend.lock().unwrap();
        guard.objects.insert(
            "remote.txt".to_string(),
            MockObject {
                bytes: payload.clone(),
                etag: etag.clone(),
                last_modified: Utc::now(),
            },
        );
    }

    // 1. Download com checksum válido -> sucesso
    let dest_ok = dir.path().join("remote_ok.txt");
    let returned_etag = s3_client
        .download_file("remote.txt", &dest_ok)
        .await
        .unwrap();
    assert_eq!(returned_etag, etag);
    assert_eq!(fs::read(&dest_ok).unwrap(), payload);

    // 2. Download com checksum inválido (corrompido) -> descarta temporário e retorna ChecksumMismatch
    {
        let mut guard = mock_backend.lock().unwrap();
        guard.corrupt_downloads = true;
    }
    let dest_bad = dir.path().join("remote_corrupt.txt");
    let err = s3_client
        .download_file("remote.txt", &dest_bad)
        .await
        .unwrap_err();
    assert!(matches!(err, S3Error::ChecksumMismatch { .. }));
    assert!(
        !dest_bad.exists(),
        "Corrupted file must not exist at destination"
    );
    assert!(
        !dest_bad.with_extension("tmp").exists(),
        "Temp file must be discarded on checksum failure"
    );
}

#[tokio::test]
async fn test_conflict_detection_and_resolution_integration() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    let local_file = dir.path().join("nota.txt");
    fs::write(&local_file, b"versao modificada localmente").unwrap();

    let last_sync = Utc::now() - Duration::minutes(10);
    let local_mod = Utc::now() - Duration::minutes(2);
    let remote_mod = Utc::now() - Duration::minutes(1);

    db.upsert_file_entry(&FileEntry {
        s3_key: "nota.txt".to_string(),
        local_path: local_file.to_string_lossy().to_string(),
        sync_state: SyncState::Synced,
        last_sync_etag: Some("etag-antigo".to_string()),
        local_hash: None,
        last_sync_time: Some(last_sync),
        local_modified_time: Some(local_mod),
        size_bytes: 28,
    })
    .unwrap();

    let remote_bytes = b"versao modificada remotamente".to_vec();
    {
        let mut guard = mock_backend.lock().unwrap();
        guard.objects.insert(
            "nota.txt".to_string(),
            MockObject {
                etag: compute_md5_bytes(&remote_bytes),
                bytes: remote_bytes.clone(),
                last_modified: remote_mod,
            },
        );
    }

    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let qm = Arc::new(QueueManager::new(&db));
    let nd = NetworkDetector::new();
    let sm = Arc::new(StatusMonitor::default());
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = SyncEngine::new(cfg, Some(s3_client), db, qm, sm, nd, emitter.clone());
    engine.poll_remote_changes().await.unwrap();

    // Verificar que ambas as versões foram preservadas e o evento conflict:detected foi emitido
    assert_eq!(
        fs::read_to_string(&local_file).unwrap(),
        "versao modificada localmente"
    );

    let events = emitter.events.lock().unwrap();
    let conflict_ev = events
        .iter()
        .find(|(name, _)| name == "conflict:detected")
        .expect("conflict:detected event should be emitted");

    let conflict_copy_path = conflict_ev.1["conflict_copy_path"].as_str().unwrap();
    assert!(conflict_copy_path.contains("(conflito-"));
    assert_eq!(fs::read(conflict_copy_path).unwrap(), remote_bytes);
}

#[tokio::test]
async fn test_offline_queue_processed_after_reconnection() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let qm = Arc::new(QueueManager::new(&db));
    let nd = NetworkDetector::new();
    let sm = Arc::new(StatusMonitor::new(Some(nd.clone()), None));
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = Arc::new(
        SyncEngine::new(
            cfg,
            Some(s3_client),
            db,
            qm.clone(),
            sm,
            nd.clone(),
            emitter,
        )
        .with_retry_policy(RetryPolicy::fast_for_tests()),
    );

    // Ficar offline
    nd.set_online(false);

    let f1 = dir.path().join("offline1.txt");
    let f2 = dir.path().join("offline2.txt");
    fs::write(&f1, b"arquivo 1 offline").unwrap();
    fs::write(&f2, b"arquivo 2 offline").unwrap();

    engine
        .process_local_event(LocalFsEvent::Created(f1))
        .await
        .unwrap();
    engine
        .process_local_event(LocalFsEvent::Created(f2))
        .await
        .unwrap();

    // Enquanto offline, a fila mantém os 2 itens pendentes
    engine.process_queue().await.unwrap();
    assert_eq!(qm.count_pending().unwrap(), 2);

    // Simular reconexão e processar fila
    nd.set_online(true);
    engine.process_queue().await.unwrap();

    assert_eq!(qm.count_pending().unwrap(), 0);
    let guard = mock_backend.lock().unwrap();
    assert!(guard.objects.contains_key("offline1.txt"));
    assert!(guard.objects.contains_key("offline2.txt"));
}

#[tokio::test]
async fn test_invalid_credentials_rejected_on_verify_access() {
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", None);
    {
        let mut guard = mock_backend.lock().unwrap();
        guard.fail_auth = true;
    }

    let creds_mgr = CredentialsManager::new();
    let err = creds_mgr.verify_access(&s3_client).await.unwrap_err();
    assert!(err.to_string().contains("InvalidAccessKeyId") || err.to_string().contains("failed"));
}

#[tokio::test]
async fn test_s3_folder_marker_and_nested_file_download_without_os_error_183() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    // Simular arquivo 0-byte "boletos" criado anteriormente no disco no lugar da pasta
    let stale_file_as_dir = dir.path().join("boletos");
    fs::write(&stale_file_as_dir, b"").unwrap();
    assert!(stale_file_as_dir.is_file());

    let pdf_bytes = b"%PDF-1.4 Condominio boleto content".to_vec();
    {
        let mut guard = mock_backend.lock().unwrap();
        // Objeto marcador de pasta S3 ("boletos/") + arquivo dentro da pasta
        guard.objects.insert(
            "boletos/".to_string(),
            MockObject {
                bytes: Vec::new(),
                etag: compute_md5_bytes(&[]),
                last_modified: Utc::now(),
            },
        );
        guard.objects.insert(
            "boletos/Condominio - Ex Atlas Garantia.pdf".to_string(),
            MockObject {
                bytes: pdf_bytes.clone(),
                etag: compute_md5_bytes(&pdf_bytes),
                last_modified: Utc::now(),
            },
        );
    }

    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let qm = Arc::new(QueueManager::new(&db));
    let nd = NetworkDetector::new();
    let sm = Arc::new(StatusMonitor::default());
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = Arc::new(
        SyncEngine::new(cfg, Some(s3_client), db, qm, sm, nd, emitter)
            .with_retry_policy(RetryPolicy::fast_for_tests()),
    );

    engine.poll_remote_changes().await.unwrap();
    engine.process_queue().await.unwrap();

    assert!(
        stale_file_as_dir.is_dir(),
        "'boletos' must be converted into a directory"
    );
    let downloaded_pdf = stale_file_as_dir.join("Condominio - Ex Atlas Garantia.pdf");
    assert!(downloaded_pdf.is_file());
    assert_eq!(fs::read(&downloaded_pdf).unwrap(), pdf_bytes);
}

#[tokio::test]
async fn test_processing_items_recovered_after_crash() {
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    let content = b"content that was being uploaded during crash";
    let file_path = dir.path().join("interrupted.txt");
    fs::write(&file_path, content).unwrap();

    let qm = Arc::new(QueueManager::new(&db));

    // Enqueue and immediately dequeue → marks row as 'Processing', simulating a crash mid-upload
    qm.enqueue(SyncOperation {
        operation_type: SyncOperationType::Upload,
        s3_key: "interrupted.txt".to_string(),
        local_path: file_path.to_string_lossy().to_string(),
        extra: None,
    })
    .unwrap();
    qm.dequeue_next().unwrap(); // row is now 'Processing'
    assert_eq!(qm.count_pending().unwrap(), 0, "no Pending rows yet");

    // Simulate restart: recover_interrupted_items() resets Processing → Pending
    let recovered = qm.recover_interrupted_items().unwrap();
    assert_eq!(recovered, 1, "one interrupted item recovered");
    assert_eq!(qm.count_pending().unwrap(), 1, "item is now Pending again");

    // Build an engine with the same in-memory DB and drain the queue
    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let nd = NetworkDetector::new();
    let sm = Arc::new(StatusMonitor::new(Some(nd.clone()), None));
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = Arc::new(
        SyncEngine::new(cfg, Some(s3_client), db, qm.clone(), sm, nd, emitter)
            .with_retry_policy(RetryPolicy::fast_for_tests()),
    );

    engine.process_queue().await.unwrap();

    let guard = mock_backend.lock().unwrap();
    assert!(
        guard.objects.contains_key("interrupted.txt"),
        "recovered item must be uploaded to S3"
    );
}

#[tokio::test]
async fn test_reconnect_callback_drains_queue_automatically() {
    // Verifica que a fila é processada AUTOMATICAMENTE pelo callback de reconexão,
    // sem precisar chamar process_queue() manualmente.
    let dir = tempdir().unwrap();
    let db = Database::new_in_memory().unwrap();
    let (s3_client, mock_backend) = S3Client::new_mock("my-test-bucket", Some(db.clone()));

    let mut cfg = AppConfig::default();
    cfg.bucket_name = "my-test-bucket".to_string();
    cfg.local_path = dir.path().to_string_lossy().to_string();
    cfg.aws_region = "us-east-1".to_string();

    let nd = NetworkDetector::new();
    let qm = Arc::new(QueueManager::new(&db));
    let sm = Arc::new(StatusMonitor::new(Some(nd.clone()), None));
    let emitter = Arc::new(RecordingEmitter::default());

    let engine = Arc::new(
        SyncEngine::new(
            cfg,
            Some(s3_client),
            db,
            qm.clone(),
            sm,
            nd.clone(),
            emitter,
        )
        .with_retry_policy(RetryPolicy::fast_for_tests()),
    );

    // Registrar o mesmo callback de reconexão que start() registraria
    let engine_for_cb = Arc::clone(&engine);
    nd.on_state_change(move |online| {
        if online {
            let eng = Arc::clone(&engine_for_cb);
            tokio::spawn(async move {
                let _ = eng.queue_manager().requeue_failed_items();
                let _ = eng.process_queue().await;
            });
        }
    });

    // Ficar offline e enfileirar dois arquivos via eventos locais
    nd.set_online(false);

    let f1 = dir.path().join("cb_offline1.txt");
    let f2 = dir.path().join("cb_offline2.txt");
    fs::write(&f1, b"arquivo offline via callback 1").unwrap();
    fs::write(&f2, b"arquivo offline via callback 2").unwrap();

    engine
        .process_local_event(LocalFsEvent::Created(f1))
        .await
        .unwrap();
    engine
        .process_local_event(LocalFsEvent::Created(f2))
        .await
        .unwrap();

    // process_queue enquanto offline é no-op
    engine.process_queue().await.unwrap();
    assert_eq!(
        qm.count_pending().unwrap(),
        2,
        "dois itens devem estar pendentes offline"
    );

    // Disparar reconexão — o callback spawna process_queue automaticamente
    nd.set_online(true);

    // Aguardar a task assíncrona do callback concluir
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    assert_eq!(
        qm.count_pending().unwrap(),
        0,
        "callback deve ter drenado a fila"
    );
    let guard = mock_backend.lock().unwrap();
    assert!(
        guard.objects.contains_key("cb_offline1.txt"),
        "arquivo 1 deve estar no S3"
    );
    assert!(
        guard.objects.contains_key("cb_offline2.txt"),
        "arquivo 2 deve estar no S3"
    );
}
