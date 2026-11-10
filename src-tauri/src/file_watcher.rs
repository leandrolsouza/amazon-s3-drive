use crate::models::{LocalFsEvent, WatchError};
use notify::{
    event::{ModifyKind, RenameMode},
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

pub const DEBOUNCE_DURATION: Duration = Duration::from_secs(2);

pub struct FileWatcher {
    watch_path: PathBuf,
    watcher: Option<RecommendedWatcher>,
    event_tx: mpsc::Sender<LocalFsEvent>,
    raw_tx: Option<mpsc::UnboundedSender<LocalFsEvent>>,
}

impl FileWatcher {
    pub fn new(watch_path: PathBuf, event_tx: mpsc::Sender<LocalFsEvent>) -> Result<Self, WatchError> {
        Ok(Self {
            watch_path,
            watcher: None,
            event_tx,
            raw_tx: None,
        })
    }

    pub fn spawn_debouncer(
        mut raw_rx: mpsc::UnboundedReceiver<LocalFsEvent>,
        out_tx: mpsc::Sender<LocalFsEvent>,
        debounce_duration: Duration,
    ) {
        tokio::spawn(async move {
            let pending_modifications: Arc<Mutex<HashMap<PathBuf, Instant>>> =
                Arc::new(Mutex::new(HashMap::new()));

            while let Some(event) = raw_rx.recv().await {
                match event {
                    LocalFsEvent::Modified(path) => {
                        let now = Instant::now();
                        {
                            let mut map = pending_modifications.lock().unwrap();
                            map.insert(path.clone(), now);
                        }
                        let map_clone = Arc::clone(&pending_modifications);
                        let tx_clone = out_tx.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(debounce_duration).await;
                            let should_emit = {
                                let mut map = map_clone.lock().unwrap();
                                if let Some(&last_instant) = map.get(&path) {
                                    if now == last_instant {
                                        map.remove(&path);
                                        true
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            };
                            if should_emit {
                                let _ = tx_clone.send(LocalFsEvent::Modified(path)).await;
                            }
                        });
                    }
                    other => {
                        let _ = out_tx.send(other).await;
                    }
                }
            }
        });
    }

    pub fn map_notify_event(
        event: Event,
        pending_rename_from: &mut Option<PathBuf>,
    ) -> Vec<LocalFsEvent> {
        let mut mapped = Vec::new();

        match event.kind {
            EventKind::Create(_) => {
                for path in event.paths {
                    mapped.push(LocalFsEvent::Created(path));
                }
            }
            EventKind::Modify(ModifyKind::Name(rename_mode)) => match rename_mode {
                RenameMode::Both => {
                    if event.paths.len() >= 2 {
                        mapped.push(LocalFsEvent::Renamed {
                            from: event.paths[0].clone(),
                            to: event.paths[1].clone(),
                        });
                    }
                }
                RenameMode::From => {
                    if let Some(p) = event.paths.into_iter().next() {
                        *pending_rename_from = Some(p);
                    }
                }
                RenameMode::To => {
                    if let Some(to) = event.paths.into_iter().next() {
                        if let Some(from) = pending_rename_from.take() {
                            mapped.push(LocalFsEvent::Renamed { from, to });
                        } else {
                            mapped.push(LocalFsEvent::Created(to));
                        }
                    }
                }
                _ => {
                    if event.paths.len() >= 2 {
                        mapped.push(LocalFsEvent::Renamed {
                            from: event.paths[0].clone(),
                            to: event.paths[1].clone(),
                        });
                    }
                }
            },
            EventKind::Modify(_) => {
                for path in event.paths {
                    mapped.push(LocalFsEvent::Modified(path));
                }
            }
            EventKind::Remove(_) => {
                for path in event.paths {
                    mapped.push(LocalFsEvent::Deleted(path));
                }
            }
            _ => {}
        }

        mapped
    }

    pub fn start(&mut self) -> Result<(), WatchError> {
        if !self.watch_path.exists() {
            return Err(WatchError::InvalidPath(format!(
                "Watch path does not exist: {}",
                self.watch_path.display()
            )));
        }

        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<LocalFsEvent>();
        Self::spawn_debouncer(raw_rx, self.event_tx.clone(), DEBOUNCE_DURATION);

        let tx_for_cb = raw_tx.clone();
        let pending_rename: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let mut guard = pending_rename.lock().unwrap();
                    let events = Self::map_notify_event(event, &mut guard);
                    for ev in events {
                        let _ = tx_for_cb.send(ev);
                    }
                }
            },
            Config::default(),
        )
        .map_err(|e| WatchError::Notify(e.to_string()))?;

        watcher
            .watch(&self.watch_path, RecursiveMode::Recursive)
            .map_err(|e| WatchError::Notify(e.to_string()))?;

        self.watcher = Some(watcher);
        self.raw_tx = Some(raw_tx);
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut watcher) = self.watcher.take() {
            let _ = watcher.unwatch(&self.watch_path);
        }
        self.raw_tx = None;
    }

    pub fn update_path(&mut self, new_path: PathBuf) -> Result<(), WatchError> {
        self.stop();
        self.watch_path = new_path;
        self.start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange};

    #[tokio::test]
    async fn test_debounce_collapses_rapid_modifications() {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let (out_tx, mut out_rx) = mpsc::channel(16);

        // Usamos debounce de 2s conforme especificação (Requisito 2.2)
        FileWatcher::spawn_debouncer(raw_rx, out_tx, Duration::from_secs(2));

        let file_path = PathBuf::from("/tmp/test_debounce.txt");
        raw_tx
            .send(LocalFsEvent::Modified(file_path.clone()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        raw_tx
            .send(LocalFsEvent::Modified(file_path.clone()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        raw_tx
            .send(LocalFsEvent::Modified(file_path.clone()))
            .unwrap();

        let received = tokio::time::timeout(Duration::from_millis(2600), out_rx.recv())
            .await
            .expect("Expected debounced Modified event")
            .unwrap();

        assert_eq!(received, LocalFsEvent::Modified(file_path));

        // Garantir que nenhum outro evento foi emitido
        let extra = tokio::time::timeout(Duration::from_millis(300), out_rx.recv()).await;
        assert!(extra.is_err(), "Should not emit extra Modified events");
    }

    #[test]
    fn test_rename_event_mapping() {
        let mut pending = None;
        let from_path = PathBuf::from("/tmp/old_name.txt");
        let to_path = PathBuf::from("/tmp/new_name.txt");

        let ev_both = Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            paths: vec![from_path.clone(), to_path.clone()],
            attrs: Default::default(),
        };
        let mapped = FileWatcher::map_notify_event(ev_both, &mut pending);
        assert_eq!(
            mapped,
            vec![LocalFsEvent::Renamed {
                from: from_path.clone(),
                to: to_path.clone()
            }]
        );

        // Testar par From + To separado
        let ev_from = Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            paths: vec![from_path.clone()],
            attrs: Default::default(),
        };
        assert!(FileWatcher::map_notify_event(ev_from, &mut pending).is_empty());

        let ev_to = Event {
            kind: EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            paths: vec![to_path.clone()],
            attrs: Default::default(),
        };
        let mapped_pair = FileWatcher::map_notify_event(ev_to, &mut pending);
        assert_eq!(
            mapped_pair,
            vec![LocalFsEvent::Renamed {
                from: from_path,
                to: to_path
            }]
        );

        // Create & Modify
        let ev_create = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("/tmp/a.txt")],
            attrs: Default::default(),
        };
        assert_eq!(
            FileWatcher::map_notify_event(ev_create, &mut pending),
            vec![LocalFsEvent::Created(PathBuf::from("/tmp/a.txt"))]
        );

        let ev_mod = Event {
            kind: EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            paths: vec![PathBuf::from("/tmp/a.txt")],
            attrs: Default::default(),
        };
        assert_eq!(
            FileWatcher::map_notify_event(ev_mod, &mut pending),
            vec![LocalFsEvent::Modified(PathBuf::from("/tmp/a.txt"))]
        );
    }
}
