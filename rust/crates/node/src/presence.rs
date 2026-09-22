use crate::NodeId;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
pub const CONNECTED_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerStatus {
    pub id: NodeId,
    pub name: String,
    pub connected: bool,
    pub last_received_ago_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct Presence {
    received: Option<Instant>,
    error: Option<String>,
}

impl Presence {
    pub fn heartbeat_received(&mut self, now: Instant) {
        self.received = Some(now);
        self.error = None;
    }

    pub fn failed(&mut self, error: String) {
        self.error = Some(error);
    }

    fn connected(&self, now: Instant) -> bool {
        self.received
            .is_some_and(|received| now.saturating_duration_since(received) < CONNECTED_WINDOW)
    }

    pub fn status(&self, id: NodeId, name: String, now: Instant) -> PeerStatus {
        PeerStatus {
            id,
            name,
            connected: self.connected(now),
            last_received_ago_ms: self.received.map(|received| {
                now.saturating_duration_since(received)
                    .as_millis()
                    .min(u64::MAX as u128) as u64
            }),
            last_error: self.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn unknown_then_received_then_stale_at_the_strict_boundary() {
        let start = Instant::now();
        let mut presence = Presence::default();
        assert!(!presence.connected(start));
        presence.heartbeat_received(start);
        assert!(presence.connected(start + Duration::from_millis(59999)));
        assert!(!presence.connected(start + Duration::from_secs(60)));
    }

    #[test]
    fn errors_are_diagnostics_without_shortening_the_heartbeat_window() {
        let start = Instant::now();
        let mut presence = Presence::default();
        presence.heartbeat_received(start);
        presence.failed("connection refused".into());
        let status = presence.status(
            SecretKey::generate().public(),
            "desktop".into(),
            start + Duration::from_secs(1),
        );
        assert!(status.connected);
        assert_eq!(status.last_error.as_deref(), Some("connection refused"));
        presence.heartbeat_received(start + Duration::from_secs(2));
        assert!(presence.connected(start + Duration::from_secs(2)));
        assert!(presence.error.is_none());
    }
}
