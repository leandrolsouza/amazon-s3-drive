use crate::db::Database;
use crate::models::{
    AwsCredentials, CompletedPart, MultipartState, PartRange, S3Error, S3ObjectInfo,
    S3ObjectVersion,
};
use aws_credential_types::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{
    CompletedMultipartUpload, CompletedPart as SdkCompletedPart, ServerSideEncryption, StorageClass,
};
use chrono::{DateTime, Duration, Utc};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;

pub const MIN_PART_SIZE: u64 = 5 * 1024 * 1024; // 5 MB
pub const DEFAULT_PART_SIZE: u64 = 8 * 1024 * 1024; // 8 MB
pub const MULTIPART_THRESHOLD: u64 = 100 * 1024 * 1024; // 100 MB

#[derive(Debug, Clone)]
pub struct MockObject {
    pub bytes: Vec<u8>,
    pub etag: String,
    pub last_modified: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct MockS3Backend {
    pub objects: HashMap<String, MockObject>,
    pub fail_auth: bool,
    pub corrupt_downloads: bool,
    pub multipart_uploads: Vec<(String, Vec<CompletedPart>)>,
    pub fail_delete: bool,
}

#[derive(Clone)]
enum ClientBackend {
    Aws(aws_sdk_s3::Client),
    Mock(Arc<Mutex<MockS3Backend>>),
}

#[derive(Clone)]
pub struct S3Client {
    backend: ClientBackend,
    bucket: String,
    db: Option<Database>,
    storage_class: String,
    server_side_encryption: bool,
    bandwidth_limit_kbps: u64,
}

pub fn normalize_etag(etag: &str) -> String {
    etag.trim().trim_matches('"').to_string()
}

pub fn compute_md5_bytes(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub fn compute_md5(path: &Path) -> Result<String, S3Error> {
    let mut file = File::open(path).map_err(|e| S3Error::Io(e.to_string()))?;
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| S3Error::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn validate_checksum_bytes(bytes: &[u8], etag: &str) -> bool {
    let normalized = normalize_etag(etag);
    // Para objetos enviados via multipart, o ETag tem sufixo "-N"
    if normalized.contains('-') {
        return !normalized.is_empty();
    }
    compute_md5_bytes(bytes).eq_ignore_ascii_case(&normalized)
}

pub fn validate_checksum(path: &Path, etag: &str) -> Result<bool, S3Error> {
    let normalized = normalize_etag(etag);
    if normalized.contains('-') {
        return Ok(!normalized.is_empty());
    }
    let file_md5 = compute_md5(path)?;
    Ok(file_md5.eq_ignore_ascii_case(&normalized))
}

/// Garante que o diretório (e seus pais) existam. Se algum ancestral existir como arquivo
/// (ex: marcador de pasta 0-byte do S3 baixado anteriormente como arquivo), remove o arquivo
/// antes de criar o diretório para evitar `os error 183` no Windows.
pub fn ensure_dir_exists(dir: &Path) -> Result<(), S3Error> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    let mut ancestors: Vec<&Path> = dir.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if let Ok(meta) = fs::symlink_metadata(ancestor) {
            if meta.is_file() {
                let _ = fs::remove_file(ancestor);
            }
        }
    }
    fs::create_dir_all(dir).map_err(|e| S3Error::Io(e.to_string()))
}

impl S3Client {
    pub async fn new(
        creds: &AwsCredentials,
        bucket: String,
        db: Option<Database>,
    ) -> Result<Self, S3Error> {
        Self::new_with_options(creds, bucket, db, None, false).await
    }

