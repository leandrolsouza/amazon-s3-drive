# Implementation Plan: Amazon S3 Drive

## Overview

Implementação do Amazon S3 Drive — aplicação desktop cross-platform com Tauri 2.x (Rust + React/TypeScript) que sincroniza bidirecionalmente uma pasta local com um bucket Amazon S3. O backend Rust cuida de toda a lógica de sincronização; o frontend React/TypeScript fornece configuração, system tray e notificações. A comunicação entre camadas ocorre exclusivamente via Tauri Commands e Events.

## Tasks

- [x] 1. Setup do projeto Tauri 2.x
  - [x] 1.1 Inicializar projeto Tauri 2.x com template React/TypeScript
    - Executar `npm create tauri-app@latest amazon-s3-drive -- --template react-ts`
    - Configurar `tauri.conf.json` com `identifier = "com.amazon-s3-drive.app"`
    - Habilitar capabilities em `src-tauri/capabilities/default.json`: `core:notification`, `core:tray`, `core:path:default`, `core:fs:default`
    - _Requirements: 1.1_

  - [x] 1.2 Configurar dependências Rust no `src-tauri/Cargo.toml`
    - Adicionar crates: `aws-sdk-s3 = "1"`, `tokio = { version = "1", features = ["full"] }`, `notify = "6"`, `rusqlite = { version = "0.31", features = ["bundled"] }`, `keyring = "2"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `thiserror = "1"`, `chrono = { version = "0.4", features = ["serde"] }`, `glob = "0.3"`, `dirs = "5"`, `md-5 = "0.10"`, `tokio-util = "0.7"`
    - Adicionar dev-dependency: `proptest = "1"`
    - _Requirements: 2.1, 2.6, 3.1, 4.3, 7.1_

  - [x] 1.3 Configurar dependências TypeScript e ambiente de testes no frontend
    - Instalar: `zustand@^4`, `fast-check@^3`
    - Instalar dev: `vitest@^1`, `@testing-library/react@^14`, `@testing-library/user-event@^14`, `@vitest/coverage-v8`
    - Criar `vitest.config.ts` com `environment: 'jsdom'`
    - _Requirements: 7.2_

- [x] 2. Definir modelos de dados Rust e schema SQLite
  - [x] 2.1 Criar `src-tauri/src/models.rs` com todos os tipos de domínio
    - Structs: `AppConfig`, `NotificationSettings`, `AwsCredentials`, `FileEntry`, `S3ObjectInfo`, `SyncOperation`, `QueueItem`, `ConflictInfo`, `MultipartState`, `CompletedPart`, `PartRange`
    - Enums com `#[serde(tag)]`: `SyncOperationType`, `SyncState`, `GlobalSyncState`
    - Enum de erros com `thiserror::Error`: `SyncError`, `ConfigError`, `CredError`, `QueueError`, `WatchError`, `ConflictError`
    - Tipos auxiliares de resposta: `ValidationError { field: String, reason: String }`, `FileStatus`, `QueueStats`
    - Derivar `Serialize`, `Deserialize`, `Clone`, `Debug`, `PartialEq` onde necessário
    - _Requirements: 5.5, 6.1, 7.1, 8.3_

  - [x] 2.2 Criar `src-tauri/src/db.rs` com inicialização do SQLite e DDL completo
    - Implementar `Database::new(path: &Path) -> Result<Database, rusqlite::Error>` que executa o DDL
    - Tabelas: `sync_queue`, `file_registry`, `multipart_state` com índices conforme design
    - Implementar `Database::new_in_memory() -> Result<Database>` para uso em testes
    - _Requirements: 2.7, 3.5, 8.2_

- [x] 3. Implementar ConfigParser
  - [x] 3.1 Criar `src-tauri/src/config_parser.rs` com `ConfigParser` e todos os métodos
    - `load() -> Result<AppConfig, ConfigError>`: ler arquivo com `fs::read_to_string`; retornar `ConfigError` descritivo para JSON malformado ou bytes não-UTF-8
    - `save(config: &AppConfig) -> Result<(), ConfigError>`: serializar com `serde_json::to_string_pretty`; criar diretório pai via `fs::create_dir_all` se necessário
    - `create_empty() -> Result<(), ConfigError>`: criar arquivo com campos obrigatórios em branco/padrão
    - `validate(config: &AppConfig) -> Result<(), Vec<ValidationError>>`: checar presença de `bucket_name`, `local_path`, `aws_region`
    - Localizar via `dirs::config_dir()` em `amazon-s3-drive/config.json`
    - _Requirements: 5.5, 5.6, 5.7, 5.8, 5.9_

  - [x]* 3.2 Escrever property test para Property 1 — config serialization round-trip
    - **Property 1: Config serialization round-trip**
    - **Validates: Requirements 5.5, 5.6**
    - Criar strategy `arb_app_config()` cobrindo todos os campos incluindo `exclusion_patterns` variados
    - Verificar que `serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap() == config` para qualquer `AppConfig` válido

  - [x]* 3.3 Escrever unit tests para ConfigParser
    - Testar `create_empty()` gera arquivo com todos os campos obrigatórios presentes
    - Testar `load()` retorna `ConfigError` descritivo para JSON malformado (`{invalid}`)
    - Testar `load()` retorna `ConfigError` para arquivo com bytes não-UTF-8
    - _Requirements: 5.8, 5.9_

