mod app;
mod membership;
mod mesh_id;
mod network;
mod presence;
mod storage;
mod transfer;
mod upload;

pub use app::{validate_app_name, verify_app, AppCallContext, AppHandler, Diagnostic};
pub use iroh::EndpointId as NodeId;
pub use membership::Member;
pub use mesh_id::MeshId;

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("device has reached the 64-group limit")]
    MeshLimit,
    #[error("device is not a member of this mesh")]
    NotMember,
    #[error("{0}")]
    TicketRejected(String),
    #[error("node is closed")]
    NodeClosed,
    #[error("node blob store is not configured")]
    StoreNotConfigured,
    #[error("node directory is in use")]
    NodeBusy,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("network unavailable: {0}")]
    Unavailable(String),
}

pub use presence::{PeerStatus, CONNECTED_WINDOW, HEARTBEAT_INTERVAL};
pub use storage::write_private;
pub use transfer::FetchError;

use anyhow::{bail, ensure, Context, Result};
use app::{
    AppProtocol, AppRequest, AppState, APP_ALPN, MAX_APP_BYTES, MAX_APP_ENCODED, MAX_APP_WIRE,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{endpoint::presets, protocol::Router, Endpoint, EndpointAddr, SecretKey};
use membership::Mesh;
use network::{Protocol, Shared, DEPART_ALPN, PAIR_ALPN, PING_ALPN, SYNC_ALPN};
use serde::{Deserialize, Serialize};
use spirit_core::{BlobHash, BlobStore};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use storage::Storage;
use tokio::sync::watch;
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
                NodeError::NotMember
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

#[derive(Clone, Debug)]
pub struct NodeConfig {
    pub store: Option<PathBuf>,
    pub local: bool,
    pub request_timeout: Duration,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            local: false,
            store: None,
            request_timeout: Duration::from_secs(10),
        }
    }
}

impl NodeConfig {
    pub fn with_store(mut self, path: impl Into<PathBuf>) -> Self {
        self.store = Some(path.into());
        self
    }

