mod membership;
mod mesh_id;
mod network;
mod presence;
mod storage;

pub use iroh::EndpointId as NodeId;
pub use membership::Member;
pub use mesh_id::MeshId;
pub use presence::{PeerStatus, CONNECTED_WINDOW, HEARTBEAT_INTERVAL};
pub use storage::write_private;

use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{endpoint::presets, protocol::Router, Endpoint, EndpointAddr, SecretKey};
use membership::Mesh;
use network::{Protocol, Shared, DEPART_ALPN, PAIR_ALPN, PING_ALPN, SYNC_ALPN};
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
    pub mesh_id: Option<MeshId>,
    pub mesh_name: Option<String>,
    pub members: Vec<Member>,
    #[serde(default)]
    pub meshes: Vec<MeshInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeshInfo {
    pub id: MeshId,
    pub name: String,
    pub members: Vec<MeshMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeshMember {
    pub id: NodeId,
    pub name: String,
    pub generation: u32,
}

impl NodeInfo {
    pub fn only_mesh(&self) -> Result<MeshId> {
        match self.meshes.as_slice() {
            [mesh] => Ok(mesh.id),
            [] => bail!("device is not enrolled in a mesh"),
            _ => bail!("this device is in several meshes; choose one"),
        }
    }

    pub fn select_mesh(&self, selected: Option<MeshId>) -> Result<MeshId> {
        if let Some(id) = selected {
            ensure!(
                self.meshes.iter().any(|mesh| mesh.id == id),
                "device is not a member of this mesh"
            );
            Ok(id)
        } else {
            self.only_mesh()
        }
    }

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
pub struct LeftMesh {
    pub mesh_id: MeshId,
    pub mesh_name: String,
    pub remaining_members: usize,
    pub notified_members: usize,
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
            ticket.address.addrs.len() <= membership::MAX_ADDRESSES,
            "too many ticket addresses"
        );
        for transport in &ticket.address.addrs {
            ensure!(
                serde_json::to_vec(transport)?.len() <= membership::MAX_TRANSPORT_ADDRESS_BYTES,
                "transport address is too long"
            );
        }
        Ok(ticket)
    }
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

#[cfg(test)]
static TEST_PAIR_ADDRESSES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::BTreeMap<NodeId, EndpointAddr>>,
> = std::sync::OnceLock::new();

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

    pub fn create_mesh(root: impl AsRef<Path>, name: &str) -> Result<MeshId> {
        let (storage, mut state) = Storage::open(root.as_ref())?;
        ensure!(
            state.meshes.len() < storage::MAX_CURRENT_MESHES,
            "device has reached the 64-group limit"
        );
        let mesh = Mesh::create(name, state.member.clone(), &storage.key)?;
        let id = mesh.id;
        state.meshes.insert(id, mesh);
        storage.save(&state)?;
        Ok(id)
    }

    pub fn leave_mesh(root: impl AsRef<Path>, mesh_id: MeshId) -> Result<LeftMesh> {
        let (storage, mut state) = Storage::open(root.as_ref())?;
        let departure = state.leave(mesh_id, &storage.key)?;
        storage.save(&state)?;
        Ok(LeftMesh {
            mesh_id: departure.mesh.id,
            mesh_name: departure.mesh.name.clone(),
            remaining_members: departure.mesh.members().count(),
            notified_members: 0,
        })
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
            .accept(DEPART_ALPN, Protocol::depart(shared.clone()))
            .spawn();
        let gossip = tokio::spawn(network::gossip(shared.clone()));
        Ok(Self {
            shared,
            router,
            gossip,
        })
    }

    pub fn peers(&self) -> Vec<PeerStatus> {
        let state = self.shared.state.lock().unwrap();
        let members: std::collections::BTreeMap<_, _> = state
            .meshes
            .values()
            .flat_map(|mesh| mesh.members())
            .filter(|member| member.id != state.member.id)
            .map(|member| (member.id, member.clone()))
            .collect();
        let presence = self.shared.presence.lock().unwrap();
        let now = Instant::now();
        members
            .into_values()
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

    pub fn new_mesh(&self, name: &str) -> Result<MeshId> {
        let id = self.shared.update(|state| {
            ensure!(
                state.meshes.len() < storage::MAX_CURRENT_MESHES,
                "device has reached the 64-group limit"
            );
            let mesh = Mesh::create(name, state.member.clone(), &self.shared.storage.key)?;
            let id = mesh.id;
            state.meshes.insert(id, mesh);
            Ok((id, true))
        })?;
        *self.shared.pending.lock().unwrap() = None;
        Ok(id)
    }

    pub fn create_first_mesh(&self, name: &str) -> Result<MeshId> {
        let mut pending = self.shared.pending.lock().unwrap();
        let id = self.shared.update(|state| {
            ensure!(
                state.meshes.is_empty(),
                "createMesh is single-mesh until S3; this device already belongs to a mesh"
            );
            let mesh = Mesh::create(name, state.member.clone(), &self.shared.storage.key)?;
            let id = mesh.id;
            state.meshes.insert(id, mesh);
            Ok((id, true))
        })?;
        *pending = None;
        Ok(id)
    }

    pub async fn pair(&self, lifetime: Duration) -> Result<String> {
        self.pair_inner(lifetime, false).await
    }

    pub async fn pair_when_unenrolled(&self, lifetime: Duration) -> Result<String> {
        self.pair_inner(lifetime, true).await
    }

    async fn pair_inner(&self, lifetime: Duration, unenrolled_only: bool) -> Result<String> {
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
        let address = self.shared.endpoint.addr();
        #[cfg(test)]
        let address = TEST_PAIR_ADDRESSES
            .get()
            .and_then(|addresses| addresses.lock().unwrap().remove(&address.id))
            .unwrap_or(address);
        let ticket = PairingTicket {
            address: membership::bounded_address(address),
            name: self.info().name,
            secret: SecretKey::generate().to_bytes(),
            expires_at: now()? + lifetime.as_secs(),
        };
        let encoded = ticket.encode()?;
        let mut pending = self.shared.pending.lock().unwrap();
        if unenrolled_only {
            ensure!(
                self.shared.state.lock().unwrap().meshes.is_empty(),
                "pair is single-mesh until S3; this device already belongs to a mesh"
            );
        }
        *pending = Some(ticket);
        Ok(encoded)
    }

    pub async fn add(&self, mesh_id: MeshId, ticket: &str) -> Result<Member> {
        ensure!(
            self.shared
                .state
                .lock()
                .unwrap()
                .meshes
                .contains_key(&mesh_id),
            "device is not a member of this mesh"
        );
        let ticket = PairingTicket::decode(ticket)?;
        ensure!(
            ticket.address.id != self.info().id,
            "cannot enroll this device into itself"
        );
        let member = Member {
            id: ticket.address.id,
            name: ticket.name.clone(),
        };
        let mut response = self.enroll(mesh_id, &ticket, &member).await?;
        if let Some(departure) = response.departure.take() {
            ensure!(
                departure.mesh.id == mesh_id,
                "departure names a different mesh"
            );
            self.shared.record_departure(&departure, member.id)?;
            response = self.enroll(mesh_id, &ticket, &member).await?;
        }
        let joined = joined_after_retry(response)?;
        ensure!(
            joined.mesh.id == mesh_id,
            "enrollment response names a different mesh"
        );
        ensure!(
            self.shared
                .state
                .lock()
                .unwrap()
                .meshes
                .get(&mesh_id)
                .is_some_and(|mesh| mesh.same_identity(&joined.mesh)),
            "enrollment response has a different mesh identity"
        );
        ensure!(
            joined.mesh.member(member.id) == Some(&member),
            "enrollment did not admit the expected device"
        );
        self.shared.merge_current(&joined, member.id)?;
        Ok(member)
    }

    async fn enroll(
        &self,
        mesh_id: MeshId,
        ticket: &PairingTicket,
        member: &Member,
    ) -> Result<network::EnrollmentReply> {
        let mut snapshot = self.shared.snapshot(mesh_id)?;
        snapshot
            .mesh
            .admit(member.clone(), &self.shared.storage.key)?;
        snapshot.addresses.insert(member.id, ticket.address.clone());
        let request = network::Enrollment {
            secret: ticket.secret,
            snapshot,
        };
        self.shared
            .request(ticket.address.clone(), PAIR_ALPN, &request)
            .await
    }

    pub async fn leave(&self, mesh_id: MeshId) -> Result<LeftMesh> {
        let (departure, peers) = {
            let mut pending = self.shared.pending.lock().unwrap();
            let (departure, peers) = self.shared.update(|state| {
                let peers: Vec<_> = state
                    .meshes
                    .get(&mesh_id)
                    .context("device is not a member of this mesh")?
                    .members()
                    .filter(|member| member.id != state.member.id)
                    .map(|member| {
                        state
                            .addresses
                            .get(&member.id)
                            .and_then(|entry| entry.dial().cloned())
                            .unwrap_or_else(|| member.id.into())
                    })
                    .collect();
                Ok((
                    (state.leave(mesh_id, &self.shared.storage.key)?, peers),
                    true,
                ))
            })?;
            *pending = None;
            (departure, peers)
        };
        let remaining_members = peers.len();
        let left = LeftMesh {
            mesh_id: departure.mesh.id,
            mesh_name: departure.mesh.name.clone(),
            remaining_members,
            notified_members: 0,
        };
        let notified_members = self.shared.announce_departure(departure, peers).await;
        Ok(LeftMesh {
            notified_members,
            ..left
        })
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

fn joined_after_retry(response: network::EnrollmentReply) -> Result<membership::Snapshot> {
    ensure!(
        response.departure.is_none(),
        "device refused readmission twice; update the introducing device if it runs an older Spirit"
    );
    response
        .joined
        .map_err(|error| anyhow::anyhow!(network::bounded_diagnostic(&error)))
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
    fn a_second_departure_refusal_is_an_error_not_another_retry() {
        let key = SecretKey::generate();
        let mesh = Mesh::create(
            "home",
            Member {
                id: key.public(),
                name: "device".into(),
            },
            &key,
        )
        .unwrap();
        let reply = network::EnrollmentReply {
            joined: Err("this device left the mesh".into()),
            departure: Some(membership::Snapshot::without_addresses(mesh)),
        };
        assert!(joined_after_retry(reply)
            .unwrap_err()
            .to_string()
            .contains("refused readmission twice"));
    }

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
            mesh_id: Some(MeshId::legacy(a.id)),
            mesh_name: Some("home".into()),
            members: vec![a.clone(), b.clone()],
            meshes: Vec::new(),
        };
        assert!(info
            .resolve("desktop")
            .unwrap_err()
            .to_string()
            .contains("ambiguous"));
        assert_eq!(info.resolve(&b.id.to_string()).unwrap(), b);
    }
}