- [x] 4. Implementar CredentialsManager
  - [x] 4.1 Criar `src-tauri/src/credentials_manager.rs` com `CredentialsManager`
    - `store(creds: &AwsCredentials)`: serializar como JSON e salvar via `keyring` com service `"amazon-s3-drive"`, entry `"aws-credentials"`
    - `load() -> Result<Option<AwsCredentials>>`: recuperar e desserializar do keyring
    - `delete()`: remover entrada do keyring
    - `validate_format(creds: &AwsCredentials) -> Result<(), Vec<ValidationError>>`: validar `access_key_id` regex `[A-Z0-9]{20}`, `secret_access_key` len == 40, `region` regex `^[a-z]{2}-[a-z]+-\d$`
    - `verify_access(s3_client: &S3Client) -> Result<(), CredError>`: delegar para `s3_client.test_access()`
    - _Requirements: 4.1, 4.2, 4.3, 4.6, 4.7_

  - [x]* 4.2 Escrever property test para Property 3 — credential format validation
    - **Property 3: Credential format validation**
    - **Validates: Requirements 4.1, 4.2**
    - Criar `arb_valid_credentials()`: `access_key_id` exatamente 20 chars `[A-Z0-9]`, `secret_access_key` 40 chars quaisquer, `region` no formato `xx-xxxx-N`
    - Criar `arb_invalid_credentials()`: ao menos um campo violando a regra
    - Verificar `Ok(())` para todos os válidos; `Err` identificando o campo correto para todos os inválidos

  - [x]* 4.3 Escrever unit tests para CredentialsManager
    - Testar `validate_format` para `access_key_id` com 19 e 21 chars (deve rejeitar)
    - Testar `validate_format` para `secret_access_key` com 39 e 41 chars (deve rejeitar)
    - Testar região `"us-east-1"` aceita, `"US-EAST-1"` e `"us-east"` rejeitadas
    - Verificar que a mensagem de erro identifica o campo com falha
    - _Requirements: 4.1, 4.2_

- [x] 5. Implementar S3Client
  - [x] 5.1 Criar `src-tauri/src/s3_client.rs` com operações S3 fundamentais
    - Construir `aws_sdk_s3::Client` a partir de `AwsCredentials` (ou IAM role via `aws_config::from_env`)
    - `list_objects(prefix) -> Result<Vec<S3ObjectInfo>>`: `ListObjectsV2`, normalizar ETags removendo aspas duplas
    - `delete_object(key)`, `copy_object(src, dst)`, `head_object(key) -> S3ObjectInfo`
    - `test_access() -> Result<()>`: `ListObjectsV2` com `max_keys=1`
    - _Requirements: 4.6, 4.7, 2.3, 2.4, 2.5, 3.3_

  - [x] 5.2 Implementar `upload_file` e helper `requires_multipart`
    - `upload_file(key, path) -> Result<String>`: `PutObject` com body lido do arquivo; retornar ETag normalizado
    - `requires_multipart(file_size: u64) -> bool`: `file_size > 100 * 1024 * 1024`
    - _Requirements: 2.1, 2.2_

  - [x] 5.3 Implementar `compute_parts` e `upload_multipart` com retomada de estado
    - `compute_parts(file_size: u64, part_size: u64) -> Vec<PartRange>`: todas as partes (exceto a última) com `size >= 5 MB`; a última parte recebe o restante
    - `upload_multipart(key, path, resume) -> Result<String>`: `CreateMultipartUpload` → loop `UploadPart` por `PartRange` → `CompleteMultipartUpload`; persistir `MultipartState` no SQLite após cada parte; retomar de `resume.completed_parts` se fornecido; expirar estado após 24h
    - _Requirements: 2.6, 2.7_

  - [x] 5.4 Implementar `download_file` com validação de checksum MD5
    - `download_file(key, dest) -> Result<String>`: `GetObject` → escrever em `dest.with_extension("tmp")`
    - Calcular MD5 do arquivo temporário; comparar com ETag S3 (aspas removidas)
    - Se válido: mover `temp → dest`; se inválido: descartar `temp`, retornar `SyncError::ChecksumMismatch`
    - Implementar helpers `compute_md5(path: &Path) -> String` e `validate_checksum(path: &Path, etag: &str) -> bool`
    - _Requirements: 3.6, 3.7_

  - [x]* 5.5 Escrever property test para Property 4 — checksum validation correctness
    - **Property 4: Checksum validation correctness**
    - **Validates: Requirements 3.6**
    - Para qualquer `Vec<u8>`, `validate_checksum_bytes(&bytes, &compute_md5_bytes(&bytes))` retorna `true`
    - Para qualquer par onde `wrong_etag != compute_md5_bytes(&bytes)`, retorna `false`

  - [x]* 5.6 Escrever property test para Property 5 — multipart part sizes
    - **Property 5: Multipart part sizes**
    - **Validates: Requirements 2.6**
    - Para qualquer `file_size > 100 MB` e `part_size >= 5 MB`, verificar: todas as partes exceto a última têm `size >= 5 MB` e `sum(parts[*].size) == file_size`

  - [x]* 5.7 Escrever unit tests para S3Client
    - Testar `requires_multipart`: exatamente `100 MB` retorna `false`, `100 MB + 1 byte` retorna `true`
    - Testar `compute_parts` para arquivo de 150 MB com `part_size = 8 MB`: verificar contagem e tamanho da última parte
    - Testar `compute_parts` para arquivo divisível exatamente pelo `part_size`
    - _Requirements: 2.6_