    pub async fn new_with_options(
        creds: &AwsCredentials,
        bucket: String,
        db: Option<Database>,
        endpoint_url: Option<String>,
        force_path_style: bool,
    ) -> Result<Self, S3Error> {
        let region = aws_config::Region::new(creds.region.clone());
        let sdk_config = if creds.use_iam_role {
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(region)
                .load()
                .await
        } else {
            let credentials = Credentials::new(
                creds.access_key_id.clone(),
                creds.secret_access_key.clone(),
                creds.session_token.clone(),
                None,
                "amazon-s3-drive",
            );
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .region(region)
                .credentials_provider(credentials)
                .load()
                .await
        };

        let mut s3_config_builder =
            aws_sdk_s3::config::Builder::from(&sdk_config).force_path_style(force_path_style);

        if let Some(ep) = endpoint_url {
            let trimmed = ep.trim();
            if !trimmed.is_empty() {
                s3_config_builder = s3_config_builder.endpoint_url(trimmed);
            }
        }

        let inner = aws_sdk_s3::Client::from_conf(s3_config_builder.build());
        Ok(Self {
            backend: ClientBackend::Aws(inner),
            bucket,
            db,
            storage_class: "STANDARD".to_string(),
            server_side_encryption: false,
            bandwidth_limit_kbps: 0,
        })
    }

    pub fn with_storage_options(mut self, storage_class: &str, sse: bool) -> Self {
        if !storage_class.trim().is_empty() {
            self.storage_class = storage_class.trim().to_string();
        }
        self.server_side_encryption = sse;
        self
    }

    pub fn with_bandwidth_limit(mut self, limit_kbps: u64) -> Self {
        self.bandwidth_limit_kbps = limit_kbps;
        self
    }

    async fn apply_bandwidth_throttle(&self, bytes_count: u64) {
        if self.bandwidth_limit_kbps > 0 && bytes_count > 0 {
            let bytes_per_sec = (self.bandwidth_limit_kbps as f64) * 1024.0;
            let delay_ms = ((bytes_count as f64 / bytes_per_sec) * 1000.0).min(15_000.0) as u64;
            if delay_ms > 5 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }
    }

    fn sdk_storage_class(&self) -> StorageClass {
        StorageClass::from(self.storage_class.as_str())
    }

    pub fn new_mock(bucket: &str, db: Option<Database>) -> (Self, Arc<Mutex<MockS3Backend>>) {
        let mock = Arc::new(Mutex::new(MockS3Backend::default()));
        let client = Self {
            backend: ClientBackend::Mock(Arc::clone(&mock)),
            bucket: bucket.to_string(),
            db,
            storage_class: "STANDARD".to_string(),
            server_side_encryption: false,
            bandwidth_limit_kbps: 0,
        };
        (client, mock)
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Determina se o arquivo requer multipart (tamanho > 100 MB) — Requisito 2.6
    pub fn requires_multipart(file_size: u64) -> bool {
        file_size > MULTIPART_THRESHOLD
    }

    /// Calcula as partes para multipart upload (todas >= 5 MB, exceto a última) — Requisito 2.6
    pub fn compute_parts(file_size: u64, part_size: u64) -> Vec<PartRange> {
        if file_size == 0 {
            return Vec::new();
        }
        let min_for_10k = (file_size + 9_999) / 10_000;
        let effective_part_size = part_size.max(MIN_PART_SIZE).max(min_for_10k);

        let mut parts = Vec::new();
        let mut offset = 0u64;
        let mut part_number = 1i32;

        while offset < file_size {
            let remaining = file_size - offset;
            let size = remaining.min(effective_part_size);
            parts.push(PartRange {
                part_number,
                offset,
                size,
            });
            offset += size;
            part_number += 1;
        }

        parts
    }

    pub async fn test_access(&self) -> Result<(), S3Error> {
        self.test_prefix_access(None).await
    }

    pub(crate) async fn test_prefix_access(&self, prefix: Option<&str>) -> Result<(), S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                client
                    .list_objects_v2()
                    .bucket(&self.bucket)
                    .set_prefix(prefix.map(str::to_owned))
                    .max_keys(1)
                    .send()
                    .await
                    .map_err(|e| S3Error::Auth(e.to_string()))?;
                Ok(())
            }
            ClientBackend::Mock(mock) => {
                let guard = mock.lock().unwrap();
                if guard.fail_auth {
                    return Err(S3Error::Auth(
                        "InvalidAccessKeyId or AccessDenied".to_string(),
                    ));
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn discover_buckets(&self) -> Result<Vec<(String, Option<String>)>, S3Error> {
        use aws_sdk_s3::error::ProvideErrorMetadata;

        let ClientBackend::Aws(client) = &self.backend else {
            return Err(S3Error::Sdk("Cliente AWS necessário.".into()));
        };
        let mut pages = client.list_buckets().max_buckets(1000).into_paginator().send();
        let mut buckets = Vec::new();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|error| {
                S3Error::Sdk(error.as_service_error().and_then(|err| err.code())
                    .unwrap_or("Falha ao listar os buckets.").to_string())
            })?;
            buckets.extend(page.buckets().iter().filter_map(|bucket| {
                Some((bucket.name()?.to_owned(), bucket.bucket_region().map(str::to_owned)))
            }));
        }
        buckets.sort_unstable();
        Ok(buckets)
    }

