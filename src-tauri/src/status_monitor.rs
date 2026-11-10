use crate::models::{FileStatus, GlobalSyncState, SyncState};
use crate::network_detector::NetworkDetector;
use chrono::Utc;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub fn compute_global_state(states: &[SyncState]) -> GlobalSyncState {
    if states.iter().any(|s| matches!(s, SyncState::Error(_))) {
        GlobalSyncState::Error
    } else if states
        .iter()
        .any(|s| matches!(s, SyncState::Syncing | SyncState::Pending))
    {
        GlobalSyncState::Syncing
    } else {
        GlobalSyncState::UpToDate
    }
}

#[derive(Clone)]
pub struct StatusMonitor {
    states: Arc<RwLock<HashMap<String, FileStatus>>>,
    network_detector: Option<NetworkDetector>,
    log_dir: Option<PathBuf>,
}

impl Default for StatusMonitor {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl StatusMonitor {
    pub fn new(network_detector: Option<NetworkDetector>, log_dir: Option<PathBuf>) -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
            network_detector,
            log_dir,
        }
    }

    pub fn set_state(&self, key: &str, state: SyncState) {
        self.set_file_status(key, state, 0);
    }

    pub fn set_file_status(&self, key: &str, state: SyncState, size_bytes: u64) {
        let now_str = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

        if let SyncState::Error(ref err_msg) = state {
            self.log_error(key, err_msg);
        }

        let mut map = self.states.write().unwrap();
        let entry = map.entry(key.to_string()).or_insert_with(|| FileStatus {
            path: key.to_string(),
            state: state.clone(),
            last_modified: Some(now_str.clone()),
            size_bytes,
        });
        let changed = entry.state != state || (size_bytes > 0 && entry.size_bytes != size_bytes);
        entry.state = state.clone();
        entry.last_modified = Some(now_str);
        if size_bytes > 0 {
            entry.size_bytes = size_bytes;
        }
        drop(map);

        if changed {
            match state {
                SyncState::Synced => self.write_log("INFO", "SyncEngine", key, "Transferência concluída (Synced)"),
                SyncState::Syncing => self.write_log("INFO", "SyncEngine", key, "Transferência iniciada (Syncing)"),
                SyncState::Pending => {}
                SyncState::Error(_) => {}
            }
        }
    }

    pub fn remove_state(&self, key: &str) {
        let mut map = self.states.write().unwrap();
        map.remove(key);
    }

    /// Remove todos os estados em memória — usado quando a identidade de sincronização muda.
    pub fn clear_all(&self) {
        self.states.write().unwrap().clear();
    }

    pub fn get_all(&self) -> Vec<FileStatus> {
        let map = self.states.read().unwrap();
        let mut list: Vec<FileStatus> = map.values().cloned().collect();
        list.sort_by(|a, b| a.path.cmp(&b.path));
        list
    }

    /// Calcula o estado global seguindo a prioridade: Offline > Error > Syncing/Pending > UpToDate (Requisito 7.2, 7.3)
    pub fn global_state(&self) -> GlobalSyncState {
        if let Some(ref nd) = self.network_detector {
            if !nd.is_online() {
                return GlobalSyncState::Offline;
            }
        }
        let map = self.states.read().unwrap();
        let states: Vec<SyncState> = map.values().map(|fs| fs.state.clone()).collect();
        compute_global_state(&states)
    }

    /// Registra detalhes do erro em arquivo de log com precisão de segundos (Requisito 7.6)
    pub fn log_error(&self, file_name: &str, error_type: &str) {
        self.write_log("ERROR", "StatusMonitor", file_name, error_type);
    }

    pub fn write_log(&self, level: &str, component: &str, file_name: &str, detail: &str) {
        let base_dir = self.log_dir.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("amazon-s3-drive")
        });

        if fs::create_dir_all(&base_dir).is_ok() {
            let log_path = base_dir.join("sync.log");
            let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
            let line = format!(
                "[{}] [{}] [{}] file=\"{}\" detail=\"{}\"\n",
                timestamp, level, component, file_name, detail
            );
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }

    pub fn read_logs(&self, max_lines: usize) -> Vec<String> {
        let base_dir = self.log_dir.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("amazon-s3-drive")
        });
        let log_path = base_dir.join("sync.log");
        if let Ok(content) = fs::read_to_string(log_path) {
            let all_lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let skip = all_lines.len().saturating_sub(max_lines);
            all_lines.into_iter().skip(skip).collect()
        } else {
            Vec::new()
        }
    }

    pub fn clear_logs(&self) -> Result<(), String> {
        let base_dir = self.log_dir.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("amazon-s3-drive")
        });
        let log_path = base_dir.join("sync.log");
        if log_path.exists() {
            fs::write(log_path, b"").map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_sync_state() -> impl Strategy<Value = SyncState> {
        prop_oneof![
            Just(SyncState::Pending),
            Just(SyncState::Syncing),
            Just(SyncState::Synced),
            "[a-zA-Z0-9 ]{1,20}".prop_map(SyncState::Error),
        ]
    }

    // Feature: amazon-s3-drive, Property 11: Global state priority
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn global_state_follows_priority(states in prop::collection::vec(arb_sync_state(), 1..=100)) {
            let global = compute_global_state(&states);
            let has_error = states.iter().any(|s| matches!(s, SyncState::Error(_)));
            let has_syncing = states.iter().any(|s| matches!(s, SyncState::Syncing | SyncState::Pending));
            let all_synced = states.iter().all(|s| matches!(s, SyncState::Synced));

            if has_error {
                prop_assert_eq!(global, GlobalSyncState::Error);
            } else if has_syncing {
                prop_assert_eq!(global, GlobalSyncState::Syncing);
            } else if all_synced {
                prop_assert_eq!(global, GlobalSyncState::UpToDate);
            }
        }
    }

    #[test]
    fn test_individual_state_transitions() {
        let monitor = StatusMonitor::default();
        assert_eq!(monitor.global_state(), GlobalSyncState::UpToDate);

        monitor.set_state("file1.txt", SyncState::Pending);
        assert_eq!(monitor.global_state(), GlobalSyncState::Syncing);

        monitor.set_state("file1.txt", SyncState::Syncing);
        assert_eq!(monitor.global_state(), GlobalSyncState::Syncing);

        monitor.set_state("file1.txt", SyncState::Synced);
        assert_eq!(monitor.global_state(), GlobalSyncState::UpToDate);
    }

    #[test]
    fn test_synced_plus_error_returns_error() {
        let monitor = StatusMonitor::default();
        monitor.set_state("a.txt", SyncState::Synced);
        monitor.set_state("b.txt", SyncState::Error("Network error".to_string()));
        assert_eq!(monitor.global_state(), GlobalSyncState::Error);
    }
}