    pub fn local() -> Self {
        Self {
            local: true,
            store: None,
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
    closed: AtomicBool,
    app: Arc<AppState>,
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
            NodeError::MeshLimit
        );
        let mesh = Mesh::create(name, state.member.clone(), &storage.key)
            .map_err(|error| NodeError::Invalid(error.to_string()))?;
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
        let store = config
            .store
            .as_ref()
            .map(BlobStore::open)
            .transpose()?
            .map(Arc::new);
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
            diagnostics: Default::default(),
            endpoint: endpoint.clone(),
            config,
            store,
            shares: Mutex::new(BTreeMap::new()),
            uploads: Mutex::new(BTreeMap::new()),
            fetches: Arc::new(tokio::sync::Semaphore::new(transfer::MAX_FETCHES)),
        });
        let app = Arc::new(AppState::default());
        let router = Router::builder(endpoint)
            .accept(PAIR_ALPN, Protocol::pair(shared.clone()))
            .accept(SYNC_ALPN, Protocol::sync(shared.clone()))
            .accept(PING_ALPN, Protocol::ping(shared.clone()))
            .accept(DEPART_ALPN, Protocol::depart(shared.clone()))
            .accept(APP_ALPN, AppProtocol::new(shared.clone(), app.clone()))
            .accept(upload::BLOB_ALPN, upload::BlobProtocol(shared.clone()))
            .spawn();
        let gossip = tokio::spawn(network::gossip(shared.clone()));
        Ok(Self {
            shared,
            router,
            gossip,
            closed: AtomicBool::new(false),
            app,
        })
    }

    pub fn has_pending_ticket(&self) -> bool {
        self.shared.pending.lock().unwrap().is_some()
    }

    fn store(&self) -> Result<&BlobStore> {
        self.ensure_open()?;
        self.shared
            .store
            .as_deref()
            .ok_or(NodeError::StoreNotConfigured.into())
    }

    pub fn import_file(&self, path: impl AsRef<Path>) -> Result<BlobHash> {
        Ok(self.store()?.import_file(path)?)
    }

    pub fn import_reader(&self, reader: impl Read) -> Result<BlobHash> {
        Ok(self.store()?.import_reader(reader)?)
    }

    pub fn export_file(&self, hash: BlobHash, path: impl AsRef<Path>) -> Result<u64> {
        Ok(self.store()?.export_file(hash, path)?)
    }

    pub fn export_to(&self, hash: BlobHash, writer: impl Write) -> Result<u64> {
        Ok(self.store()?.export_to(hash, writer)?)
    }

    pub fn has_blob(&self, hash: BlobHash) -> Result<bool> {
        Ok(self.store()?.has(hash))
    }

    pub fn blob_size(&self, hash: BlobHash) -> Result<u64> {
        Ok(self.store()?.size(hash)?)
    }

    fn update_shares(
        &self,
        mesh: MeshId,
        change: impl FnOnce(&mut BTreeSet<BlobHash>) -> Result<()>,
    ) -> Result<()> {
        self.store()?;
        let state = self.shared.state.lock().unwrap();
        ensure!(state.current_member(mesh), NodeError::NotMember);
        let mut shares = self.shared.shares.lock().unwrap();
        change(shares.entry(mesh).or_default())
    }

    pub fn set_shares(
        &self,
        mesh: MeshId,
        hashes: impl IntoIterator<Item = BlobHash>,
    ) -> Result<()> {
        self.ensure_open()?;
        let hashes: BTreeSet<_> = hashes.into_iter().collect();
        ensure!(
            hashes.len() <= upload::MAX_SHARES_PER_MESH,
            NodeError::Invalid("mesh share limit exceeded".into())
        );
        self.update_shares(mesh, |set| {
            *set = hashes;
            Ok(())
        })
    }

    pub fn share(&self, mesh: MeshId, hash: BlobHash) -> Result<()> {
        self.update_shares(mesh, |set| {
            ensure!(
                set.contains(&hash) || set.len() < upload::MAX_SHARES_PER_MESH,
                NodeError::Invalid("mesh share limit exceeded".into())
            );
            set.insert(hash);
            Ok(())
        })
    }

    pub fn unshare(&self, mesh: MeshId, hash: BlobHash) -> Result<()> {
        self.update_shares(mesh, |set| {
            set.remove(&hash);
            Ok(())
        })
    }

    pub async fn fetch(
        &self,
        mesh: MeshId,
        provider: NodeId,
        hash: BlobHash,
        expected_size: Option<u64>,
        progress: impl Fn(u64, u64),
    ) -> std::result::Result<u64, FetchError> {
        self.ensure_open()?;
        let (_sender, mut cancel) = watch::channel(false);
        transfer::fetch(
            &self.shared,
            mesh,
            provider,
            hash,
            expected_size,
            transfer::FetchCallbacks {
                progress,
                queued: || {},
            },
            &mut cancel,
        )
        .await
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "preserve the public cancellable fetch API"
    )]
    pub async fn fetch_cancellable(
        &self,
        mesh: MeshId,
        provider: NodeId,
        hash: BlobHash,
        expected_size: Option<u64>,
        progress: impl Fn(u64, u64),
        queued: impl Fn(),
        mut cancel: watch::Receiver<bool>,
    ) -> std::result::Result<u64, FetchError> {
        self.ensure_open()?;
        transfer::fetch(
            &self.shared,
            mesh,
            provider,
            hash,
            expected_size,
            transfer::FetchCallbacks { progress, queued },
            &mut cancel,
        )
        .await
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
        let mut pending = self.shared.pending.lock().unwrap();
        let (id, was_unenrolled) = self.shared.update(|state| {
            let was_unenrolled = state.meshes.is_empty();
            ensure!(
                state.meshes.len() < storage::MAX_CURRENT_MESHES,
                NodeError::MeshLimit
            );
            let mesh = Mesh::create(name, state.member.clone(), &self.shared.storage.key)
                .map_err(|error| NodeError::Invalid(error.to_string()))?;
            let id = mesh.id;
            state.meshes.insert(id, mesh);
            Ok(((id, was_unenrolled), true))
        })?;
        if was_unenrolled {
            *pending = None;
        }
        Ok(id)
    }

    pub async fn pair(&self, lifetime: Duration) -> Result<String> {
        self.ensure_open()?;
        ensure!(
            (1..=3600).contains(&lifetime.as_secs()),
            NodeError::Invalid("ticket lifetime must be 1–3600 seconds".into())
        );
        if !self.shared.config.local {
            tokio::time::timeout(
                self.shared.config.request_timeout,
                self.shared.endpoint.online(),
            )
            .await
            .map_err(|_| {
                NodeError::Unavailable(
                    "relay is unavailable; use node serve --local for same-machine testing".into(),
                )
            })?;
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
        *self.shared.pending.lock().unwrap() = Some(ticket);
        Ok(encoded)
    }

    pub async fn add(&self, mesh_id: MeshId, ticket: &str) -> Result<Member> {
        self.ensure_open()?;
        ensure!(
            self.shared
                .state
                .lock()
                .unwrap()
                .meshes
                .contains_key(&mesh_id),
            NodeError::NotMember
        );
        let ticket = PairingTicket::decode(ticket)
            .map_err(|error| NodeError::TicketRejected(error.to_string()))?;
        ensure!(
            ticket.address.id != self.info().id,
            NodeError::TicketRejected("cannot enroll this device into itself".into())
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
        self.ensure_open()?;
        let (departure, peers) = {
            let mut pending = self.shared.pending.lock().unwrap();
            let (departure, peers) = self.shared.update(|state| {
                let peers: Vec<_> = state
                    .meshes
                    .get(&mesh_id)
                    .ok_or(NodeError::NotMember)?
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
        self.ensure_open()?;
        let info = self.info();
        let member = info
            .members
            .iter()
            .find(|m| m.id == id)
            .ok_or(NodeError::NotMember)?;
        ensure!(id != info.id, "choose another mesh member to ping");
        let elapsed_ms = self.shared.ping(id).await?;
        Ok(Pong {
            id,
            name: member.name.clone(),
            elapsed_ms,
        })
    }

    fn ensure_open(&self) -> std::result::Result<(), NodeError> {
        if self.closed.load(Ordering::Acquire) {
            Err(NodeError::NodeClosed)
        } else {
            Ok(())
        }
    }

    pub fn register_app_handler(&self, protocol: &str, handler: Arc<dyn AppHandler>) -> Result<()> {
        self.ensure_open()?;
        self.app.register(protocol, handler)
    }

    pub fn unregister_app_handler(&self, protocol: &str) -> Result<()> {
        self.ensure_open()?;
        self.app.unregister(protocol)
    }

    pub async fn app_request(
        &self,
        mesh: MeshId,
        peer: NodeId,
        protocol: &str,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>> {
        self.ensure_open()?;
        app::validate_app_name(protocol)?;
        ensure!(
            payload.len() <= MAX_APP_BYTES,
            NodeError::Invalid("app payload is too large".into())
        );
        ensure!(
            self.shared.both_current_members(mesh, peer),
            NodeError::NotMember
        );
        let request = AppRequest {
            mesh_id: mesh,
            protocol: protocol.into(),
            payload: URL_SAFE_NO_PAD.encode(payload),
        };
        let reply: std::result::Result<String, String> = self
            .shared
            .app_exchange(
                self.shared.address_for_mesh(peer, mesh),
                &request,
                MAX_APP_WIRE,
            )
            .await?;
        let encoded = reply.map_err(|_| NodeError::Unavailable("app unavailable".into()))?;
        ensure!(
            encoded.len() <= MAX_APP_ENCODED,
            NodeError::Invalid("app response is too large".into())
        );
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| NodeError::Invalid("invalid app response".into()))?;
        ensure!(
            bytes.len() <= MAX_APP_BYTES,
            NodeError::Invalid("app response is too large".into())
        );
        Ok(bytes)
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.shared.diagnostics.entries()
    }

    pub fn sign_app(&self, domain: &str, bytes: &[u8]) -> Result<String> {
        self.ensure_open()?;
        app::sign_app(&self.shared.storage.key, domain, bytes)
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        self.gossip.abort();
        self.router.shutdown().await?;
        Ok(())
    }
}

fn joined_after_retry(response: network::EnrollmentReply) -> Result<membership::Snapshot> {
    ensure!(response.departure.is_none(), NodeError::TicketRejected("device refused readmission twice; update the introducing device if it runs an older Spirit".into()));
    response
        .joined
        .map_err(|error| NodeError::TicketRejected(network::bounded_diagnostic(&error)).into())
}

impl Drop for Node {
    fn drop(&mut self) {
        self.gossip.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn operations_reject_a_closed_node() {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "device").unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        node.shutdown().await.unwrap();
        assert!(matches!(
            node.pair(Duration::from_secs(300))
                .await
                .unwrap_err()
                .downcast_ref::<NodeError>(),
            Some(NodeError::NodeClosed)
        ));
        assert!(matches!(
            node.ping(node.info().id)
                .await
                .unwrap_err()
                .downcast_ref::<NodeError>(),
            Some(NodeError::NodeClosed)
        ));
    }

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