- [x] 6. Implementar FileWatcher
  - [x] 6.1 Criar `src-tauri/src/file_watcher.rs` com `FileWatcher` usando crate `notify`
    - Usar `notify::RecommendedWatcher` (inotify / FSEvents / ReadDirectoryChanges)
    - Mapear eventos raw para `LocalFsEvent`: `Create → Created`, `Modify → Modified`, `Remove → Deleted`, eventos de rename para `Renamed { from, to }`
    - `start()`: iniciar watcher no path configurado, enviar eventos por `mpsc::Sender<LocalFsEvent>`
    - `stop()`: fazer drop do watcher; `update_path(new_path)`: reiniciar no novo diretório
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5_

  - [x] 6.2 Implementar debounce de 2 segundos para eventos `Modified`
    - Manter `HashMap<PathBuf, tokio::time::Instant>` da última modificação por arquivo
    - Colapsar múltiplos `Modified` para o mesmo arquivo dentro de 2 segundos em um único evento usando `tokio::time::sleep`
    - _Requirements: 2.2_

  - [x]* 6.3 Escrever unit tests para FileWatcher
    - Testar debounce: 3 modificações rápidas no mesmo arquivo emitem apenas 1 `Modified` após ~2s
    - Testar que `Renamed { from, to }` é emitido corretamente ao renomear um arquivo
    - _Requirements: 2.2, 2.3_

- [x] 7. Implementar QueueManager
  - [x] 7.1 Criar `src-tauri/src/queue_manager.rs` com `QueueManager` sobre SQLite
    - `enqueue(op: SyncOperation) -> Result<Option<i64>>`: inserir em `sync_queue`; retornar `None` se `count_pending() >= QUEUE_MAX_SIZE (10_000)`
    - `dequeue_next() -> Result<Option<QueueItem>>`: selecionar item `Pending` mais antigo por `created_at`, marcar `state = 'Processing'` atomicamente
    - `mark_completed(id)` e `mark_failed(id, error)`: atualizar estado e incrementar `retry_count`
    - `count_pending()`, `is_full()`, `get_all_pending_ordered()`
    - `new_in_memory() -> Result<Self>`: construtor de teste via `Connection::open_in_memory()`
    - _Requirements: 8.2, 8.3, 8.5, 8.6_

  - [x]* 7.2 Escrever property test para Property 9 — Queue FIFO ordering
    - **Property 9: Queue FIFO ordering**
    - **Validates: Requirements 8.3**
    - Inserir sequência de operações com timestamps distintos via `enqueue_with_time(op, created_at)`
    - Verificar que chamadas repetidas a `dequeue_next` retornam `created_at` em ordem estritamente crescente

  - [x]* 7.3 Escrever property test para Property 10 — Queue capacity enforcement
    - **Property 10: Queue capacity enforcement**
    - **Validates: Requirements 8.2, 8.5**
    - Inserir `QUEUE_MAX_SIZE + N` operações (N entre 1 e 100)
    - Verificar: exatamente `QUEUE_MAX_SIZE` retornam `Some`; restantes retornam `None`; `count_pending()` permanece em `QUEUE_MAX_SIZE`

  - [x]* 7.4 Escrever unit tests para QueueManager
    - Testar `mark_failed` preserva o item com `retry_count` incrementado e `error_message` preenchido
    - Testar `get_all_pending_ordered` retorna apenas itens `Pending` em ordem cronológica
    - _Requirements: 8.6_

