use crate::{
    membership::Snapshot,
    now,
    presence::{Presence, HEARTBEAT_INTERVAL},
    storage::{State, Storage},
    NodeConfig, NodeId, PairingTicket,
};
use anyhow::{ensure, Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::task::JoinSet;

pub(crate) const PAIR_ALPN: &[u8] = b"spirit/pair/1";
pub(crate) const SYNC_ALPN: &[u8] = b"spirit/mesh/1";
pub(crate) const PING_ALPN: &[u8] = b"spirit/ping/1";
const MAX_MESSAGE: usize = 256 * 1024;

pub(crate) struct Shared {
    pub storage: Storage,
    pub state: Mutex<State>,
    pub pending: Mutex<Option<PairingTicket>>,
    pub presence: Mutex<BTreeMap<NodeId, Presence>>,
    pub endpoint: Endpoint,
    pub config: NodeConfig,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Enrollment {
    pub secret: [u8; 32],
    pub snapshot: Snapshot,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct EnrollmentReply {
    pub joined: std::result::Result<Snapshot, String>,
}

impl Shared {
    pub fn heartbeat_received(&self, id: NodeId) {
        self.presence
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .heartbeat_received(Instant::now());
    }

    fn failed(&self, id: NodeId, error: &anyhow::Error) {
        if self
            .state
            .lock()
            .unwrap()
            .mesh
            .as_ref()
            .and_then(|mesh| mesh.member(id))
            .is_some()
        {
            self.presence
                .lock()
                .unwrap()
                .entry(id)
                .or_default()
                .failed(format!("{error:#}"));
        }
    }

    fn record_failure<T>(&self, id: NodeId, result: &Result<T>) {
        if let Err(error) = result {
            self.failed(id, error);
        }
    }

    pub async fn ping(&self, id: NodeId) -> Result<u128> {
        let result = async {
            let address = self.address(id);
            self.sync(address.clone()).await?;
            let start = Instant::now();
            let response: String = self.request(address, PING_ALPN, &"ping").await?;
            ensure!(response == "pong", "invalid pong response");
            Ok(start.elapsed().as_millis())
        }
        .await;
        match &result {
            Ok(_) => self.heartbeat_received(id),
            Err(error) => self.failed(id, error),
        }
        result
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        self.state.lock().unwrap().snapshot(self.endpoint.addr())
    }

    pub fn address(&self, id: NodeId) -> EndpointAddr {
        self.state
            .lock()
            .unwrap()
            .addresses
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.into())
    }

    pub fn merge(&self, snapshot: &Snapshot, source: NodeId) -> Result<()> {
        snapshot.verify()?;
        ensure!(
            snapshot.mesh.member(source).is_some(),
            "peer is not a mesh member"
        );
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        next.merge(snapshot, source)?;
        if serde_json::to_vec(&next)? != serde_json::to_vec(&*state)? {
            self.storage.save(&next)?;
            *state = next;
        }
        Ok(())
    }

    pub async fn request<T: Serialize, R: DeserializeOwned>(
        &self,
        address: EndpointAddr,
        alpn: &[u8],
        request: &T,
    ) -> Result<R> {
        let id = address.id;
        let result = self.exchange(address, alpn, request).await;
        if let Err(error) = &result {
            self.failed(id, error);
        }
        result
    }

    async fn exchange<T: Serialize, R: DeserializeOwned>(
        &self,
        address: EndpointAddr,
        alpn: &[u8],
        request: &T,
    ) -> Result<R> {
        let bytes = serde_json::to_vec(request)?;
        ensure!(bytes.len() <= MAX_MESSAGE, "request is too large");
        let connection = tokio::time::timeout(
            self.config.request_timeout,
            self.endpoint.connect(address, alpn),
        )
        .await
        .context("connection timed out")??;
        let result = tokio::time::timeout(self.config.request_timeout, async {
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(&bytes).await?;
            send.finish()?;
            let response = recv.read_to_end(MAX_MESSAGE).await?;
            Ok::<R, anyhow::Error>(serde_json::from_slice(&response)?)
        })
        .await
        .context("request timed out");
        connection.close(0u32.into(), b"done");
        result?
    }

    pub async fn sync(&self, address: EndpointAddr) -> Result<()> {
        let id = address.id;
        let result = async {
            let snapshot = self.snapshot()?;
            let response: Snapshot = self.request(address, SYNC_ALPN, &snapshot).await?;
            self.merge(&response, id)
        }
        .await;
        self.record_failure(id, &result);
        result
    }

    fn enroll(&self, request: Enrollment, remote: NodeId) -> Result<Snapshot> {
        let mut pending = self.pending.lock().unwrap();
        let ticket = pending
            .as_ref()
            .context("pairing ticket is unavailable or already consumed")?;
        ensure!(
            bool::from(ticket.secret.ct_eq(&request.secret)),
            "invalid pairing secret"
        );
        ensure!(ticket.expires_at > now()?, "pairing ticket has expired");
        request.snapshot.verify()?;
        ensure!(
            request.snapshot.mesh.member(remote).is_some(),
            "introducer is not a mesh member"
        );
        self.merge(&request.snapshot, remote)?;
        *pending = None;
        self.snapshot()
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Pair,
    Sync,
    Ping,
}

pub(crate) struct Protocol {
    shared: Arc<Shared>,
    kind: Kind,
}

impl fmt::Debug for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Protocol")
            .field("kind", &self.kind)
            .finish()
    }
}

impl Protocol {
    pub fn pair(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            kind: Kind::Pair,
        }
    }
    pub fn sync(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            kind: Kind::Sync,
        }
    }
    pub fn ping(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            kind: Kind::Ping,
        }
    }

    async fn respond(&self, connection: &Connection) -> Result<()> {
        let remote = connection.remote_id();
        if matches!(self.kind, Kind::Ping) {
            let state = self.shared.state.lock().unwrap();
            ensure!(
                state.mesh.as_ref().and_then(|m| m.member(remote)).is_some(),
                "peer is not a mesh member"
            );
        }
        let (mut send, mut recv) = connection.accept_bi().await?;
        let limit = if matches!(self.kind, Kind::Ping) {
            16
        } else {
            MAX_MESSAGE
        };
        let bytes = recv.read_to_end(limit).await?;
        let response = match self.kind {
            Kind::Pair => {
                let request: Enrollment = serde_json::from_slice(&bytes)?;
                serde_json::to_vec(&EnrollmentReply {
                    joined: self
                        .shared
                        .enroll(request, remote)
                        .map_err(|e| e.to_string()),
                })?
            }
            Kind::Sync => {
                ensure!(
                    self.shared.state.lock().unwrap().mesh.is_some(),
                    "device is not enrolled"
                );
                let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
                self.shared.merge(&snapshot, remote)?;
                serde_json::to_vec(&self.shared.snapshot()?)?
            }
            Kind::Ping => {
                let request: String = serde_json::from_slice(&bytes)?;
                ensure!(request == "ping", "invalid ping");
                self.shared.heartbeat_received(remote);
                serde_json::to_vec("pong")?
            }
        };
        ensure!(response.len() <= MAX_MESSAGE, "response is too large");
        send.write_all(&response).await?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

impl ProtocolHandler for Protocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let result = tokio::time::timeout(
            self.shared.config.request_timeout,
            self.respond(&connection),
        )
        .await;
        connection.close(0u32.into(), b"finished");
        let result = result
            .context("incoming request timed out")
            .and_then(|result| result);
        if let Err(error) = &result {
            self.shared.failed(connection.remote_id(), error);
        }
        result.map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

pub(crate) async fn gossip(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut in_flight = BTreeSet::new();
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let peers: Vec<_> = {
                    let state = shared.state.lock().unwrap();
                    state.mesh.as_ref().map(|mesh| mesh.admissions.iter()
                        .map(|admission| admission.member.id)
                        .filter(|id| *id != state.member.id).collect()).unwrap_or_default()
                };
                for id in peers {
                    if !in_flight.insert(id) { continue; }
                    let shared = shared.clone();
                    tasks.spawn(async move {
                        let result = tokio::time::timeout(HEARTBEAT_INTERVAL - Duration::from_secs(1), shared.ping(id))
                            .await.context("heartbeat timed out").and_then(|result| result);
                        if let Err(error) = result { shared.failed(id, &error); }
                        id
                    });
                }
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Ok(id) = result { in_flight.remove(&id); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, PairingTicket};

    async fn device(name: &str) -> (tempfile::TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), name).unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        (dir, node)
    }

    #[tokio::test]
    async fn server_rejects_outsider_pings_wrong_mesh_sync_and_bad_secrets() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.new_mesh("one").unwrap();
        b.new_mesh("two").unwrap();
        let reply: Result<String> = b
            .shared
            .request(a.shared.endpoint.addr(), PING_ALPN, &"ping")
            .await;
        assert!(reply.is_err());
        assert!(b.shared.sync(a.shared.endpoint.addr()).await.is_err());
        assert_eq!(a.info().members.len(), 1);
        let (_c_dir, c) = device("c").await;
        let valid = c.pair(Duration::from_secs(60)).await.unwrap();
        let mut forged = PairingTicket::decode(&valid).unwrap();
        forged.secret[0] ^= 1;
        assert!(a.add(&forged.encode().unwrap()).await.is_err());
        assert!(c.info().mesh_id.is_none());
        let replacement = c.pair(Duration::from_secs(60)).await.unwrap();
        assert!(a.add(&valid).await.is_err());
        a.add(&replacement).await.unwrap();
        assert_eq!(c.ping(a.info().id).await.unwrap().id, a.info().id);
        let reply: Result<String> = c
            .shared
            .request(a.shared.endpoint.addr(), PING_ALPN, &"not ping")
            .await;
        assert!(reply.is_err());
        c.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn only_valid_ping_and_pong_messages_refresh_presence() {
        let (a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        a.new_mesh("one").unwrap();
        let b_id = b.info().id;
        a.add(&b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        assert!(!a.peers()[0].connected);
        assert!(a.peers()[0].last_received_ago_ms.is_none());
        assert!(!b.peers()[0].connected);
        assert!(b.peers()[0].last_received_ago_ms.is_none());

        a.shared.sync(b.shared.endpoint.addr()).await.unwrap();
        assert!(a.peers()[0].last_received_ago_ms.is_none());
        assert!(b.peers()[0].last_received_ago_ms.is_none());

        let invalid: Result<String> = a
            .shared
            .request(b.shared.endpoint.addr(), PING_ALPN, &"not ping")
            .await;
        assert!(invalid.is_err());
        assert!(a.peers()[0].last_received_ago_ms.is_none());
        assert!(b.peers()[0].last_received_ago_ms.is_none());

        a.ping(b_id).await.unwrap();
        assert!(a.peers()[0].connected);
        assert!(a.peers()[0].last_received_ago_ms.is_some());
        assert!(b.peers()[0].connected);
        assert!(b.peers()[0].last_received_ago_ms.is_some());

        a.shutdown().await.unwrap();
        drop(a);
        let a = Node::bind(a_dir.path(), NodeConfig::local()).await.unwrap();
        a.gossip.abort();
        assert!(!a.peers()[0].connected);
        assert!(a.peers()[0].last_received_ago_ms.is_none());
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn receiver_checks_its_own_ticket_expiry() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.new_mesh("one").unwrap();
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        b.shared
            .pending
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .expires_at = now().unwrap() - 1;
        assert!(a
            .add(&ticket)
            .await
            .unwrap_err()
            .to_string()
            .contains("expired"));
        assert!(b.info().mesh_id.is_none());
        b.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }
}
