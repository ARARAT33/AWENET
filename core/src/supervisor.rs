//! Live peer supervision: authenticated reconnect, heartbeat, and bounded backoff.
//!
//! A supervisor owns one long-lived secure session per configured peer endpoint.
//! When a session fails or a heartbeat times out it closes that session and
//! reconnects with bounded exponential backoff. No central server is involved.

use crate::network::{NetworkError, Node};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{task::JoinHandle, time::sleep};

const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(20);
const DEFAULT_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(60);
const DEFAULT_MAX_ATTEMPTS_BEFORE_CAP: u32 = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SupervisorConfig {
    pub heartbeat_interval_secs: u64,
    pub initial_backoff_secs: u64,
    pub max_backoff_secs: u64,
    pub max_attempts_before_cap: u32,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval_secs: DEFAULT_HEARTBEAT.as_secs(),
            initial_backoff_secs: DEFAULT_INITIAL_BACKOFF.as_secs(),
            max_backoff_secs: DEFAULT_MAX_BACKOFF.as_secs(),
            max_attempts_before_cap: DEFAULT_MAX_ATTEMPTS_BEFORE_CAP,
        }
    }
}

impl SupervisorConfig {
    fn heartbeat(&self) -> Duration {
        Duration::from_secs(self.heartbeat_interval_secs.max(1))
    }

    fn backoff(&self, failures: u32) -> Duration {
        let initial = self.initial_backoff_secs.max(1);
        let max = self.max_backoff_secs.max(initial);
        let shift = failures.min(self.max_attempts_before_cap.max(1));
        let seconds = initial.saturating_mul(1u64 << shift.min(20));
        Duration::from_secs(seconds.min(max))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SupervisorSnapshot {
    pub connection_attempts: u64,
    pub successful_sessions: u64,
    pub reconnects: u64,
    pub heartbeat_failures: u64,
}

#[derive(Default)]
struct Counters {
    connection_attempts: AtomicU64,
    successful_sessions: AtomicU64,
    reconnects: AtomicU64,
    heartbeat_failures: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> SupervisorSnapshot {
        SupervisorSnapshot {
            connection_attempts: self.connection_attempts.load(Ordering::Relaxed),
            successful_sessions: self.successful_sessions.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            heartbeat_failures: self.heartbeat_failures.load(Ordering::Relaxed),
        }
    }
}

/// Supervises one or more real TCP peers without a central coordinator.
#[derive(Clone)]
pub struct PeerSupervisor {
    node: Node,
    endpoints: Vec<SocketAddr>,
    config: SupervisorConfig,
    counters: Arc<Counters>,
}

impl PeerSupervisor {
    pub fn new(
        node: Node,
        endpoints: Vec<SocketAddr>,
        config: SupervisorConfig,
    ) -> Result<Self, NetworkError> {
        if endpoints.is_empty() {
            return Err(NetworkError::Protocol(
                "peer supervisor requires at least one endpoint".into(),
            ));
        }
        Ok(Self {
            node,
            endpoints,
            config,
            counters: Arc::new(Counters::default()),
        })
    }

    pub fn snapshot(&self) -> SupervisorSnapshot {
        self.counters.snapshot()
    }

    /// Start supervision. The returned task runs until it is aborted.
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut workers = tokio::task::JoinSet::new();
            for endpoint in self.endpoints {
                let node = self.node.clone();
                let config = self.config.clone();
                let counters = Arc::clone(&self.counters);
                workers.spawn(async move {
                    supervise_endpoint(node, endpoint, config, counters).await;
                });
            }
            while workers.join_next().await.is_some() {}
        })
    }
}

async fn supervise_endpoint(
    node: Node,
    endpoint: SocketAddr,
    config: SupervisorConfig,
    counters: Arc<Counters>,
) {
    let mut failures = 0u32;
    let mut sequence = 0u64;

    loop {
        counters.connection_attempts.fetch_add(1, Ordering::Relaxed);
        match node.connect(endpoint).await {
            Ok(mut connection) => {
                counters.successful_sessions.fetch_add(1, Ordering::Relaxed);
                failures = 0;

                loop {
                    match connection.ping_roundtrip(sequence).await {
                        Ok(_) => {
                            sequence = sequence.wrapping_add(1);
                            sleep(config.heartbeat()).await;
                        }
                        Err(_) => {
                            counters.heartbeat_failures.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
            Err(_) => {
                failures = failures.saturating_add(1);
            }
        }

        counters.reconnects.fetch_add(1, Ordering::Relaxed);
        sleep(config.backoff(failures)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded_and_exponential() {
        let c = SupervisorConfig {
            initial_backoff_secs: 1,
            max_backoff_secs: 8,
            ..Default::default()
        };
        assert_eq!(c.backoff(0), Duration::from_secs(2));
        assert_eq!(c.backoff(1), Duration::from_secs(4));
        assert_eq!(c.backoff(2), Duration::from_secs(8));
        assert_eq!(c.backoff(10), Duration::from_secs(8));
    }

    #[test]
    fn empty_endpoint_list_is_rejected() {
        let identity = crate::identity::Identity::generate(
            crate::identity::Username::new("supervisor-test").unwrap(),
        );
        let node = Node::new(identity, "127.0.0.1:0".parse().unwrap());
        assert!(PeerSupervisor::new(node, vec![], SupervisorConfig::default()).is_err());
    }
}
