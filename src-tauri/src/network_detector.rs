use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::net::TcpStream;

#[derive(Clone)]
pub struct NetworkDetector {
    is_online: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
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
            running: Arc::new(AtomicBool::new(false)),
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
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let endpoint = if s3_endpoint.contains(':') {
            s3_endpoint.to_string()
        } else {
            format!("{}:443", s3_endpoint)
        };
        let detector = self.clone();
        tokio::spawn(async move {
            while detector.running.load(Ordering::SeqCst) {
                let connected = tokio::time::timeout(
                    Duration::from_secs(3),
                    TcpStream::connect(&endpoint),
                )
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);

                detector.set_online(connected);
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
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