- [x] 8. Implementar ConflictResolver
  - [x] 8.1 Implementar `detect_conflict` e `conflict_path` em `src-tauri/src/conflict_resolver.rs`
    - `detect_conflict(file: &FileEntry, s3: &S3ObjectInfo) -> bool`: retornar `true` sse `file.local_modified_time != file.last_sync_time AND Some(s3.etag.clone()) != file.last_sync_etag`
    - `conflict_path(original: &Path, timestamp: &DateTime<Utc>) -> PathBuf`: extrair `stem` e `extension`; gerar `{stem}(conflito-{%Y-%m-%d-%H-%M-%S}).{ext}` no mesmo diretório
    - _Requirements: 6.1, 6.2, 6.4_

  - [x] 8.2 Implementar `resolve()` com criação de cópia de conflito e construção do `ConflictInfo`
    - Baixar versão remota para `conflict_path` via `s3_client.download_file`
    - Construir `ConflictInfo` preenchendo todos os campos: `file_name`, `local_path`, `conflict_copy_path`, `local_modified`, `remote_modified`, `local_size_bytes`, `remote_size_bytes`
    - Se a cópia não puder ser salva (IO error): preservar versão local, retornar `Err(ConflictError::CannotSaveConflictCopy)`
    - _Requirements: 6.2, 6.3, 6.5_

  - [x]* 8.3 Escrever property test para Property 6 — conflict detection condition
    - **Property 6: Conflict detection condition**
    - **Validates: Requirements 6.1, 6.4**
    - Para qualquer `(FileEntry, S3ObjectInfo)`, verificar que `detect_conflict` retorna `true` sse ambas as condições são satisfeitas (`local_modified_time != last_sync_time AND s3.etag != last_sync_etag`)

  - [x]* 8.4 Escrever property test para Property 7 — conflict resolution output correctness
    - **Property 7: Conflict resolution output correctness**
    - **Validates: Requirements 6.2, 6.3**
    - Para qualquer `(Path, DateTime<Utc>)`, verificar que `conflict_path` gera nome correspondendo ao padrão `{stem}(conflito-AAAA-MM-DD-HH-MM-SS).{ext}`
    - Verificar que todos os campos obrigatórios do `ConflictInfo` são não-padrão e derivados dos inputs

  - [x]* 8.5 Escrever unit tests para ConflictResolver
    - Testar `resolve()` retorna `Err(CannotSaveConflictCopy)` quando diretório de destino está sem permissão
    - Testar `conflict_path` para arquivo sem extensão e com extensão composta (ex: `.tar.gz`)
    - _Requirements: 6.5_

- [x] 9. Checkpoint — Garantir que todos os testes do backend puro passam
  - Executar `cargo test` em `src-tauri/` e corrigir falhas antes de prosseguir
  - Verificar que todos os módulos compilam sem warnings de unused imports ou dead code

- [x] 10. Implementar NetworkDetector
  - [x] 10.1 Criar `src-tauri/src/network_detector.rs` com `NetworkDetector`
    - `start(s3_endpoint: &str)`: iniciar loop Tokio que tenta conexão TCP ao endpoint S3 a cada 10s; atualizar `AtomicBool is_online`
    - `is_online() -> bool`: leitura do `AtomicBool`
    - `on_state_change(callback: impl Fn(bool) + Send + 'static)`: invocar callback ao detectar transição offline ↔ online
    - _Requirements: 8.1, 8.3_

  - [x]* 10.2 Escrever unit tests para NetworkDetector
    - Testar transição de estado online → offline → online invoca callback com `true`/`false` corretos
    - _Requirements: 8.1_

