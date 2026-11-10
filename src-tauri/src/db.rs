use crate::models::{CompletedPart, FileEntry, MultipartState, SyncState};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const DDL: &str = r#"
-- Fila de operações pendentes de sincronização
CREATE TABLE IF NOT EXISTS sync_queue (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_type  TEXT NOT NULL CHECK (operation_type IN
                    ('Upload','Download','Delete','Rename','Move')),
    s3_key          TEXT NOT NULL,
    local_path      TEXT NOT NULL,
    extra           TEXT,
    created_at      TEXT NOT NULL,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'Pending'
                    CHECK (state IN ('Pending','Processing','Completed','Failed')),
    error_message   TEXT
);

-- Registro de arquivos sincronizados (fonte da verdade para detecção de conflitos)
CREATE TABLE IF NOT EXISTS file_registry (
    s3_key          TEXT PRIMARY KEY,
    local_path      TEXT NOT NULL UNIQUE,
    sync_state      TEXT NOT NULL DEFAULT 'Synced',
    last_sync_etag  TEXT,
    last_sync_time  TEXT,
    local_mtime     TEXT,
    size_bytes      INTEGER NOT NULL DEFAULT 0
);

-- Estado de retomada de multipart uploads (expiram após 24h)
CREATE TABLE IF NOT EXISTS multipart_state (
    s3_key          TEXT PRIMARY KEY,
    upload_id       TEXT NOT NULL,
    local_path      TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    parts_json      TEXT NOT NULL
);

-- Metadados internos de sincronização (fingerprint de configuração, etc.)
CREATE TABLE IF NOT EXISTS sync_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Índices para performance
CREATE INDEX IF NOT EXISTS idx_queue_state_created
    ON sync_queue (state, created_at) WHERE state = 'Pending';
CREATE INDEX IF NOT EXISTS idx_registry_local
    ON file_registry (local_path);
"#;

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(DDL)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn new_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(DDL)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    // === Operações em file_registry ===

    pub fn upsert_file_entry(&self, entry: &FileEntry) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let sync_state_str = serde_json::to_string(&entry.sync_state).unwrap_or_else(|_| "\"Synced\"".to_string());
        let last_sync_time = entry.last_sync_time.map(|dt| dt.to_rfc3339());
        let local_mtime = entry.local_modified_time.map(|dt| dt.to_rfc3339());

        conn.execute(
            r#"
            INSERT INTO file_registry (s3_key, local_path, sync_state, last_sync_etag, last_sync_time, local_mtime, size_bytes)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(s3_key) DO UPDATE SET
                local_path = excluded.local_path,
                sync_state = excluded.sync_state,
                last_sync_etag = excluded.last_sync_etag,
                last_sync_time = excluded.last_sync_time,
                local_mtime = excluded.local_mtime,
                size_bytes = excluded.size_bytes
            "#,
            params![
                entry.s3_key,
                entry.local_path,
                sync_state_str,
                entry.last_sync_etag,
                last_sync_time,
                local_mtime,
                entry.size_bytes as i64
            ],
        )?;
        Ok(())
    }

    pub fn get_file_entry_by_key(&self, s3_key: &str) -> Result<Option<FileEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT s3_key, local_path, sync_state, last_sync_etag, last_sync_time, local_mtime, size_bytes FROM file_registry WHERE s3_key = ?1",
            params![s3_key],
            |row| Self::row_to_file_entry(row),
        )
        .optional()
    }

    pub fn get_file_entry_by_path(&self, local_path: &str) -> Result<Option<FileEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT s3_key, local_path, sync_state, last_sync_etag, last_sync_time, local_mtime, size_bytes FROM file_registry WHERE local_path = ?1",
            params![local_path],
            |row| Self::row_to_file_entry(row),
        )
        .optional()
    }

    pub fn list_file_entries(&self) -> Result<Vec<FileEntry>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT s3_key, local_path, sync_state, last_sync_etag, last_sync_time, local_mtime, size_bytes FROM file_registry",
        )?;
        let rows = stmt.query_map([], |row| Self::row_to_file_entry(row))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn delete_file_entry(&self, s3_key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM file_registry WHERE s3_key = ?1", params![s3_key])?;
        Ok(())
    }

    fn row_to_file_entry(row: &rusqlite::Row) -> Result<FileEntry, rusqlite::Error> {
        let s3_key: String = row.get(0)?;
        let local_path: String = row.get(1)?;
        let sync_state_raw: String = row.get(2)?;
        let last_sync_etag: Option<String> = row.get(3)?;
        let last_sync_time_raw: Option<String> = row.get(4)?;
        let local_mtime_raw: Option<String> = row.get(5)?;
        let size_bytes: i64 = row.get(6)?;

        let sync_state: SyncState = serde_json::from_str(&sync_state_raw).unwrap_or(SyncState::Synced);
        let last_sync_time = last_sync_time_raw
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let local_modified_time = local_mtime_raw
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(FileEntry {
            s3_key,
            local_path,
            sync_state,
            last_sync_etag,
            last_sync_time,
            local_modified_time,
            size_bytes: size_bytes.max(0) as u64,
        })
    }

    // === Operações em multipart_state ===

    pub fn save_multipart_state(&self, state: &MultipartState) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let parts_json = serde_json::to_string(&state.completed_parts).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            r#"
            INSERT INTO multipart_state (s3_key, upload_id, local_path, created_at, parts_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(s3_key) DO UPDATE SET
                upload_id = excluded.upload_id,
                local_path = excluded.local_path,
                created_at = excluded.created_at,
                parts_json = excluded.parts_json
            "#,
            params![
                state.s3_key,
                state.upload_id,
                state.local_path,
                state.created_at.to_rfc3339(),
                parts_json
            ],
        )?;
        Ok(())
    }

    pub fn get_multipart_state(&self, s3_key: &str) -> Result<Option<MultipartState>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT s3_key, upload_id, local_path, created_at, parts_json FROM multipart_state WHERE s3_key = ?1",
            params![s3_key],
            |row| {
                let s3_key: String = row.get(0)?;
                let upload_id: String = row.get(1)?;
                let local_path: String = row.get(2)?;
                let created_at_str: String = row.get(3)?;
                let parts_json: String = row.get(4)?;

                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let completed_parts: Vec<CompletedPart> =
                    serde_json::from_str(&parts_json).unwrap_or_default();

                Ok(MultipartState {
                    s3_key,
                    upload_id,
                    local_path,
                    created_at,
                    completed_parts,
                })
            },
        )
        .optional()
    }

    pub fn delete_multipart_state(&self, s3_key: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM multipart_state WHERE s3_key = ?1", params![s3_key])?;
        Ok(())
    }

    // === Metadados de sincronização ===

    pub fn get_config_fingerprint(&self) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM sync_meta WHERE key = 'config_fingerprint'",
            [],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn set_config_fingerprint(&self, fp: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_meta (key, value) VALUES ('config_fingerprint', ?1) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![fp],
        )?;
        Ok(())
    }

    /// Descarta todos os dados de sincronização vinculados à configuração anterior:
    /// fila de operações pendentes, registro de arquivos e estado de multipart uploads.
    /// Chamado quando a identidade de sincronização muda (bucket, prefixo, pasta local ou endpoint).
    pub fn clear_all_sync_data(&self) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM sync_queue;\
             DELETE FROM file_registry;\
             DELETE FROM multipart_state;",
        )?;
        Ok(())
    }
}