    pub(crate) async fn create_private_bucket(&self, region: &str) -> Result<(), S3Error> {
        use aws_sdk_s3::error::ProvideErrorMetadata;
        use aws_sdk_s3::types::{BucketLocationConstraint, CreateBucketConfiguration};

        let ClientBackend::Aws(client) = &self.backend else {
            return Err(S3Error::Sdk("Cliente AWS necessário.".into()));
        };
        // Em us-east-1, repetir CreateBucket em um bucket próprio pode alterar ACLs legadas.
        if region == "us-east-1" {
            match client.head_bucket().bucket(&self.bucket).send().await {
                Err(error) if error.raw_response().is_some_and(|r| r.status().as_u16() == 404) => {}
                _ => return Err(S3Error::Sdk(
                    "O nome já está em uso ou não foi possível confirmar sua disponibilidade.".into(),
                )),
            }
        }
        let mut request = client.create_bucket().bucket(&self.bucket);
        if region != "us-east-1" {
            request = request.create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::from(region))
                    .build(),
            );
        }
        // Preserva os padrões do S3 de bloqueio de acesso público e ACLs desativadas.
        request.send().await.map_err(|error| {
            let code = error.as_service_error().and_then(|err| err.code());
            let message = match code {
                Some("BucketAlreadyExists") => "Esse nome de bucket já está em uso.",
                Some("BucketAlreadyOwnedByYou") => "Esse bucket já existe na sua conta. Adicione-o como existente.",
                Some("AccessDenied") => "A identidade AWS precisa de s3:CreateBucket.",
                Some(code) => code,
                None => "O S3 não confirmou a criação. Consulte a lista de buckets antes de tentar novamente.",
            };
            S3Error::Sdk(message.into())
        })?;
        Ok(())
    }

    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<S3ObjectInfo>, S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                let mut result = Vec::new();
                let mut continuation_token: Option<String> = None;

                loop {
                    let mut req = client.list_objects_v2().bucket(&self.bucket);
                    if let Some(p) = prefix {
                        if !p.is_empty() {
                            req = req.prefix(p);
                        }
                    }
                    if let Some(ref token) = continuation_token {
                        req = req.continuation_token(token);
                    }

                    let resp = req.send().await.map_err(|e| S3Error::Sdk(e.to_string()))?;

                    for obj in resp.contents() {
                        if let Some(key) = obj.key() {
                            let etag = obj.e_tag().map(normalize_etag).unwrap_or_default();
                            let size = obj.size().unwrap_or(0).max(0) as u64;
                            let last_modified = obj
                                .last_modified()
                                .and_then(|dt| {
                                    DateTime::<Utc>::from_timestamp(dt.secs(), dt.subsec_nanos())
                                })
                                .unwrap_or_else(Utc::now);

                            result.push(S3ObjectInfo {
                                key: key.to_string(),
                                etag,
                                size,
                                last_modified,
                            });
                        }
                    }

                    if resp.is_truncated().unwrap_or(false) {
                        if let Some(next_token) = resp.next_continuation_token() {
                            continuation_token = Some(next_token.to_string());
                            continue;
                        }
                    }
                    break;
                }

                Ok(result)
            }
            ClientBackend::Mock(mock) => {
                let guard = mock.lock().unwrap();
                if guard.fail_auth {
                    return Err(S3Error::Auth("Credentials expired".to_string()));
                }
                let mut list: Vec<S3ObjectInfo> = guard
                    .objects
                    .iter()
                    .filter(|(k, _)| match prefix {
                        Some(p) if !p.is_empty() => k.starts_with(p),
                        _ => true,
                    })
                    .map(|(k, v)| S3ObjectInfo {
                        key: k.clone(),
                        etag: normalize_etag(&v.etag),
                        size: v.bytes.len() as u64,
                        last_modified: v.last_modified,
                    })
                    .collect();
                list.sort_by(|a, b| a.key.cmp(&b.key));
                Ok(list)
            }
        }
    }

    pub async fn head_object(&self, key: &str) -> Result<S3ObjectInfo, S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                let resp = client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;

                let etag = resp.e_tag().map(normalize_etag).unwrap_or_default();
                let size = resp.content_length().unwrap_or(0).max(0) as u64;
                let last_modified = resp
                    .last_modified()
                    .and_then(|dt| DateTime::<Utc>::from_timestamp(dt.secs(), dt.subsec_nanos()))
                    .unwrap_or_else(Utc::now);

                Ok(S3ObjectInfo {
                    key: key.to_string(),
                    etag,
                    size,
                    last_modified,
                })
            }
            ClientBackend::Mock(mock) => {
                let guard = mock.lock().unwrap();
                let obj = guard
                    .objects
                    .get(key)
                    .ok_or_else(|| S3Error::Sdk(format!("NoSuchKey: {}", key)))?;
                Ok(S3ObjectInfo {
                    key: key.to_string(),
                    etag: normalize_etag(&obj.etag),
                    size: obj.bytes.len() as u64,
                    last_modified: obj.last_modified,
                })
            }
        }
    }

    pub async fn delete_object(&self, key: &str) -> Result<(), S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;
                Ok(())
            }
            ClientBackend::Mock(mock) => {
                let mut guard = mock.lock().unwrap();
                if guard.fail_delete {
                    return Err(S3Error::Sdk("Simulated delete error".to_string()));
                }
                guard.objects.remove(key);
                Ok(())
            }
        }
    }

    /// Exclusão condicional: uma edição concorrente nunca deve ser apagada pela decisão anterior.
    pub async fn delete_object_if_match(&self, key: &str, etag: &str) -> Result<(), S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                client
                    .delete_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .if_match(format!("\"{}\"", normalize_etag(etag)))
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;
                Ok(())
            }
            ClientBackend::Mock(mock) => {
                let mut guard = mock.lock().unwrap();
                if guard.fail_delete
                    || guard.objects.get(key).map(|o| normalize_etag(&o.etag))
                        != Some(normalize_etag(etag))
                {
                    return Err(S3Error::Sdk(
                        "Exclusão cancelada: versão mudou ou remoção falhou.".into(),
                    ));
                }
                guard.objects.remove(key);
                Ok(())
            }
        }
    }

    pub async fn copy_object(&self, src: &str, dst: &str) -> Result<(), S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                let copy_source: String = format!("{}/{}", self.bucket, src)
                    .bytes()
                    .map(|byte| {
                        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(&byte) {
                            (byte as char).to_string()
                        } else {
                            format!("%{byte:02X}")
                        }
                    })
                    .collect();
                client
                    .copy_object()
                    .bucket(&self.bucket)
                    .copy_source(copy_source)
                    .key(dst)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;
                Ok(())
            }
            ClientBackend::Mock(mock) => {
                let mut guard = mock.lock().unwrap();
                let existing = guard
                    .objects
                    .get(src)
                    .cloned()
                    .ok_or_else(|| S3Error::Sdk(format!("Source key not found: {}", src)))?;
                guard.objects.insert(dst.to_string(), existing);
                Ok(())
            }
        }
    }

    pub async fn upload_file(&self, key: &str, path: &Path) -> Result<String, S3Error> {
        let bytes = fs::read(path).map_err(|e| S3Error::Io(e.to_string()))?;
        self.apply_bandwidth_throttle(bytes.len() as u64).await;
        let computed_etag = compute_md5_bytes(&bytes);

        match &self.backend {
            ClientBackend::Aws(client) => {
                let body = ByteStream::from(bytes);
                let mut req = client
                    .put_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .storage_class(self.sdk_storage_class())
                    .body(body);
                if self.server_side_encryption {
                    req = req.server_side_encryption(ServerSideEncryption::Aes256);
                }
                let resp = req.send().await.map_err(|e| S3Error::Sdk(e.to_string()))?;
                let etag = resp.e_tag().map(normalize_etag).unwrap_or(computed_etag);
                Ok(etag)
            }
            ClientBackend::Mock(mock) => {
                let mut guard = mock.lock().unwrap();
                if guard.fail_auth {
                    return Err(S3Error::Auth("AccessDenied".to_string()));
                }
                guard.objects.insert(
                    key.to_string(),
                    MockObject {
                        bytes,
                        etag: computed_etag.clone(),
                        last_modified: Utc::now(),
                    },
                );
                Ok(computed_etag)
            }
        }
    }

    pub async fn upload_multipart(
        &self,
        key: &str,
        path: &Path,
        resume: Option<MultipartState>,
    ) -> Result<String, S3Error> {
        self.upload_multipart_with_progress(key, path, resume, |_, _| {})
            .await
    }

    pub async fn upload_multipart_with_progress<F>(
        &self,
        key: &str,
        path: &Path,
        resume: Option<MultipartState>,
        mut on_progress: F,
    ) -> Result<String, S3Error>
    where
        F: FnMut(u64, u64),
    {
        let metadata = fs::metadata(path).map_err(|e| S3Error::Io(e.to_string()))?;
        let file_size = metadata.len();
        let parts = Self::compute_parts(file_size, DEFAULT_PART_SIZE);

        // Verificar estado de retomada (expira após 24 horas — Requisito 2.7)
        let now = Utc::now();
        let valid_resume = resume
            .filter(|state| now.signed_duration_since(state.created_at) < Duration::hours(24));

        let mut state = if let Some(existing) = valid_resume {
            existing
        } else {
            let upload_id = match &self.backend {
                ClientBackend::Aws(client) => {
                    let mut req = client
                        .create_multipart_upload()
                        .bucket(&self.bucket)
                        .key(key)
                        .storage_class(self.sdk_storage_class());
                    if self.server_side_encryption {
                        req = req.server_side_encryption(ServerSideEncryption::Aes256);
                    }
                    let resp = req.send().await.map_err(|e| S3Error::Sdk(e.to_string()))?;
                    resp.upload_id()
                        .ok_or_else(|| S3Error::Sdk("Missing upload_id".to_string()))?
                        .to_string()
                }
                ClientBackend::Mock(_) => format!("mock-upload-{}", now.timestamp_millis()),
            };

            MultipartState {
                s3_key: key.to_string(),
                upload_id,
                local_path: path.to_string_lossy().to_string(),
                created_at: now,
                completed_parts: Vec::new(),
            }
        };

        if let Some(db) = &self.db {
            let _ = db.save_multipart_state(&state);
        }

        let mut file = File::open(path).map_err(|e| S3Error::Io(e.to_string()))?;
        let mut full_bytes_for_mock = Vec::new();
        let mut bytes_sent = 0u64;

        for part in &parts {
            if let Some(done) = state
                .completed_parts
                .iter()
                .find(|cp| cp.part_number == part.part_number)
            {
                if matches!(self.backend, ClientBackend::Mock(_)) {
                    let mut buf = vec![0u8; part.size as usize];
                    file.seek(SeekFrom::Start(part.offset))
                        .map_err(|e| S3Error::Io(e.to_string()))?;
                    file.read_exact(&mut buf)
                        .map_err(|e| S3Error::Io(e.to_string()))?;
                    full_bytes_for_mock.extend_from_slice(&buf);
                }
                bytes_sent = bytes_sent.saturating_add(done.size);
                on_progress(bytes_sent, file_size);
                continue;
            }

            let mut buf = vec![0u8; part.size as usize];
            file.seek(SeekFrom::Start(part.offset))
                .map_err(|e| S3Error::Io(e.to_string()))?;
            file.read_exact(&mut buf)
                .map_err(|e| S3Error::Io(e.to_string()))?;

            let part_etag = match &self.backend {
                ClientBackend::Aws(client) => {
                    let resp = client
                        .upload_part()
                        .bucket(&self.bucket)
                        .key(key)
                        .upload_id(&state.upload_id)
                        .part_number(part.part_number)
                        .body(ByteStream::from(buf))
                        .send()
                        .await
                        .map_err(|e| S3Error::Sdk(e.to_string()))?;
                    resp.e_tag().map(normalize_etag).unwrap_or_default()
                }
                ClientBackend::Mock(_) => {
                    let etag = compute_md5_bytes(&buf);
                    full_bytes_for_mock.extend_from_slice(&buf);
                    etag
                }
            };

            state.completed_parts.push(CompletedPart {
                part_number: part.part_number,
                etag: part_etag,
                byte_offset: part.offset,
                size: part.size,
            });

            bytes_sent = bytes_sent.saturating_add(part.size);
            on_progress(bytes_sent, file_size);
            self.apply_bandwidth_throttle(part.size).await;

            if let Some(db) = &self.db {
                let _ = db.save_multipart_state(&state);
            }
        }

        let final_etag = match &self.backend {
            ClientBackend::Aws(client) => {
                let sdk_parts: Vec<SdkCompletedPart> = state
                    .completed_parts
                    .iter()
                    .map(|cp| {
                        SdkCompletedPart::builder()
                            .part_number(cp.part_number)
                            .e_tag(&cp.etag)
                            .build()
                    })
                    .collect();

                let completed_upload = CompletedMultipartUpload::builder()
                    .set_parts(Some(sdk_parts))
                    .build();

                let resp = client
                    .complete_multipart_upload()
                    .bucket(&self.bucket)
                    .key(key)
                    .upload_id(&state.upload_id)
                    .multipart_upload(completed_upload)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;

                resp.e_tag().map(normalize_etag).unwrap_or_default()
            }
            ClientBackend::Mock(mock) => {
                let etag = compute_md5_bytes(&full_bytes_for_mock);
                let mut guard = mock.lock().unwrap();
                guard
                    .multipart_uploads
                    .push((key.to_string(), state.completed_parts.clone()));
                guard.objects.insert(
                    key.to_string(),
                    MockObject {
                        bytes: full_bytes_for_mock,
                        etag: etag.clone(),
                        last_modified: Utc::now(),
                    },
                );
                etag
            }
        };

        if let Some(db) = &self.db {
            let _ = db.delete_multipart_state(key);
        }

        Ok(final_etag)
    }

    pub async fn download_file(&self, key: &str, dest: &Path) -> Result<String, S3Error> {
        // Objetos marcadores de diretório no S3 (terminados em '/') devem criar apenas a pasta local
        if key.ends_with('/') {
            ensure_dir_exists(dest)?;
            return Ok(String::new());
        }

        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                ensure_dir_exists(parent)?;
            }
        }

        let temp_path = dest.with_extension("tmp");
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }

        let expected_etag = match &self.backend {
            ClientBackend::Aws(client) => {
                let resp = client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;

                let etag = resp.e_tag().map(normalize_etag).unwrap_or_default();

                // Transmitir o corpo diretamente para o arquivo temporário sem
                // carregar o objeto inteiro em memória (Requisito 3.5).
                let mut reader = resp.body.into_async_read();
                let mut tmp_file = tokio::fs::File::create(&temp_path)
                    .await
                    .map_err(|e| S3Error::Io(e.to_string()))?;
                tokio::io::copy(&mut reader, &mut tmp_file)
                    .await
                    .map_err(|e| S3Error::Sdk(format!("Erro ao transmitir '{}': {}", key, e)))?;
                tmp_file
                    .flush()
                    .await
                    .map_err(|e| S3Error::Io(e.to_string()))?;
                drop(tmp_file);

                etag
            }
            ClientBackend::Mock(mock) => {
                let (bytes, etag, corrupt) = {
                    let guard = mock.lock().unwrap();
                    let obj = guard
                        .objects
                        .get(key)
                        .ok_or_else(|| S3Error::Sdk(format!("NoSuchKey: {}", key)))?;
                    (
                        obj.bytes.clone(),
                        normalize_etag(&obj.etag),
                        guard.corrupt_downloads,
                    )
                };

                let mut tmp_file =
                    File::create(&temp_path).map_err(|e| S3Error::Io(e.to_string()))?;
                if corrupt {
                    let mut corrupted = bytes;
                    corrupted.push(0xFF);
                    tmp_file
                        .write_all(&corrupted)
                        .map_err(|e| S3Error::Io(e.to_string()))?;
                } else {
                    tmp_file
                        .write_all(&bytes)
                        .map_err(|e| S3Error::Io(e.to_string()))?;
                }
                tmp_file.flush().map_err(|e| S3Error::Io(e.to_string()))?;
                etag
            }
        };

        // Requisito 3.6 e 3.7: validar integridade via MD5 checksum contra ETag
        let actual_md5 = compute_md5(&temp_path)?;
        let is_valid = validate_checksum(&temp_path, &expected_etag)?;

        if !is_valid {
            let _ = fs::remove_file(&temp_path);
            return Err(S3Error::ChecksumMismatch {
                file: key.to_string(),
                expected: expected_etag,
                received: actual_md5,
            });
        }

        if dest.exists() && dest.is_file() {
            let _ = fs::remove_file(dest);
        }
        let downloaded_len = fs::metadata(&temp_path).map(|m| m.len()).unwrap_or(0);
        self.apply_bandwidth_throttle(downloaded_len).await;
        fs::rename(&temp_path, dest).map_err(|e| S3Error::Io(e.to_string()))?;
        Ok(expected_etag)
    }

    /// Gera uma URL pré-assinada (Presigned URL) temporária para compartilhamento/download direto do objeto S3
    pub async fn generate_presigned_url(
        &self,
        key: &str,
        expires_in_secs: u64,
    ) -> Result<String, S3Error> {
        let clamped_secs = expires_in_secs.clamp(60, 604_800); // entre 1 min e 7 dias
        match &self.backend {
            ClientBackend::Aws(client) => {
                let presigning_config = aws_sdk_s3::presigning::PresigningConfig::expires_in(
                    std::time::Duration::from_secs(clamped_secs),
                )
                .map_err(|e| S3Error::Sdk(e.to_string()))?;

                let presigned_req = client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(key)
                    .presigned(presigning_config)
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;

                Ok(presigned_req.uri().to_string())
            }
            ClientBackend::Mock(_) => Ok(format!(
                "https://{}.s3.amazonaws.com/{}?X-Amz-Expires={}",
                self.bucket, key, clamped_secs
            )),
        }
    }

    /// Lista as versões disponíveis de um objeto específico no bucket S3
    pub async fn list_object_versions(&self, key: &str) -> Result<Vec<S3ObjectVersion>, S3Error> {
        match &self.backend {
            ClientBackend::Aws(client) => {
                let resp = client
                    .list_object_versions()
                    .bucket(&self.bucket)
                    .prefix(key)
                    .send()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?;

                let mut versions = Vec::new();
                for v in resp.versions() {
                    if v.key() != Some(key) {
                        continue;
                    }
                    let version_id = v.version_id().unwrap_or("null").to_string();
                    let is_latest = v.is_latest().unwrap_or(false);
                    let size_bytes = v.size().unwrap_or(0).max(0) as u64;
                    let etag = v.e_tag().map(normalize_etag).unwrap_or_default();
                    let last_modified = v
                        .last_modified()
                        .and_then(|dt| {
                            DateTime::<Utc>::from_timestamp(dt.secs(), dt.subsec_nanos())
                        })
                        .unwrap_or_else(Utc::now)
                        .format("%Y-%m-%d %H:%M:%S UTC")
                        .to_string();

                    versions.push(S3ObjectVersion {
                        version_id,
                        is_latest,
                        last_modified,
                        size_bytes,
                        etag,
                    });
                }
                Ok(versions)
            }
            ClientBackend::Mock(mock) => {
                let guard = mock.lock().unwrap();
                if let Some(obj) = guard.objects.get(key) {
                    Ok(vec![S3ObjectVersion {
                        version_id: "v1-latest".to_string(),
                        is_latest: true,
                        last_modified: obj
                            .last_modified
                            .format("%Y-%m-%d %H:%M:%S UTC")
                            .to_string(),
                        size_bytes: obj.bytes.len() as u64,
                        etag: normalize_etag(&obj.etag),
                    }])
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    /// Baixa uma versão específica (`version_id`) de um objeto do S3 para o destino local
    pub async fn download_object_version(
        &self,
        key: &str,
        version_id: &str,
        dest: &Path,
    ) -> Result<String, S3Error> {
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                ensure_dir_exists(parent)?;
            }
        }

        match &self.backend {
            ClientBackend::Aws(client) => {
                let mut req = client.get_object().bucket(&self.bucket).key(key);
                if !version_id.is_empty() && version_id != "null" {
                    req = req.version_id(version_id);
                }
                let resp = req.send().await.map_err(|e| S3Error::Sdk(e.to_string()))?;

                let etag = resp.e_tag().map(normalize_etag).unwrap_or_default();
                let bytes = resp
                    .body
                    .collect()
                    .await
                    .map_err(|e| S3Error::Sdk(e.to_string()))?
                    .into_bytes();

                let temp_path = dest.with_extension("tmp");
                let mut tmp_file =
                    File::create(&temp_path).map_err(|e| S3Error::Io(e.to_string()))?;
                tmp_file
                    .write_all(&bytes)
                    .map_err(|e| S3Error::Io(e.to_string()))?;
                tmp_file.flush().map_err(|e| S3Error::Io(e.to_string()))?;

                if dest.exists() && dest.is_file() {
                    let _ = fs::remove_file(dest);
                }
                fs::rename(&temp_path, dest).map_err(|e| S3Error::Io(e.to_string()))?;
                Ok(etag)
            }
            ClientBackend::Mock(_) => self.download_file(key, dest).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Feature: amazon-s3-drive, Property 4: Checksum validation correctness
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn checksum_valid_for_own_md5(bytes in prop::collection::vec(any::<u8>(), 1..=4096)) {
            let etag = compute_md5_bytes(&bytes);
            prop_assert!(validate_checksum_bytes(&bytes, &etag));
        }

        #[test]
        fn checksum_invalid_for_other(
            bytes in prop::collection::vec(any::<u8>(), 1..=4096),
            wrong_etag in "[a-f0-9]{32}"
        ) {
            let real_etag = compute_md5_bytes(&bytes);
            prop_assume!(wrong_etag != real_etag);
            prop_assert!(!validate_checksum_bytes(&bytes, &wrong_etag));
        }
    }

    // Feature: amazon-s3-drive, Property 5: Multipart part sizes
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn multipart_parts_valid(
            file_size in (MULTIPART_THRESHOLD + 1)..=(10 * 1024 * 1024 * 1024u64),
            part_size in MIN_PART_SIZE..=(100 * 1024 * 1024u64),
        ) {
            let parts = S3Client::compute_parts(file_size, part_size);
            prop_assert!(!parts.is_empty());
            for part in &parts[..parts.len() - 1] {
                prop_assert!(part.size >= MIN_PART_SIZE);
            }
            let total: u64 = parts.iter().map(|p| p.size).sum();
            prop_assert_eq!(total, file_size);
        }
    }

    #[test]
    fn test_requires_multipart_boundary() {
        assert!(!S3Client::requires_multipart(100 * 1024 * 1024));
        assert!(S3Client::requires_multipart(100 * 1024 * 1024 + 1));
    }

    #[test]
    fn test_compute_parts_150mb_with_8mb_parts() {
        let file_size = 150 * 1024 * 1024;
        let part_size = 8 * 1024 * 1024;
        let parts = S3Client::compute_parts(file_size, part_size);

        // 150 / 8 = 18 partes de 8 MB (144 MB) + 1 parte de 6 MB = 19 partes
        assert_eq!(parts.len(), 19);
        for p in &parts[..18] {
            assert_eq!(p.size, 8 * 1024 * 1024);
        }
        assert_eq!(parts[18].size, 6 * 1024 * 1024);
        assert_eq!(parts.iter().map(|p| p.size).sum::<u64>(), file_size);
    }

    #[test]
    fn test_compute_parts_exact_multiple() {
        let file_size = 160 * 1024 * 1024;
        let part_size = 8 * 1024 * 1024;
        let parts = S3Client::compute_parts(file_size, part_size);

        assert_eq!(parts.len(), 20);
        for p in &parts {
            assert_eq!(p.size, part_size);
        }
        assert_eq!(parts.iter().map(|p| p.size).sum::<u64>(), file_size);
    }
}