- [x] 11. Implementar StatusMonitor
  - [x] 11.1 Criar `src-tauri/src/status_monitor.rs` com `StatusMonitor`
    - `set_state(key: &str, state: SyncState)`: atualizar mapa `Arc<RwLock<HashMap<String, SyncState>>>`
    - `get_all() -> Vec<FileStatus>`: retornar snapshot atual
    - `global_state() -> GlobalSyncState`: calcular com prioridade `Error > Syncing/Pending > UpToDate`; retornar `Offline` se `NetworkDetector::is_online() == false`
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_

  - [x]* 11.2 Escrever property test para Property 11 — global state priority
    - **Property 11: Global state priority**
    - **Validates: Requirements 7.1, 7.2, 7.3**
    - Para qualquer conjunto não-vazio de `SyncState`, verificar que `compute_global_state(&states)` retorna `Error` se há ao menos um `Error`; `Syncing` se há `Syncing`/`Pending` e nenhum `Error`; `UpToDate` sse todos são `Synced`

  - [x]* 11.3 Escrever unit tests para StatusMonitor
    - Testar cada transição de estado individualmente
    - Testar mix de `Synced + Error` retorna `Error` (prioridade)
    - _Requirements: 7.1, 7.2, 7.3_

- [x] 12. Implementar SyncEngine
  - [x] 12.1 Criar `src-tauri/src/sync_engine.rs` com funções de mapeamento de caminho
    - `s3_key_to_local_path(key: &str, base: &Path) -> PathBuf`: substituir `/` pelo separador do SO
    - `local_path_to_s3_key(path: &Path, base: &Path) -> String`: substituir separador do SO por `/`; normalizar separadores consecutivos
    - _Requirements: 1.4_

  - [x]* 12.2 Escrever property test para Property 2 — S3 key ↔ local path round-trip
    - **Property 2: S3 key ↔ local path round-trip**
    - **Validates: Requirements 1.4**
    - Para qualquer chave S3 válida (componentes separados por `/`), verificar que `local_path_to_s3_key(s3_key_to_local_path(key, base), base) == normalize(key)`

  - [x] 12.3 Implementar struct `SyncEngine` com `start()` e `stop()` com shutdown gracioso
    - Struct com `Arc` para `S3Client`, `QueueManager`, `ConflictResolver`, `StatusMonitor`, `NetworkDetector`, `ConfigParser` e `app_handle: tauri::AppHandle`
    - `start()`: iniciar `FileWatcher`, `NetworkDetector`, loop de polling remoto (tokio task) e consumer da fila (tokio task)
    - `stop()`: parar `FileWatcher`; `tokio::time::timeout(60s, drain_active_transfers())`; persistir estado da fila
    - _Requirements: 1.5_

  - [x] 12.4 Implementar `is_excluded` e `process_local_event`
    - `is_excluded(path: &Path, patterns: &[String]) -> bool`: testar `path.file_name()` contra cada glob pattern via crate `glob`
    - `process_local_event(event: LocalFsEvent)`: checar exclusão → detectar conflito via `ConflictResolver` → enfileirar operação; mapear `Renamed { from, to }` para `Copy(from→to) + Delete(from)`
    - Emitir `sync:file-status-changed(Pending)` para cada arquivo enfileirado
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 9.3, 9.8_

  - [x]* 12.5 Escrever property test para Property 8 — exclusion pattern filtering
    - **Property 8: Exclusion pattern filtering**
    - **Validates: Requirements 9.3, 9.4, 9.8**
    - Para qualquer caminho e lista de glob patterns, verificar que `is_excluded` retorna `true` sse ao menos um padrão corresponde ao `file_name` do caminho

  - [x] 12.6 Implementar `poll_remote_changes` com diff contra `file_registry`
    - `list_objects` → comparar ETags com `file_registry` → enfileirar `Download` para novos/modificados e `Delete` para removidos
    - Respeitar `s3_prefix` configurado; ignorar objetos correspondendo a exclusion patterns
    - Atualizar `file_registry` após cada sync bem-sucedido
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 9.4_

  - [x] 12.7 Implementar `process_queue` com retry policy e integração de conflito offline
    - Consumir fila em ordem FIFO; aplicar `RetryPolicy`: upload com backoff exponencial (~1s, ~2s, ~4s com jitter), download com delay fixo de 10s entre tentativas
    - Após 3 falhas: `mark_failed`, emitir `sync:error`, setar estado `Error` no `StatusMonitor`
    - Após reconexão: processar fila offline em ordem cronológica; aplicar `ConflictResolver` para conflitos pós-reconexão
    - Emitir `sync:file-status-changed(Synced)` ao concluir; `sync:initial-complete` ao terminar a primeira varredura completa
    - _Requirements: 2.7, 2.8, 3.5, 3.8, 8.3, 8.4, 8.6_

  - [x]* 12.8 Escrever unit tests para SyncEngine
    - Testar `process_local_event` com arquivo excluído: não enfileira operação
    - Testar `stop()` aguarda transferências ativas por até 60s antes de finalizar
    - _Requirements: 1.5, 9.3_

