use crate::db::Database;
use crate::models::{QueueError, QueueItem, QueueStats, SyncOperation, SyncOperationType};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::{Arc, Mutex};

pub const QUEUE_MAX_SIZE: usize = 10_000;

#[derive(Clone)]
pub struct QueueManager {
    conn: Arc<Mutex<Connection>>,
}

impl QueueManager {
    pub fn new(db: &Database) -> Self {
        Self {
            conn: db.connection(),
        }
    }

    pub fn new_in_memory() -> Result<Self, QueueError> {
        let db = Database::new_in_memory().map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(Self::new(&db))
    }

    /// Enfileira operação. Retorna None se a fila estiver cheia (>= 10.000 pendentes).
    pub fn enqueue(&self, op: SyncOperation) -> Result<Option<i64>, QueueError> {
        self.enqueue_with_time(op, Utc::now())
    }

    pub fn enqueue_with_time(
        &self,
        op: SyncOperation,
        created_at: DateTime<Utc>,
    ) -> Result<Option<i64>, QueueError> {
        let conn = self.conn.lock().unwrap();
        let pending: usize =
            conn.query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE state = 'Pending'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| QueueError::Database(e.to_string()))? as usize;

        if pending >= QUEUE_MAX_SIZE {
            return Ok(None);
        }

        let mut stmt = conn
            .prepare_cached(
                r#"
                INSERT INTO sync_queue (operation_type, s3_key, local_path, extra, created_at, retry_count, state)
                VALUES (?1, ?2, ?3, ?4, ?5, 0, 'Pending')
                "#,
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;

        stmt.execute(params![
            op.operation_type.as_str(),
            op.s3_key,
            op.local_path,
            op.extra,
            created_at.to_rfc3339()
        ])
        .map_err(|e| QueueError::Database(e.to_string()))?;

        Ok(Some(conn.last_insert_rowid()))
    }

