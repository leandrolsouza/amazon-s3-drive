use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct NetworkDetector {
    is_online: Arc<AtomicBool>,
    probe: Arc<Mutex<Option<CancellationToken>>>,
    callbacks: Arc<Mutex<Vec<Box<dyn Fn(bool) + Send + 'static>>>>,
}

impl Default for NetworkDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkDetector {
    pub fn new() -> Self {
        Self {
            is_online: Arc::new(AtomicBool::new(true)),
            probe: Arc::new(Mutex::new(None)),
            callbacks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn is_online(&self) -> bool {
        self.is_online.load(Ordering::SeqCst)
    }

    pub fn set_online(&self, online: bool) {
        let prev = self.is_online.swap(online, Ordering::SeqCst);
        if prev != online {
            let cbs = self.callbacks.lock().unwrap();
            for cb in cbs.iter() {
                cb(online);
            }
        }
    }

    pub fn on_state_change(&self, callback: impl Fn(bool) + Send + 'static) {
        self.callbacks.lock().unwrap().push(Box::new(callback));
    }

    /// Inicia loop em background: tenta conexão TCP ao endpoint S3 a cada 10s (Requisito 8.1, 8.3)
    pub fn start(&self, s3_endpoint: &str) {
        let mut probe = self.probe.lock().unwrap();
        if probe.is_some() {
            return;
        }
        let cancel = CancellationToken::new();
        *probe = Some(cancel.clone());
        let endpoint = if s3_endpoint.contains(':') {
            s3_endpoint.to_string()
        } else {
            format!("{}:443", s3_endpoint)
        };
        let detector = self.clone();
        tokio::spawn(async move {
            let probe_loop = async {
                loop {
                    let connected = tokio::time::timeout(
                        Duration::from_secs(3),
                        TcpStream::connect(&endpoint),
                    )
                    .await
                    .is_ok_and(|result| result.is_ok());
                    {
                        let _active = detector.probe.lock().unwrap();
                        if cancel.is_cancelled() {
                            return;
                        }
                        detector.set_online(connected);
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = probe_loop => {}
            }
        });
    }

    pub fn stop(&self) {
        if let Some(cancel) = self.probe.lock().unwrap().take() {
            cancel.cancel();
        }
    }

    pub(crate) fn clear_callbacks(&self) {
        self.callbacks.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transition_invokes_callback() {
        let detector = NetworkDetector::new();
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let transitions_clone = Arc::clone(&transitions);

        detector.on_state_change(move |online| {
            transitions_clone.lock().unwrap().push(online);
        });

        assert!(detector.is_online());

        // Transição online -> offline
        detector.set_online(false);
        assert!(!detector.is_online());

        // Chamar novamente com false não deve disparar callback duplicado
        detector.set_online(false);

        // Transição offline -> online
        detector.set_online(true);
        assert!(detector.is_online());

        let recorded = transitions.lock().unwrap().clone();
        assert_eq!(recorded, vec![false, true]);
    }
}