- [x] 13. Checkpoint — Backend integrado compila e testes passam
  - Executar `cargo test` e garantir compilação sem erros de integração entre módulos
  - Verificar que `SyncEngine::start()` e `stop()` funcionam em teste básico sem pânico

- [x] 14. Implementar Tauri Commands e Events Bridge
  - [x] 14.1 Configurar estado global Tauri em `src-tauri/src/main.rs`
    - Definir `AppState { sync_engine: Arc<SyncEngine>, creds_manager: Arc<CredentialsManager>, config_parser: Arc<ConfigParser> }`
    - Inicializar todos os módulos e registrar `AppState` via `tauri::Builder::manage()`
    - Declarar todos os `#[tauri::command]` no `.invoke_handler(tauri::generate_handler![...])`
    - _Requirements: 4.1, 5.1_

  - [x] 14.2 Implementar Commands de configuração e credenciais
    - `get_config`, `save_config`, `validate_config`: delegar para `ConfigParser`
    - `save_credentials`, `test_connection`, `delete_credentials`: delegar para `CredentialsManager`
    - Converter erros de domínio para `String` via `.map_err(|e| e.to_string())`
    - _Requirements: 4.1, 4.2, 4.6, 5.1, 5.2, 5.4, 5.7_

  - [x] 14.3 Implementar Commands de controle de sync, status e resolução de conflito
    - `start_sync`, `stop_sync`: delegar para `SyncEngine`
    - `get_sync_status`, `get_global_state`: delegar para `StatusMonitor`
    - `get_queue_stats`: delegar para `QueueManager`
    - `resolve_conflict(strategy: "local"|"remote"|"both", local_path, conflict_path)`: executar ação de limpeza no `SyncEngine` conforme estratégia escolhida pelo usuário
    - _Requirements: 7.1, 7.2, 8.5, 10.3_

  - [x] 14.4 Verificar emissão de todos os Tauri Events definidos no design
    - Confirmar que cada evento é emitido via `app_handle.emit_all(...)` no local correto:
      `sync:file-status-changed`, `sync:error`, `sync:initial-complete`, `conflict:detected`, `network:offline`, `network:online`, `queue:full`, `credentials:expired`, `credentials:invalid`
    - _Requirements: 7.5, 7.6, 10.1, 10.2, 10.3_

- [x] 15. Implementar TypeScript interfaces e Zustand store
  - [x] 15.1 Criar `src/types.ts` com todas as interfaces TypeScript do design
    - `AppConfig`, `NotificationSettings`, `AwsCredentials`, `SyncState`, `GlobalSyncState`
    - `FileStatus`, `ConflictEvent`, `ValidationError`, `QueueStats`, `FileStatusEvent`, `SyncErrorEvent`
    - _Requirements: 5.5, 7.1_

  - [x] 15.2 Implementar Zustand store em `src/store.ts`
    - Estado: `config: AppConfig | null`, `globalState: GlobalSyncState`, `fileStatuses: FileStatus[]`, `queueStats: QueueStats`, `isOnline: boolean`
    - Actions: `setConfig`, `setGlobalState`, `updateFileStatus`, `setQueueStats`, `setOnline`
    - _Requirements: 7.1, 7.2_

  - [x] 15.3 Implementar hook `src/hooks/useSyncEvents.ts` com listeners de Tauri Events
    - Registrar listeners via `listen<T>()` para todos os eventos em `useEffect`; cancelar na teardown
    - Atualizar store em resposta a `sync:file-status-changed`, `sync:error`, `network:offline`, `network:online`, `queue:full`, `credentials:expired`, `credentials:invalid`
    - _Requirements: 7.1, 7.2, 8.1, 8.5_

