mod membership;
mod network;
mod presence;
mod storage;

pub use iroh::EndpointId as NodeId;
pub use membership::Member;
pub use presence::{PeerStatus, CONNECTED_WINDOW, HEARTBEAT_INTERVAL};
pub use storage::write_private;

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{endpoint::presets, protocol::Router, Endpoint, EndpointAddr, SecretKey};
use membership::Mesh;
use network::{Protocol, Shared, PAIR_ALPN, PING_ALPN, SYNC_ALPN};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage::Storage;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: NodeId,
    pub name: String,
    pub mesh_id: Option<NodeId>,
    pub mesh_name: Option<String>,
    pub members: Vec<Member>,
}

impl NodeInfo {
    pub fn resolve(&self, nickname_or_id: &str) -> Result<Member> {
        if let Ok(id) = nickname_or_id.parse::<NodeId>() {
            if let Some(member) = self.members.iter().find(|m| m.id == id) {
                return Ok(member.clone());
            }
        }
        let matches: Vec<_> = self
            .members
            .iter()
            .filter(|m| m.name == nickname_or_id)
            .collect();
        match matches.as_slice() {
            [member] => Ok((*member).clone()),
            [] => bail!("no mesh member named {nickname_or_id}"),
            _ => bail!("nickname {nickname_or_id} is ambiguous; use a device ID from spirit mesh members --ids"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NodeConfig {
    pub local: bool,
    pub request_timeout: Duration,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            local: false,
            request_timeout: Duration::from_secs(10),
        }
    }
}

impl NodeConfig {
    pub fn local() -> Self {
        Self {
            local: true,
            request_timeout: Duration::from_secs(3),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pong {
    pub id: NodeId,
    pub name: String,
    pub elapsed_ms: u128,
}

#[derive(Clone, Serialize, Deserialize)]
struct PairingTicket {
    address: EndpointAddr,
    name: String,
    secret: [u8; 32],
    expires_at: u64,
}

impl PairingTicket {
    fn encode(&self) -> Result<String> {
        Ok(format!(
            "spirit1{}",
            URL_SAFE_NO_PAD.encode(postcard::to_stdvec(self)?)
        ))
    }

    fn decode(ticket: &str) -> Result<Self> {
        ensure!(ticket.len() <= 8192, "pairing ticket is too large");
        let encoded = ticket
            .strip_prefix("spirit1")
            .context("invalid pairing ticket")?;
        let ticket: Self = postcard::from_bytes(
            &URL_SAFE_NO_PAD
                .decode(encoded)
                .context("invalid pairing ticket")?,
        )?;
        membership::validate_name(&ticket.name)?;
        ensure!(ticket.expires_at > now()?, "pairing ticket has expired");
        ensure!(
            ticket.address.addrs.len() <= 16,
            "too many ticket addresses"
        );
        Ok(ticket)
    }
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

pub struct Node {
    shared: Arc<Shared>,
    router: Router,
    gossip: JoinHandle<()>,
}

impl Node {
    pub fn init(root: impl AsRef<Path>, name: &str) -> Result<NodeInfo> {
        Storage::init(root.as_ref(), name)
    }

    pub fn read_info(root: impl AsRef<Path>) -> Result<NodeInfo> {
        Ok(Storage::read(root.as_ref())?.info())
    }

    pub fn create_mesh(root: impl AsRef<Path>, name: &str) -> Result<NodeInfo> {
        let (storage, mut state) = Storage::open(root.as_ref())?;
        ensure!(state.mesh.is_none(), "device already belongs to a mesh");
        state.mesh = Some(Mesh::create(name, state.member.clone(), &storage.key)?);
        storage.save(&state)?;
        Ok(state.info())
    }

    pub async fn bind(root: impl AsRef<Path>, config: NodeConfig) -> Result<Self> {
        ensure!(
            !config.request_timeout.is_zero(),
            "request timeout must be positive"
        );
        let (storage, state) = Storage::open(root.as_ref())?;
        let builder = if config.local {
            Endpoint::builder(presets::Minimal)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")?
        } else {
            Endpoint::builder(presets::N0)
        };
        let endpoint = builder.secret_key(storage.key.clone()).bind().await?;
        let shared = Arc::new(Shared {
            storage,
            state: Mutex::new(state),
            pending: Mutex::new(None),
            presence: Mutex::new(Default::default()),
            endpoint: endpoint.clone(),
            config,
        });
        let router = Router::builder(endpoint)
            .accept(PAIR_ALPN, Protocol::pair(shared.clone()))
            .accept(SYNC_ALPN, Protocol::sync(shared.clone()))
            .accept(PING_ALPN, Protocol::ping(shared.clone()))
            .spawn();
        let gossip = tokio::spawn(network::gossip(shared.clone()));
        Ok(Self {
            shared,
            router,
            gossip,
        })
    }

    pub fn peers(&self) -> Vec<PeerStatus> {
        let info = self.info();
        let presence = self.shared.presence.lock().unwrap();
        let now = Instant::now();
        info.members
            .into_iter()
            .filter(|member| member.id != info.id)
            .map(|member| {
                presence
                    .get(&member.id)
                    .unwrap_or(&presence::Presence::default())
                    .status(member.id, member.name, now)
            })
            .collect()
    }

    pub fn info(&self) -> NodeInfo {
        self.shared.state.lock().unwrap().info()
    }

    pub fn new_mesh(&self, name: &str) -> Result<NodeInfo> {
        let mut state = self.shared.state.lock().unwrap();
        ensure!(state.mesh.is_none(), "device already belongs to a mesh");
        let mut next = state.clone();
        next.mesh = Some(Mesh::create(
            name,
            state.member.clone(),
            &self.shared.storage.key,
        )?);
        self.shared.storage.save(&next)?;
        *state = next;
        Ok(state.info())
    }

    pub async fn pair(&self, lifetime: Duration) -> Result<String> {
        ensure!(
            (1..=3600).contains(&lifetime.as_secs()),
            "ticket lifetime must be 1–3600 seconds"
        );
        if !self.shared.config.local {
            tokio::time::timeout(
                self.shared.config.request_timeout,
                self.shared.endpoint.online(),
            )
            .await
            .context("relay is unavailable; use node serve --local for same-machine testing")?;
        }
        let ticket = PairingTicket {
            address: self.shared.endpoint.addr(),
            name: self.info().name,
            secret: SecretKey::generate().to_bytes(),
            expires_at: now()? + lifetime.as_secs(),
        };
        let encoded = ticket.encode()?;
        *self.shared.pending.lock().unwrap() = Some(ticket);
        Ok(encoded)
    }

    pub async fn add(&self, ticket: &str) -> Result<Member> {
        let ticket = PairingTicket::decode(ticket)?;
        ensure!(
            ticket.address.id != self.info().id,
            "cannot enroll this device into itself"
        );
        let mut snapshot = self.shared.snapshot()?;
        let member = Member {
            id: ticket.address.id,
            name: ticket.name.clone(),
        };
        snapshot
            .mesh
            .admit(member.clone(), &self.shared.storage.key)?;
        snapshot.addresses.insert(member.id, ticket.address.clone());
        let request = network::Enrollment {
            secret: ticket.secret,
            snapshot,
        };
        let response: network::EnrollmentReply = self
            .shared
            .request(ticket.address, PAIR_ALPN, &request)
            .await?;
        let joined = response.joined.map_err(anyhow::Error::msg)?;
        ensure!(
            joined.mesh.member(member.id) == Some(&member),
            "enrollment did not admit the expected device"
        );
        self.shared.merge(&joined, member.id)?;
        self.shared.received(member.id);
        Ok(member)
    }

    pub async fn ping(&self, id: NodeId) -> Result<Pong> {
        let info = self.info();
        let member = info
            .members
            .iter()
            .find(|m| m.id == id)
            .context("device is not a member of this mesh")?;
        ensure!(id != info.id, "choose another mesh member to ping");
        let elapsed_ms = self.shared.ping(id).await?;
        Ok(Pong {
            id,
            name: member.name.clone(),
            elapsed_ms,
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.gossip.abort();
        self.router.shutdown().await?;
        Ok(())
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.gossip.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_nicknames_require_an_id() {
        let a = Member {
            id: SecretKey::generate().public(),
            name: "desktop".into(),
        };
        let b = Member {
            id: SecretKey::generate().public(),
            name: "desktop".into(),
        };
        let info = NodeInfo {
            id: a.id,
            name: a.name.clone(),
            mesh_id: Some(a.id),
            mesh_name: Some("home".into()),
            members: vec![a.clone(), b.clone()],
        };
        assert!(info
            .resolve("desktop")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        assert_eq!(info.resolve(&b.id.to_string()).unwrap(), b);
    }
}