    /// Retorna o próximo item pendente em ordem cronológica (FIFO) e marca como 'Processing'.
    pub fn dequeue_next(&self) -> Result<Option<QueueItem>, QueueError> {
        let conn = self.conn.lock().unwrap();
        let item_opt = conn
            .query_row(
                r#"
                SELECT id, operation_type, s3_key, local_path, extra, created_at, retry_count, error_message
                FROM sync_queue
                WHERE state = 'Pending'
                ORDER BY created_at ASC, id ASC
                LIMIT 1
                "#,
                [],
                |row| Self::row_to_item(row),
            )
            .optional()
            .map_err(|e| QueueError::Database(e.to_string()))?;

        if let Some(ref item) = item_opt {
            conn.execute(
                "UPDATE sync_queue SET state = 'Processing' WHERE id = ?1",
                params![item.id],
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;
        }

        Ok(item_opt)
    }

    pub fn mark_completed(&self, id: i64) -> Result<(), QueueError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET state = 'Completed' WHERE id = ?1",
            params![id],
        )
        .map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn mark_failed(&self, id: i64, error: &str) -> Result<(), QueueError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_queue SET state = 'Failed', retry_count = retry_count + 1, error_message = ?1 WHERE id = ?2",
            params![error, id],
        )
        .map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn active_delete_keys(&self) -> Result<std::collections::HashSet<String>, QueueError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare_cached("SELECT s3_key FROM sync_queue WHERE operation_type = 'Delete' AND state IN ('Pending', 'Processing')")
            .map_err(|err| QueueError::Database(err.to_string()))?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|err| QueueError::Database(err.to_string()))?;
        rows.collect::<Result<_, _>>()
            .map_err(|err| QueueError::Database(err.to_string()))
    }

    pub fn get_item(&self, id: i64) -> Result<Option<QueueItem>, QueueError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            r#"
            SELECT id, operation_type, s3_key, local_path, extra, created_at, retry_count, error_message
            FROM sync_queue
            WHERE id = ?1
            "#,
            params![id],
            |row| Self::row_to_item(row),
        )
        .optional()
        .map_err(|e| QueueError::Database(e.to_string()))
    }

    pub fn count_pending(&self) -> Result<usize, QueueError> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sync_queue WHERE state = 'Pending'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(count.max(0) as usize)
    }

    pub fn is_full(&self) -> Result<bool, QueueError> {
        Ok(self.count_pending()? >= QUEUE_MAX_SIZE)
    }

    pub fn get_stats(&self) -> Result<QueueStats, QueueError> {
        let pending_count = self.count_pending()?;
        Ok(QueueStats {
            pending_count,
            is_full: pending_count >= QUEUE_MAX_SIZE,
        })
    }

    pub fn get_all_pending_ordered(&self) -> Result<Vec<QueueItem>, QueueError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, operation_type, s3_key, local_path, extra, created_at, retry_count, error_message
                FROM sync_queue
                WHERE state = 'Pending'
                ORDER BY created_at ASC, id ASC
                "#,
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| Self::row_to_item(row))
            .map_err(|e| QueueError::Database(e.to_string()))?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| QueueError::Database(e.to_string()))?);
        }
        Ok(items)
    }

    /// Recoloca na fila as operações que ficaram em estado 'Processing' em uma sessão anterior
    /// (crash ou fechamento forçado do aplicativo). Deve ser chamado na inicialização antes do
    /// primeiro `process_queue`, de forma que nenhuma operação iniciada fique presa para sempre.
    pub fn recover_interrupted_items(&self) -> Result<usize, QueueError> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE sync_queue SET state = 'Pending' WHERE state = 'Processing'",
                [],
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(affected as usize)
    }

    /// Recoloca na fila as operações em estado 'Failed' para nova tentativa.
    /// Chamado quando a conectividade de rede é restaurada, para que falhas transitórias
    /// (timeout, erro de conexão) sejam automaticamente re-tentadas sem intervenção do usuário.
    pub fn requeue_failed_items(&self) -> Result<usize, QueueError> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE sync_queue SET state = 'Pending', error_message = NULL \
                 WHERE state = 'Failed'",
                [],
            )
            .map_err(|e| QueueError::Database(e.to_string()))?;
        Ok(affected as usize)
    }

    fn row_to_item(row: &rusqlite::Row) -> Result<QueueItem, rusqlite::Error> {
        let id: i64 = row.get(0)?;
        let op_str: String = row.get(1)?;
        let s3_key: String = row.get(2)?;
        let local_path: String = row.get(3)?;
        let extra: Option<String> = row.get(4)?;
        let created_at_str: String = row.get(5)?;
        let retry_count: u32 = row.get(6)?;
        let error_message: Option<String> = row.get(7)?;

        let operation_type =
            SyncOperationType::from_str(&op_str).unwrap_or(SyncOperationType::Upload);
        let created_at = DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(QueueItem {
            id,
            operation: SyncOperation {
                operation_type,
                s3_key,
                local_path,
                extra,
            },
            created_at,
            retry_count,
            error_message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use proptest::prelude::*;

    fn arb_sync_operation() -> impl Strategy<Value = SyncOperation> {
        (
            prop_oneof![
                Just(SyncOperationType::Upload),
                Just(SyncOperationType::Download),
                Just(SyncOperationType::Delete),
                Just(SyncOperationType::Rename),
                Just(SyncOperationType::Move),
            ],
            "[a-zA-Z0-9/_.-]{1,30}",
            "[a-zA-Z0-9/_.-]{1,30}",
            prop::option::of("[a-zA-Z0-9/_.-]{1,20}"),
        )
            .prop_map(
                |(operation_type, s3_key, local_path, extra)| SyncOperation {
                    operation_type,
                    s3_key,
                    local_path,
                    extra,
                },
            )
    }

    // Feature: amazon-s3-drive, Property 9: Queue FIFO ordering
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn queue_dequeues_in_fifo_order(ops in prop::collection::vec(arb_sync_operation(), 2..=30)) {
            let queue = QueueManager::new_in_memory().unwrap();
            let base_time = Utc::now();
            for (idx, op) in ops.into_iter().enumerate() {
                let created_at = base_time + Duration::milliseconds((idx as i64) * 10);
                queue.enqueue_with_time(op, created_at).unwrap();
            }

            let mut prev_time = DateTime::<Utc>::MIN_UTC;
            let mut count = 0;
            while let Some(item) = queue.dequeue_next().unwrap() {
                prop_assert!(item.created_at > prev_time);
                prev_time = item.created_at;
                count += 1;
            }
            prop_assert!(count >= 2);
        }
    }

    // Feature: amazon-s3-drive, Property 10: Queue capacity enforcement
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(5))]
        #[test]
        fn queue_rejects_after_max_capacity(extra_count in 1usize..=50usize) {
            let queue = QueueManager::new_in_memory().unwrap();
            let sample_op = SyncOperation {
                operation_type: SyncOperationType::Upload,
                s3_key: "docs/file.txt".to_string(),
                local_path: "/tmp/docs/file.txt".to_string(),
                extra: None,
            };

            let total = QUEUE_MAX_SIZE + extra_count;
            let mut accepted = 0usize;
            let mut rejected = 0usize;

            for _ in 0..total {
                match queue.enqueue(sample_op.clone()).unwrap() {
                    Some(_) => accepted += 1,
                    None => rejected += 1,
                }
            }

            prop_assert_eq!(accepted, QUEUE_MAX_SIZE);
            prop_assert_eq!(rejected, extra_count);
            prop_assert_eq!(queue.count_pending().unwrap(), QUEUE_MAX_SIZE);
            prop_assert!(queue.is_full().unwrap());
        }
    }

    #[test]
    fn test_mark_failed_increments_retry_and_stores_error() {
        let queue = QueueManager::new_in_memory().unwrap();
        let op = SyncOperation {
            operation_type: SyncOperationType::Upload,
            s3_key: "report.pdf".to_string(),
            local_path: "/tmp/report.pdf".to_string(),
            extra: None,
        };
        let id = queue.enqueue(op).unwrap().unwrap();
        let _ = queue.dequeue_next().unwrap().unwrap();

        queue.mark_failed(id, "Connection timeout").unwrap();
        let item = queue.get_item(id).unwrap().unwrap();
        assert_eq!(item.retry_count, 1);
        assert_eq!(item.error_message.as_deref(), Some("Connection timeout"));
    }

    #[test]
    fn test_get_all_pending_ordered_returns_only_pending_chronologically() {
        let queue = QueueManager::new_in_memory().unwrap();
        let t0 = Utc::now();

        let id1 = queue
            .enqueue_with_time(
                SyncOperation {
                    operation_type: SyncOperationType::Upload,
                    s3_key: "second.txt".to_string(),
                    local_path: "/tmp/second.txt".to_string(),
                    extra: None,
                },
                t0 + Duration::seconds(20),
            )
            .unwrap()
            .unwrap();

        let _id2 = queue
            .enqueue_with_time(
                SyncOperation {
                    operation_type: SyncOperationType::Download,
                    s3_key: "first.txt".to_string(),
                    local_path: "/tmp/first.txt".to_string(),
                    extra: None,
                },
                t0 + Duration::seconds(5),
            )
            .unwrap()
            .unwrap();

        let id3 = queue
            .enqueue_with_time(
                SyncOperation {
                    operation_type: SyncOperationType::Delete,
                    s3_key: "completed.txt".to_string(),
                    local_path: "/tmp/completed.txt".to_string(),
                    extra: None,
                },
                t0 + Duration::seconds(1),
            )
            .unwrap()
            .unwrap();

        queue.mark_completed(id3).unwrap();

        let pending = queue.get_all_pending_ordered().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].operation.s3_key, "first.txt");
        assert_eq!(pending[1].id, id1);
        assert_eq!(pending[1].operation.s3_key, "second.txt");
    }
}