- [x] 16. Implementar Frontend — Página de Configuração
  - [x] 16.1 Criar `src/pages/ConfigurationPage.tsx` com seção principal de configuração do drive
    - Campos: bucket name, local path (com seletor via `invoke('open_path_dialog')`), AWS region, S3 prefix (opcional), sync interval
    - Botões "Salvar Configuração" e "Iniciar Sync" (desabilitado sem `bucket_name` e `local_path` preenchidos)
    - Integrar `invoke('save_config')` e `invoke('validate_config')` nos handlers
    - _Requirements: 5.1, 5.2, 5.3, 5.4_

  - [x] 16.2 Adicionar seção de credenciais AWS e exclusion patterns ao `ConfigurationPage`
    - Campos de credenciais: Access Key ID, Secret Access Key, Session Token (opcional), toggle IAM Role
    - Exibir erros de validação por campo via `ValidationError[]` retornado pelo Command
    - Lista editável de exclusion patterns com validação de sintaxe glob via `invoke('validate_config')`
    - _Requirements: 4.1, 4.2, 4.5, 4.8, 9.2, 9.6_

  - [x] 16.3 Adicionar seção de preferências de notificação ao `ConfigurationPage`
    - Toggle para `sync_complete_enabled` e `conflict_enabled`
    - Desabilitar (e exibir como sempre ativo) o toggle de erros na UI, conforme Requisito 10.4
    - Salvar via `invoke('save_config')` ao alterar qualquer preferência
    - _Requirements: 10.4_

  - [x]* 16.4 Escrever unit tests para ConfigurationPage
    - Testar botão "Iniciar Sync" está desabilitado quando `bucket_name` ou `local_path` estão vazios
    - Testar campos de credenciais exibem mensagem de erro por campo quando `ValidationError[]` é retornado
    - _Requirements: 4.2, 5.4_

- [x] 17. Implementar Frontend — StatusView e System Tray
  - [x] 17.1 Criar `src/pages/StatusView.tsx` com lista de arquivos e estado global
    - Renderizar `fileStatuses` do store com ícone por estado (`Pending`, `Syncing`, `Synced`, `Error`)
    - Exibir badge de estado global (UpToDate / Syncing / Error / Offline) no topo
    - Atualizar em resposta a eventos do store sem polling ativo
    - _Requirements: 7.1, 7.2_

  - [x] 17.2 Implementar System Tray em `src-tauri/src/tray.rs` usando Tauri Tray API
    - Ícone estático para `UpToDate` (verde) e `Error` (vermelho)
    - Ícone animado para `Syncing`/`Pending` via troca de frames a cada 500ms
    - Menu de contexto: "Abrir S3 Drive", "Configurações", separador, "Sair"
    - Atualizar ícone ao receber `sync:file-status-changed` e `network:offline/online`
    - _Requirements: 7.2, 7.3, 7.4, 7.5_

  - [x]* 17.3 Escrever unit tests para StatusView
    - Testar que `StatusView` renderiza ícone correto para cada `SyncState`
    - Testar que badge de estado global reflete prioridade `Error > Syncing > UpToDate`
    - _Requirements: 7.1, 7.2_

- [x] 18. Implementar Frontend — NotificationHandler
  - [x] 18.1 Criar `src/components/NotificationHandler.tsx` que escuta eventos de notificação
    - `sync:initial-complete`: notificação com contagem de arquivos sincronizados, visível `>= 5s`
    - `sync:error` (após 3 tentativas, `retry_count >= 3`): notificação persistente com nome do arquivo e descrição do erro
    - `conflict:detected`: notificação persistente com nome do arquivo e 3 opções: "Manter local", "Manter remota", "Manter ambas"
    - Respeitar `config.notifications.sync_complete_enabled` e `conflict_enabled`; ignorar configuração para erros (sempre exibir)
    - _Requirements: 10.1, 10.2, 10.3, 10.4_

  - [x] 18.2 Implementar resolução de conflito via UI a partir da notificação
    - "Manter local": `invoke('resolve_conflict', { strategy: 'local', ... })` → apaga cópia de conflito
    - "Manter remota": `invoke('resolve_conflict', { strategy: 'remote', ... })` → apaga versão local, renomeia cópia de conflito para nome original
    - "Manter ambas": fechar notificação sem ação adicional (ambas as versões já estão salvas)
    - _Requirements: 10.3_

  - [x]* 18.3 Escrever property test para Property 1 no frontend — config round-trip via fast-check
    - **Property 1: Config serialization round-trip (TypeScript)**
    - **Validates: Requirements 5.5, 5.6**
    - `fc.asyncProperty(arbAppConfig(), async (config) => { expect(JSON.parse(JSON.stringify(config))).toEqual(config) })` com `numRuns: 100`

  - [x]* 18.4 Escrever property test para Property 3 no frontend — credential format validation via fast-check
    - **Property 3: Credential format validation (TypeScript)**
    - **Validates: Requirements 4.1, 4.2**
    - `fc.property(arbValidCredentials(), (creds) => { expect(validateCredentialsFormat(creds)).toHaveLength(0) })` com `numRuns: 100`

  - [x]* 18.5 Escrever unit tests para NotificationHandler
    - Testar que notificação de conclusão não é exibida quando `sync_complete_enabled = false`
    - Testar que notificação de erro é sempre exibida independente de configuração
    - Testar que notificação de conflito exibe as 3 opções de resolução
    - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [x] 19. Checkpoint Final — Todos os testes backend e frontend passam
  - Executar `cargo test` em `src-tauri/` e garantir que todos os property tests e unit tests passam
  - Executar `npx vitest --run` em `src/` e garantir que todos os testes TypeScript passam

