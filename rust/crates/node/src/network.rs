use crate::{
    membership::Snapshot,
    now,
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
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
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
        let snapshot = self.snapshot()?;
        let response: Snapshot = self.request(address, SYNC_ALPN, &snapshot).await?;
        self.merge(&response, id)
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
        result
            .map_err(AcceptError::from_err)?
            .map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

pub(crate) async fn gossip(shared: Arc<Shared>) {
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        interval.tick().await;
        let peers: Vec<_> = {
            let state = shared.state.lock().unwrap();
            state
                .mesh
                .as_ref()
                .map(|mesh| {
                    mesh.admissions
                        .iter()
                        .map(|a| a.member.id)
                        .filter(|id| *id != state.member.id)
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut tasks = JoinSet::new();
        for id in peers {
            if tasks.len() >= 16 {
                tasks.join_next().await;
            }
            let shared = shared.clone();
            tasks.spawn(async move {
                let _ = shared.sync(shared.address(id)).await;
            });
        }
        while tasks.join_next().await.is_some() {}
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