- [x] 20. Testes de integração com AWS mockado
  - [x] 20.1 Implementar suite de integração Rust com mock do `aws-sdk-s3`
    - Upload de arquivo `< 100 MB` end-to-end: `FileWatcher → SyncEngine → S3Client mock`
    - Upload multipart de arquivo `> 100 MB` com verificação de partes
    - Download com checksum válido (sucesso) e checksum inválido (descarte e erro)
    - _Requirements: 2.1, 2.6, 3.6_

  - [x] 20.2 Implementar testes de integração para conflito, fila offline e credenciais
    - Detectar e resolver conflito real (modificação simultânea local + S3 mockado)
    - Processar fila offline após reconexão simulada via `NetworkDetector` mock
    - Verificar acesso ao bucket com credenciais inválidas retorna erro descritivo
    - _Requirements: 6.1, 6.2, 8.3, 4.6, 4.7_

  - [x]* 20.3 Escrever testes E2E via Tauri WebDriver API
    - Fluxo completo: configuração → iniciar sync → verificar StatusView → verificar ícone de tray
    - _Requirements: 1.1, 1.2, 1.3_

- [x] 21. Configuração de packaging e distribuição
  - [x] 21.1 Configurar `tauri.conf.json` para empacotamento final
    - Definir `bundle.identifier`, ícones da aplicação (tray e dock/taskbar em múltiplos tamanhos)
    - Configurar `bundle.targets`: `["msi", "nsis"]` para Windows, `["dmg"]` para macOS, `["appimage", "deb"]` para Linux
    - Revisar capabilities para conter apenas permissões necessárias (princípio do menor privilégio)
    - _Requirements: 1.1_

  - [x] 21.2 Verificar build de produção e localização do arquivo de config por plataforma
    - Executar `npm run tauri build` e verificar bundle gerado sem erros
    - Testar que `config.json` é criado no diretório correto: `%APPDATA%\amazon-s3-drive\` (Windows), `~/Library/Application Support/amazon-s3-drive/` (macOS), `~/.config/amazon-s3-drive/` (Linux)
    - _Requirements: 5.5, 1.1_

## Notes

- Tasks marcadas com `*` são opcionais e podem ser puladas para uma MVP mais rápida
- Cada task referencia requisitos específicos para rastreabilidade completa
- Property tests (proptest + fast-check) validam invariantes universais e complementam os unit tests
- O backend Rust deve compilar e passar nos testes antes de iniciar o desenvolvimento do frontend
- Credenciais AWS **nunca** devem ser serializadas para arquivo em disco — somente via keyring nativo do SO
- A comunicação frontend ↔ backend ocorre exclusivamente via Tauri Commands (`invoke`) e Events (`listen`)
- O `SyncEngine` é o único componente que emite Tauri Events; o frontend apenas escuta

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2", "1.3"] },
    { "id": 1, "tasks": ["2.1", "2.2"] },
    { "id": 2, "tasks": ["3.1", "4.1", "5.1", "6.1", "7.1", "8.1", "10.1", "11.1", "12.1", "15.1"] },
    { "id": 3, "tasks": ["3.2", "3.3", "4.2", "4.3", "5.2", "6.2", "7.2", "7.3", "7.4", "8.3", "10.2", "11.2", "11.3", "12.2", "15.2"] },
    { "id": 4, "tasks": ["5.3", "6.3", "8.2", "12.3", "15.3"] },
    { "id": 5, "tasks": ["5.4", "5.6", "5.7", "8.4", "8.5", "12.4"] },
    { "id": 6, "tasks": ["5.5", "12.5", "12.6"] },
    { "id": 7, "tasks": ["12.7"] },
    { "id": 8, "tasks": ["12.8", "14.1"] },
    { "id": 9, "tasks": ["14.2", "14.3", "14.4"] },
    { "id": 10, "tasks": ["16.1", "17.1", "17.2"] },
    { "id": 11, "tasks": ["16.2", "16.4", "17.3"] },
    { "id": 12, "tasks": ["16.3", "18.1"] },
    { "id": 13, "tasks": ["18.2"] },
    { "id": 14, "tasks": ["18.3", "18.4", "18.5"] },
    { "id": 15, "tasks": ["20.1"] },
    { "id": 16, "tasks": ["20.2"] },
    { "id": 17, "tasks": ["20.3", "21.1"] },
    { "id": 18, "tasks": ["21.2"] }
  ]
}
```
