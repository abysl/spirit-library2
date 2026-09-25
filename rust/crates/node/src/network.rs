use crate::{
    membership::{Snapshot, VerifiedSnapshot},
    now,
    presence::{Presence, HEARTBEAT_INTERVAL},
    storage::{State, Storage},
    MeshId, NodeConfig, NodeId, PairingTicket,
};
use anyhow::{ensure, Context, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
#[cfg(test)]
use std::sync::OnceLock;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::task::JoinSet;

pub(crate) const PAIR_ALPN: &[u8] = b"spirit/pair/2";
pub(crate) const SYNC_ALPN: &[u8] = b"spirit/mesh/2";
pub(crate) const PING_ALPN: &[u8] = b"spirit/ping/1";
pub(crate) const DEPART_ALPN: &[u8] = b"spirit/depart/1";
const DEPARTURE_RECORDED: &str = "recorded";
const MAX_MESSAGE: usize = 1024 * 1024;
const HEARTBEAT_MARGIN: Duration = Duration::from_secs(1);
const HEARTBEAT_DEADLINE: Duration = HEARTBEAT_INTERVAL.saturating_sub(HEARTBEAT_MARGIN);
const PING_RESERVE: Duration = Duration::from_secs(1);
const HEARTBEAT_SYNC_BUDGET: Duration = HEARTBEAT_DEADLINE.saturating_sub(PING_RESERVE);
const _: () = assert!(
    HEARTBEAT_SYNC_BUDGET.as_nanos() + PING_RESERVE.as_nanos() <= HEARTBEAT_DEADLINE.as_nanos()
);
#[cfg(test)]
static TEST_HEARTBEAT_BUDGET: std::sync::OnceLock<Mutex<BTreeMap<NodeId, Duration>>> =
    std::sync::OnceLock::new();
#[cfg(test)]
static TEST_HEARTBEAT_PANIC: std::sync::OnceLock<Mutex<BTreeSet<NodeId>>> =
    std::sync::OnceLock::new();

fn heartbeat_budget(_local: NodeId) -> Duration {
    #[cfg(test)]
    if let Some(budget) = TEST_HEARTBEAT_BUDGET
        .get()
        .and_then(|values| values.lock().unwrap().get(&_local).copied())
    {
        return budget;
    }
    HEARTBEAT_SYNC_BUDGET
}

const MAX_DIAGNOSTIC_BYTES: usize = 256;

pub(crate) fn bounded_diagnostic(error: &str) -> String {
    let mut safe = String::new();
    for character in error.chars().filter(|character| {
        !character.is_control()
            && !matches!(
                unicode_general_category::get_general_category(*character),
                unicode_general_category::GeneralCategory::Format
            )
    }) {
        if safe.len() + character.len_utf8() > MAX_DIAGNOSTIC_BYTES {
            break;
        }
        safe.push(character);
    }
    safe
}
#[cfg(test)]
static TEST_PINGS: OnceLock<Mutex<BTreeMap<NodeId, usize>>> = OnceLock::new();
#[cfg(test)]
type TestDialLog = BTreeMap<NodeId, Vec<(MeshId, EndpointAddr)>>;
#[cfg(test)]
static TEST_DIALS: OnceLock<Mutex<TestDialLog>> = OnceLock::new();
#[cfg(test)]
static TEST_HEARTBEATS: OnceLock<Mutex<BTreeMap<NodeId, (usize, usize)>>> = OnceLock::new();

#[cfg(test)]
struct TestHeartbeat(NodeId);

#[cfg(test)]
impl TestHeartbeat {
    fn start(id: NodeId) -> Self {
        let mut counts = TEST_HEARTBEATS
            .get_or_init(Default::default)
            .lock()
            .unwrap();
        let count = counts.entry(id).or_default();
        count.0 += 1;
        count.1 = count.1.max(count.0);
        Self(id)
    }
}

#[cfg(test)]
impl Drop for TestHeartbeat {
    fn drop(&mut self) {
        TEST_HEARTBEATS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .get_mut(&self.0)
            .unwrap()
            .0 -= 1;
    }
}

#[cfg(test)]
fn test_ping_count(id: NodeId) -> usize {
    *TEST_PINGS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(&id)
        .unwrap_or(&0)
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub departure: Option<Snapshot>,
}

pub(crate) enum Enrolled {
    Joined(Snapshot),
    Departed(Snapshot),
}

impl Shared {
    pub fn heartbeat_received(&self, id: NodeId) {
        let state = self.state.lock().unwrap();
        if !state.meshes.values().any(|mesh| mesh.member(id).is_some()) {
            return;
        }
        self.presence
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .heartbeat_received(Instant::now());
    }

    pub fn is_member(&self, id: NodeId) -> bool {
        self.state
            .lock()
            .unwrap()
            .meshes
            .values()
            .any(|mesh| mesh.member(id).is_some())
    }

    fn failed(&self, id: NodeId, error: &anyhow::Error) {
        let state = self.state.lock().unwrap();
        if !state.meshes.values().any(|mesh| mesh.member(id).is_some()) {
            return;
        }
        self.presence
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .failed(bounded_diagnostic(&format!("{error:#}")));
    }

    fn record_failure<T>(&self, id: NodeId, result: &Result<T>) {
        if let Err(error) = result {
            self.failed(id, error);
        }
    }

    pub async fn ping(self: &Arc<Self>, id: NodeId) -> Result<u128> {
        self.ping_with_sync_budget(id, self.config.request_timeout)
            .await
    }

    async fn ping_with_sync_budget(self: &Arc<Self>, id: NodeId, budget: Duration) -> Result<u128> {
        #[cfg(test)]
        let _heartbeat = TestHeartbeat::start(id);
        let result = async {
            let address = self.address(id);
            let shared: Vec<_> = self
                .state
                .lock()
                .unwrap()
                .meshes
                .values()
                .filter(|mesh| mesh.member(id).is_some())
                .map(|mesh| mesh.id)
                .collect();
            ensure!(!shared.is_empty(), "peer is not a mesh member");
            let mut syncs = JoinSet::new();
            let mut pending = BTreeMap::new();
            for mesh_id in shared {
                let peer = self.clone();
                let mesh_address = self.address_for_mesh(id, mesh_id);
                #[cfg(test)]
                TEST_DIALS
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap()
                    .entry(self.endpoint.id())
                    .or_default()
                    .push((mesh_id, mesh_address.clone()));
                let task = syncs.spawn(async move { peer.sync(mesh_address, mesh_id).await });
                pending.insert(task.id(), mesh_id);
            }
            let mut errors = Vec::new();
            let sync_result = tokio::time::timeout(budget, async {
                while let Some(result) = syncs.join_next_with_id().await {
                    match result {
                        Ok((task_id, result)) => {
                            let mesh_id = pending.remove(&task_id).unwrap();
                            if let Err(error) = result {
                                errors.push(bounded_diagnostic(&format!("{mesh_id}: {error:#}")));
                            }
                        }
                        Err(error) => {
                            let mesh_id = pending.remove(&error.id()).unwrap();
                            errors.push(bounded_diagnostic(&format!("{mesh_id}: {error}")));
                        }
                    }
                }
            })
            .await;
            if sync_result.is_err() {
                syncs.abort_all();
                errors.extend(
                    pending.values().map(|id| {
                        bounded_diagnostic(&format!("{id}: membership exchange timed out"))
                    }),
                );
            }
            let start = Instant::now();
            let response: String = self.request(address, PING_ALPN, &"ping").await?;
            ensure!(response == "pong", "invalid pong response");
            self.heartbeat_received(id);
            if !errors.is_empty() {
                self.failed(id, &anyhow::anyhow!(errors.join("; ")));
            }
            Ok(start.elapsed().as_millis())
        }
        .await;
        if let Err(error) = &result {
            self.failed(id, error);
        }
        result
    }

    pub fn snapshot(&self, mesh_id: MeshId) -> Result<Snapshot> {
        self.state
            .lock()
            .unwrap()
            .snapshot(mesh_id, self.endpoint.addr())
    }

    pub fn address(&self, id: NodeId) -> EndpointAddr {
        self.state
            .lock()
            .unwrap()
            .addresses
            .get(&id)
            .and_then(|entry| entry.dial().cloned())
            .unwrap_or_else(|| id.into())
    }

    fn address_for_mesh(&self, id: NodeId, mesh_id: MeshId) -> EndpointAddr {
        self.state
            .lock()
            .unwrap()
            .addresses
            .get(&id)
            .and_then(|entry| entry.preferred(mesh_id).cloned())
            .unwrap_or_else(|| id.into())
    }

    pub fn merge_current(&self, snapshot: &Snapshot, source: NodeId) -> Result<()> {
        let verified = VerifiedSnapshot::new(snapshot)?;
        self.merge_verified_snapshot(&verified, source, true)
    }

    fn merge_verified_snapshot(
        &self,
        verified: &VerifiedSnapshot<'_>,
        source: NodeId,
        current_only: bool,
    ) -> Result<()> {
        let snapshot = verified.snapshot();
        ensure!(
            snapshot.mesh.member(source).is_some(),
            "peer is not a mesh member"
        );
        self.update(|state| {
            let changed = if current_only {
                state.merge_current(verified, source)?
            } else {
                state.join(verified, source)?
            };
            ensure!(
                !current_only || state.meshes[&snapshot.mesh.id].member(source).is_some(),
                "mesh unavailable"
            );
            Ok(((), changed))
        })
    }

    pub fn record_departure(&self, snapshot: &Snapshot, source: NodeId) -> Result<()> {
        let verified = VerifiedSnapshot::new(snapshot)?;
        let snapshot = verified.snapshot();
        ensure!(
            snapshot.mesh.departed(source),
            "peer has not left this mesh"
        );
        self.update(|state| Ok(((), state.merge_departure(&verified, source)?)))
    }

    pub fn update<T>(&self, change: impl FnOnce(&mut State) -> Result<(T, bool)>) -> Result<T> {
        let mut state = self.state.lock().unwrap();
        let mut next = state.clone();
        let (result, changed) = change(&mut next)?;
        if changed {
            self.storage.save(&next)?;
            *state = next;
        }
        let members: BTreeSet<_> = state
            .meshes
            .values()
            .flat_map(|mesh| mesh.members().map(|member| member.id))
            .collect();
        self.presence
            .lock()
            .unwrap()
            .retain(|id, _| members.contains(id));
        Ok(result)
    }

    pub async fn announce_departure(
        self: &Arc<Self>,
        departure: Snapshot,
        peers: Vec<EndpointAddr>,
    ) -> usize {
        let mut tasks = JoinSet::new();
        for address in peers {
            let shared = self.clone();
            let departure = departure.clone();
            tasks.spawn(async move {
                shared
                    .request::<_, String>(address, DEPART_ALPN, &departure)
                    .await
                    .is_ok_and(|reply| reply == DEPARTURE_RECORDED)
            });
        }
        let mut notified = 0;
        let _ = tokio::time::timeout(self.config.request_timeout, async {
            while let Some(result) = tasks.join_next().await {
                if result.unwrap_or(false) {
                    notified += 1;
                }
            }
        })
        .await;
        notified
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
            serde_json::from_slice(&response).map_err(|_| anyhow::anyhow!("invalid response"))
        })
        .await
        .context("request timed out");
        connection.close(0u32.into(), b"done");
        result?
    }

    pub async fn sync(&self, address: EndpointAddr, mesh_id: MeshId) -> Result<()> {
        let id = address.id;
        let result = async {
            let snapshot = self.snapshot(mesh_id)?;
            let response: Snapshot = self.request(address, SYNC_ALPN, &snapshot).await?;
            ensure!(
                response.mesh.id == mesh_id,
                "membership response names a different mesh"
            );
            if response.mesh.departed(id) {
                self.record_departure(&response, id)
            } else {
                self.merge_current(&response, id)
            }
        }
        .await;
        self.record_failure(id, &result);
        result
    }

    fn enroll(&self, request: Enrollment, remote: NodeId) -> Result<Enrolled> {
        let verified = VerifiedSnapshot::new(&request.snapshot)?;
        let mut pending = self.pending.lock().unwrap();
        let ticket = pending
            .as_ref()
            .context("pairing ticket is unavailable or already consumed")?;
        ensure!(
            bool::from(ticket.secret.ct_eq(&request.secret)),
            "invalid pairing secret"
        );
        ensure!(ticket.expires_at > now()?, "pairing ticket has expired");
        ensure!(
            verified.snapshot().mesh.member(remote).is_some(),
            "introducer is not a mesh member"
        );
        if let Some(departure) = self.state.lock().unwrap().rejoin_conflict(&verified)? {
            return Ok(Enrolled::Departed(departure));
        }
        let mesh_id = request.snapshot.mesh.id;
        self.merge_verified_snapshot(&verified, remote, false)?;
        *pending = None;
        Ok(Enrolled::Joined(self.snapshot(mesh_id)?))
    }

    fn answer_sync(&self, snapshot: &Snapshot, remote: NodeId) -> Result<Snapshot> {
        ensure!(snapshot.mesh.member(remote).is_some(), "mesh unavailable");
        let verified =
            VerifiedSnapshot::new(snapshot).map_err(|_| anyhow::anyhow!("mesh unavailable"))?;
        let mesh_id = snapshot.mesh.id;
        let departure = {
            let state = self.state.lock().unwrap();
            if let Some(departure) = state.departure(mesh_id) {
                ensure!(
                    departure.mesh.same_identity(&snapshot.mesh),
                    "mesh unavailable"
                );
                Some(departure)
            } else {
                None
            }
        };
        if let Some(departure) = departure {
            return Ok(departure);
        }
        if self
            .state
            .lock()
            .unwrap()
            .meshes
            .get(&mesh_id)
            .is_some_and(|mesh| mesh.records_departure_at(&snapshot.mesh, remote))
        {
            anyhow::bail!("mesh unavailable");
        }
        self.merge_verified_snapshot(&verified, remote, true)
            .map_err(|_| anyhow::anyhow!("mesh unavailable"))?;
        self.snapshot(mesh_id)
            .map_err(|_| anyhow::anyhow!("mesh unavailable"))
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Pair,
    Sync,
    Ping,
    Depart,
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
    pub fn depart(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            kind: Kind::Depart,
        }
    }

    async fn respond(&self, connection: &Connection) -> Result<()> {
        let remote = connection.remote_id();
        if matches!(self.kind, Kind::Ping) {
            ensure!(self.shared.is_member(remote), "peer is not a mesh member");
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
                serde_json::to_vec(&match self.shared.enroll(request, remote) {
                    Ok(Enrolled::Joined(snapshot)) => EnrollmentReply {
                        joined: Ok(snapshot),
                        departure: None,
                    },
                    Ok(Enrolled::Departed(departure)) => EnrollmentReply {
                        joined: Err("could not join this mesh".into()),
                        departure: Some(departure),
                    },
                    Err(error) => {
                        self.shared.failed(remote, &error);
                        EnrollmentReply {
                            joined: Err("could not join this mesh".into()),
                            departure: None,
                        }
                    }
                })?
            }
            Kind::Sync => {
                let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
                serde_json::to_vec(&self.shared.answer_sync(&snapshot, remote)?)?
            }
            Kind::Depart => {
                let departure: Snapshot = serde_json::from_slice(&bytes)?;
                self.shared.record_departure(&departure, remote)?;
                serde_json::to_vec(DEPARTURE_RECORDED)?
            }
            Kind::Ping => {
                let request: String = serde_json::from_slice(&bytes)?;
                ensure!(request == "ping", "invalid ping");
                self.shared.heartbeat_received(remote);
                #[cfg(test)]
                {
                    *TEST_PINGS
                        .get_or_init(Default::default)
                        .lock()
                        .unwrap()
                        .entry(self.shared.endpoint.id())
                        .or_default() += 1;
                }
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
    let mut task_peers = BTreeMap::new();
    loop {
        tokio::select! {
            _ = interval.tick() => {
                let peers: Vec<_> = {
                    let state = shared.state.lock().unwrap();
                    state.meshes.values().flat_map(|mesh| mesh.members().map(|member| member.id))
                        .filter(|id| *id != state.member.id).collect::<BTreeSet<_>>().into_iter().collect()
                };
                for id in peers {
                    if !in_flight.insert(id) { continue; }
                    let shared = shared.clone();
                    let task = tasks.spawn(async move {
                        #[cfg(test)]
                        if TEST_HEARTBEAT_PANIC.get().is_some_and(|ids| ids.lock().unwrap().remove(&id)) {
                            panic!("injected heartbeat panic");
                        }
                        let result = tokio::time::timeout(HEARTBEAT_DEADLINE, shared.ping_with_sync_budget(id, heartbeat_budget(shared.endpoint.id())))
                            .await.context("heartbeat timed out").and_then(|result| result);
                        if let Err(error) = result { shared.failed(id, &error); }
                    });
                    task_peers.insert(task.id(), id);
                }
            }
            Some(result) = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let task_id = match result {
                    Ok((task_id, ())) => task_id,
                    Err(error) => error.id(),
                };
                if let Some(id) = task_peers.remove(&task_id) {
                    in_flight.remove(&id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mesh, Node, PairingTicket};
    use iroh::SecretKey;

    async fn device(name: &str) -> (tempfile::TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), name).unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        (dir, node)
    }

    #[derive(Debug)]
    struct FakeResponder {
        response: Vec<u8>,
        hang: bool,
    }

    impl ProtocolHandler for FakeResponder {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let result: Result<()> = async {
                let (mut send, mut recv) = connection.accept_bi().await?;
                recv.read_to_end(MAX_MESSAGE).await?;
                if self.hang {
                    std::future::pending::<()>().await;
                }
                send.write_all(&self.response).await?;
                send.finish()?;
                connection.closed().await;
                Ok(())
            }
            .await;
            result.map_err(|error| AcceptError::from_boxed(error.into()))
        }
    }

    async fn fake_endpoint(
        key: SecretKey,
        alpn: &[u8],
        response: Vec<u8>,
        hang: bool,
    ) -> (Endpoint, iroh::protocol::Router) {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .secret_key(key)
            .bind()
            .await
            .unwrap();
        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(alpn, FakeResponder { response, hang })
            .spawn();
        (endpoint, router)
    }

    #[tokio::test]
    async fn huge_peer_response_never_enters_presence_diagnostics() {
        let (_a_dir, a) = device("a").await;
        a.gossip.abort();
        let key = SecretKey::generate();
        let mesh = a.new_mesh("mesh").unwrap();
        a.shared
            .update(|state| {
                state.meshes.get_mut(&mesh).unwrap().admit(
                    crate::Member {
                        id: key.public(),
                        name: "peer".into(),
                    },
                    &a.shared.storage.key,
                )?;
                Ok(((), true))
            })
            .unwrap();
        let id = key.public();
        let (endpoint, router) = fake_endpoint(
            key,
            SYNC_ALPN,
            serde_json::to_vec(&"x".repeat(900_000)).unwrap(),
            false,
        )
        .await;
        a.shared.sync(endpoint.addr(), mesh).await.unwrap_err();
        let error = a.peers()[0].last_error.clone().unwrap();
        assert!(error.contains("invalid response"));
        assert!(error.len() <= MAX_DIAGNOSTIC_BYTES);
        a.shared.failed(
            id,
            &anyhow::anyhow!(format!("{}\n\t{}", "界".repeat(200), "x".repeat(900_000))),
        );
        let error = a.peers()[0].last_error.clone().unwrap();
        assert!(error.len() <= MAX_DIAGNOSTIC_BYTES);
        assert!(!error.chars().any(char::is_control));
        assert_eq!(bounded_diagnostic("safe\u{202e}\u{200b}text"), "safetext");
        router.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn refused_pairing_records_a_known_peers_local_error() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let mesh_id = a.new_mesh("shared").unwrap();
        a.add(mesh_id, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        b.pair(Duration::from_secs(60)).await.unwrap();
        let invalid = Enrollment {
            secret: [0; 32],
            snapshot: a.shared.snapshot(mesh_id).unwrap(),
        };
        let reply: EnrollmentReply = a
            .shared
            .request(b.shared.endpoint.addr(), PAIR_ALPN, &invalid)
            .await
            .unwrap();
        assert_eq!(reply.joined.unwrap_err(), "could not join this mesh");
        assert!(b.peers()[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("invalid pairing secret"));
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn pairing_reply_from_another_mesh_cannot_enroll_the_introducer() {
        let (_dir, a) = device("a").await;
        a.gossip.abort();
        let target = a.new_mesh("target").unwrap();
        let fake_key = SecretKey::generate();
        let mut attacker = Mesh::create(
            "attacker",
            crate::Member {
                id: fake_key.public(),
                name: "joiner".into(),
            },
            &fake_key,
        )
        .unwrap();
        attacker
            .admit(a.shared.state.lock().unwrap().member.clone(), &fake_key)
            .unwrap();
        let reply = EnrollmentReply {
            joined: Ok(Snapshot::without_addresses(attacker.clone())),
            departure: None,
        };
        let (endpoint, router) = fake_endpoint(
            fake_key,
            PAIR_ALPN,
            serde_json::to_vec(&reply).unwrap(),
            false,
        )
        .await;
        let ticket = PairingTicket {
            address: endpoint.addr(),
            name: "joiner".into(),
            secret: [7; 32],
            expires_at: now().unwrap() + 60,
        }
        .encode()
        .unwrap();
        assert!(a
            .add(target, &ticket)
            .await
            .unwrap_err()
            .to_string()
            .contains("different mesh"));
        assert_eq!(a.info().meshes.len(), 1);
        assert!(!a
            .shared
            .state
            .lock()
            .unwrap()
            .meshes
            .contains_key(&attacker.id));
        router.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn sync_reply_cannot_create_a_mesh_for_the_requester() {
        let (_dir, a) = device("a").await;
        a.gossip.abort();
        let fake_key = SecretKey::generate();
        let current = a.new_mesh("current").unwrap();
        a.shared
            .update(|state| {
                state.meshes.get_mut(&current).unwrap().admit(
                    crate::Member {
                        id: fake_key.public(),
                        name: "peer".into(),
                    },
                    &a.shared.storage.key,
                )?;
                Ok(((), true))
            })
            .unwrap();
        let mut mesh = Mesh::create(
            "remote",
            crate::Member {
                id: fake_key.public(),
                name: "peer".into(),
            },
            &fake_key,
        )
        .unwrap();
        mesh.admit(a.shared.state.lock().unwrap().member.clone(), &fake_key)
            .unwrap();
        let snapshot = Snapshot::without_addresses(mesh.clone());
        assert!(a
            .shared
            .merge_current(&snapshot, fake_key.public())
            .is_err());
        let (endpoint, router) = fake_endpoint(
            fake_key,
            SYNC_ALPN,
            serde_json::to_vec(&snapshot).unwrap(),
            false,
        )
        .await;
        assert!(a.shared.sync(endpoint.addr(), current).await.is_err());
        assert_eq!(a.info().meshes.len(), 1);
        assert!(a.shared.state.lock().unwrap().meshes.contains_key(&current));
        assert!(!a.shared.state.lock().unwrap().meshes.contains_key(&mesh.id));
        router.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn departed_copy_requires_a_verified_member_of_the_same_founder() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        let id = a.new_mesh("secret mesh").unwrap();
        a.add(id, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        b.leave(id).await.unwrap();
        let forged = Mesh::create_with_id(
            id,
            "decoy",
            c.shared.state.lock().unwrap().member.clone(),
            &c.shared.storage.key,
        )
        .unwrap();
        let forged = Snapshot::without_addresses(forged);
        let unknown = Snapshot::without_addresses(
            Mesh::create(
                "unknown",
                c.shared.state.lock().unwrap().member.clone(),
                &c.shared.storage.key,
            )
            .unwrap(),
        );
        let unknown_error = b
            .shared
            .answer_sync(&unknown, c.info().id)
            .unwrap_err()
            .to_string();
        assert_eq!(
            b.shared
                .answer_sync(&forged, c.info().id)
                .unwrap_err()
                .to_string(),
            unknown_error
        );
        let reply: Result<Snapshot> = c
            .shared
            .request(b.shared.endpoint.addr(), SYNC_ALPN, &forged)
            .await;
        let unknown_reply: Result<Snapshot> = c
            .shared
            .request(b.shared.endpoint.addr(), SYNC_ALPN, &unknown)
            .await;
        assert_eq!(
            reply.unwrap_err().to_string(),
            unknown_reply.unwrap_err().to_string()
        );
        let mut invalid = forged.clone();
        invalid.mesh.admissions[0].member.name = "tampered".into();
        assert_eq!(
            b.shared
                .answer_sync(&invalid, c.info().id)
                .unwrap_err()
                .to_string(),
            unknown_error
        );
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn version_one_pairing_and_membership_are_not_served() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let mesh_id = a.new_mesh("new").unwrap();
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        let snapshot = a.shared.snapshot(mesh_id).unwrap();
        let enrollment = Enrollment {
            secret: PairingTicket::decode(&ticket).unwrap().secret,
            snapshot: snapshot.clone(),
        };
        let old_pair: Result<EnrollmentReply> = a
            .shared
            .request(b.shared.endpoint.addr(), b"spirit/pair/1", &enrollment)
            .await;
        let old_sync: Result<Snapshot> = a
            .shared
            .request(b.shared.endpoint.addr(), b"spirit/mesh/1", &snapshot)
            .await;
        assert!(old_pair.is_err());
        assert!(old_sync.is_err());
        assert_eq!(PAIR_ALPN, b"spirit/pair/2");
        assert_eq!(SYNC_ALPN, b"spirit/mesh/2");
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn overloaded_host_address_pairs_and_syncs_after_bounding() {
        use iroh::{RelayUrl, TransportAddr};
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        a.new_mesh("M1").unwrap();
        let mut overloaded = b.shared.endpoint.addr();
        overloaded.addrs.insert(TransportAddr::Relay(
            format!("https://relay.example/{}", "x".repeat(200))
                .parse::<RelayUrl>()
                .unwrap(),
        ));
        for index in 0..20 {
            overloaded.addrs.insert(TransportAddr::Ip(
                format!("[::1]:{}", 30000 + index).parse().unwrap(),
            ));
        }
        assert!(overloaded.addrs.len() > 16);
        crate::TEST_PAIR_ADDRESSES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(b.info().id, overloaded.clone());
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        assert_eq!(
            PairingTicket::decode(&ticket).unwrap().address.addrs.len(),
            16
        );
        a.add(a.info().mesh_id.unwrap(), &ticket).await.unwrap();
        let b_mesh = b.info().mesh_id.unwrap();
        let snapshot = b
            .shared
            .state
            .lock()
            .unwrap()
            .snapshot(b_mesh, overloaded)
            .unwrap();
        assert_eq!(snapshot.addresses[&b.info().id].addrs.len(), 16);
        let reply: Snapshot = b
            .shared
            .request(a.shared.endpoint.addr(), SYNC_ALPN, &snapshot)
            .await
            .unwrap();
        reply.verify().unwrap();
        b.shared
            .sync(a.shared.endpoint.addr(), b.info().mesh_id.unwrap())
            .await
            .unwrap();
        a.ping(b.info().id).await.unwrap();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_panicked_heartbeat_releases_the_peers_in_flight_slot() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let mesh = a.new_mesh("mesh").unwrap();
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        a.add(mesh, &ticket).await.unwrap();
        let panics = TEST_HEARTBEAT_PANIC.get_or_init(Default::default);
        panics.lock().unwrap().insert(b.info().id);
        let heartbeat = tokio::spawn(gossip(a.shared.clone()));
        tokio::time::timeout(Duration::from_secs(8), async {
            while !a.peers()[0].connected {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(!panics.lock().unwrap().contains(&b.info().id));
        heartbeat.abort();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn hanging_mesh_sync_expires_before_the_ping_reserve() {
        let (_dir, a) = device("a").await;
        a.gossip.abort();
        let fake_key = SecretKey::generate();
        let fake_id = fake_key.public();
        let id = a.new_mesh("with hanging peer").unwrap();
        a.shared
            .update(|state| {
                state.meshes.get_mut(&id).unwrap().admit(
                    crate::Member {
                        id: fake_id,
                        name: "peer".into(),
                    },
                    &a.shared.storage.key,
                )?;
                Ok(((), true))
            })
            .unwrap();
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .secret_key(fake_key)
            .bind()
            .await
            .unwrap();
        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(
                SYNC_ALPN,
                FakeResponder {
                    response: vec![],
                    hang: true,
                },
            )
            .accept(
                PING_ALPN,
                FakeResponder {
                    response: serde_json::to_vec("pong").unwrap(),
                    hang: false,
                },
            )
            .spawn();
        a.shared.state.lock().unwrap().addresses.insert(
            fake_id,
            crate::storage::AddressEntry::Current {
                direct: Some(endpoint.addr()),
                hints: BTreeMap::new(),
            },
        );
        TEST_HEARTBEAT_BUDGET
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(a.info().id, Duration::from_millis(200));
        let start = Instant::now();
        let heartbeat = tokio::spawn(gossip(a.shared.clone()));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !a.peers()[0].connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        heartbeat.abort();
        let peer = &a.peers()[0];
        assert!(peer.connected);
        let error = peer.last_error.as_deref().unwrap();
        assert!(error.contains(&id.to_string()) && error.contains("timed out"));
        router.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_permanently_failing_mesh_does_not_block_sync_or_one_ping_per_interval() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let first = a.new_mesh("first").unwrap();
        a.add(first, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let second = a.new_mesh("second").unwrap();
        a.add(second, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let conflict = SecretKey::generate();
        let new_member = SecretKey::generate();
        a.shared
            .update(|state| {
                state.meshes.get_mut(&second).unwrap().admit(
                    crate::Member {
                        id: conflict.public(),
                        name: "one".into(),
                    },
                    &a.shared.storage.key,
                )?;
                Ok(((), true))
            })
            .unwrap();
        b.shared
            .update(|state| {
                state.meshes.get_mut(&second).unwrap().admit(
                    crate::Member {
                        id: conflict.public(),
                        name: "two".into(),
                    },
                    &b.shared.storage.key,
                )?;
                state.meshes.get_mut(&first).unwrap().admit(
                    crate::Member {
                        id: new_member.public(),
                        name: "new".into(),
                    },
                    &b.shared.storage.key,
                )?;
                Ok(((), true))
            })
            .unwrap();
        let start = Instant::now();
        let heartbeat = tokio::spawn(gossip(a.shared.clone()));
        tokio::time::timeout(Duration::from_secs(3), async {
            while test_ping_count(b.info().id) < 1
                || TEST_HEARTBEATS.get().unwrap().lock().unwrap()[&b.info().id].0 != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let first_round = start.elapsed();
        tokio::time::timeout(Duration::from_secs(13), async {
            while test_ping_count(b.info().id) < 3 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(test_ping_count(b.info().id), 3);
        assert!(a
            .info()
            .meshes
            .iter()
            .find(|mesh| mesh.id == first)
            .unwrap()
            .members
            .iter()
            .any(|member| member.id == new_member.public()));
        tokio::time::timeout(Duration::from_secs(1), async {
            while TEST_HEARTBEATS.get().unwrap().lock().unwrap()[&b.info().id].0 != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(a
            .peers()
            .iter()
            .find(|peer| peer.id == b.info().id)
            .unwrap()
            .last_error
            .is_some());
        assert!(
            a.peers()
                .iter()
                .find(|peer| peer.id == b.info().id)
                .unwrap()
                .connected
        );
        assert_eq!(
            TEST_HEARTBEATS.get().unwrap().lock().unwrap()[&b.info().id].1,
            1
        );
        println!(
            "first heartbeat round with one permanently failing mesh: {first_round:?}; three intervals: {:?}",
            start.elapsed()
        );
        heartbeat.abort();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn two_meshes_isolate_membership_and_preserve_the_other_on_leave() {
        let (a_dir, a) = device("a").await;
        let (b_dir, b) = device("b").await;
        let (c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        let m1 = a.new_mesh("M1").unwrap();
        a.add(m1, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let m2 = b.new_mesh("M2").unwrap();
        b.add(m2, &c.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let a_id = a.info().id;
        let b_id = b.info().id;
        let c_id = c.info().id;
        assert_eq!(a.info().meshes[0].members.len(), 2);
        assert_eq!(c.info().meshes[0].members.len(), 2);
        assert_eq!(b.info().meshes.len(), 2);
        assert_eq!(b.peers().len(), 2);
        assert!(!b.shared.snapshot(m1).unwrap().addresses.contains_key(&c_id));
        assert!(!std::fs::read_to_string(a_dir.path().join("state.json"))
            .unwrap()
            .contains(&c_id.to_string()));
        assert!(!std::fs::read_to_string(a_dir.path().join("state.json"))
            .unwrap()
            .contains(&m2.to_string()));
        assert!(!std::fs::read_to_string(c_dir.path().join("state.json"))
            .unwrap()
            .contains(&a_id.to_string()));
        assert!(!std::fs::read_to_string(c_dir.path().join("state.json"))
            .unwrap()
            .contains(&m1.to_string()));
        let refused_ping: Result<String> = a
            .shared
            .request(c.shared.endpoint.addr(), PING_ALPN, &"ping")
            .await;
        assert!(refused_ping.is_err());
        let request = a.shared.snapshot(m1).unwrap();
        let unknown = Snapshot {
            mesh: Mesh {
                id: MeshId::generate().unwrap(),
                ..request.mesh.clone()
            },
            addresses: request.addresses.clone(),
        };
        assert_eq!(
            b.shared
                .answer_sync(&request, c_id)
                .unwrap_err()
                .to_string(),
            b.shared
                .answer_sync(&unknown, c_id)
                .unwrap_err()
                .to_string()
        );
        let wire: Result<Snapshot> = c
            .shared
            .request(b.shared.endpoint.addr(), SYNC_ALPN, &request)
            .await;
        assert!(wire.is_err());
        let second: Result<Snapshot> = c
            .shared
            .request(b.shared.endpoint.addr(), SYNC_ALPN, &unknown)
            .await;
        assert_eq!(
            wire.unwrap_err().to_string(),
            second.unwrap_err().to_string()
        );
        b.ping(c_id).await.unwrap();
        assert!(
            b.peers()
                .iter()
                .find(|peer| peer.id == c_id)
                .unwrap()
                .connected
        );
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        b.leave(m1).await.unwrap();
        assert!(b.shared.pending.lock().unwrap().is_none());
        assert!(a.add(m1, &ticket).await.is_err());
        assert_eq!(b.info().meshes.len(), 1);
        assert_eq!(b.info().meshes[0].id, m2);
        assert!(b.shared.state.lock().unwrap().departed.contains_key(&m1));
        assert!(
            b.peers()
                .iter()
                .find(|peer| peer.id == c_id)
                .unwrap()
                .connected
        );
        assert!(b.shared.state.lock().unwrap().addresses.contains_key(&c_id));
        let departure = b.shared.state.lock().unwrap().departure(m1).unwrap();
        let rejected: Result<Snapshot> = b
            .shared
            .request(a.shared.endpoint.addr(), SYNC_ALPN, &departure)
            .await;
        assert!(rejected.is_err());
        a.add(m1, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let b_info = b.info();
        assert_eq!(b_info.meshes.len(), 2);
        assert_eq!(
            b_info
                .meshes
                .iter()
                .find(|mesh| mesh.id == m1)
                .unwrap()
                .members
                .iter()
                .find(|member| member.id == b_id)
                .unwrap()
                .generation,
            1
        );
        assert!(!b.shared.state.lock().unwrap().departed.contains_key(&m1));
        assert!(b.shared.state.lock().unwrap().addresses.contains_key(&c_id));
        a.add(m1, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        assert_eq!(b.info().meshes.len(), 2);
        assert_eq!(
            b.info()
                .meshes
                .iter()
                .find(|mesh| mesh.id == m1)
                .unwrap()
                .members
                .iter()
                .find(|member| member.id == b_id)
                .unwrap()
                .generation,
            1
        );
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
        drop((a_dir, b_dir, c_dir));
    }

    #[tokio::test]
    async fn enrollment_into_a_sixty_fifth_mesh_is_rejected_without_consuming_ticket() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        for _ in 0..64 {
            a.new_mesh("full").unwrap();
        }
        let mesh_id = b.new_mesh("extra").unwrap();
        let ticket = a.pair(Duration::from_secs(60)).await.unwrap();
        assert!(b
            .add(mesh_id, &ticket)
            .await
            .unwrap_err()
            .to_string()
            .contains("could not join this mesh"));
        assert_eq!(a.info().meshes.len(), 64);
        assert!(a.shared.pending.lock().unwrap().is_some());
        b.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shared_peer_gets_one_heartbeat_after_two_mesh_syncs() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let first = a.new_mesh("one").unwrap();
        a.add(first, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let second = a.new_mesh("two").unwrap();
        a.add(second, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        assert_eq!(a.peers().len(), 1);
        let start = Instant::now();
        let heartbeat = tokio::spawn(gossip(a.shared.clone()));
        tokio::time::timeout(Duration::from_secs(8), async {
            while !a.peers().first().is_some_and(|peer| peer.connected) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(test_ping_count(b.info().id), 1);
        assert_eq!(
            TEST_HEARTBEATS.get().unwrap().lock().unwrap()[&b.info().id].1,
            1
        );
        assert_eq!(a.peers().len(), 1);
        assert!(a.peers()[0].connected);
        println!("two-mesh heartbeat round: {:?}", start.elapsed());
        heartbeat.abort();
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn creating_first_mesh_and_incoming_enrollment_serialize_on_pending() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let mesh = a.new_mesh("remote").unwrap();
        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        let b = Arc::new(b);
        let pending = b.shared.pending.lock().unwrap();
        let creator = b.clone();
        let create = tokio::task::spawn_blocking(move || creator.create_first_mesh("local"));
        std::thread::sleep(Duration::from_millis(40));
        assert!(b.info().meshes.is_empty());
        let a = Arc::new(a);
        let introducer = a.clone();
        let enroll = tokio::spawn(async move { introducer.add(mesh, &ticket).await });
        drop(pending);
        let created = create.await.unwrap().is_ok();
        let joined = enroll.await.unwrap().is_ok();
        assert_ne!(created, joined);
        assert_eq!(b.info().meshes.len(), 1);
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn per_mesh_sync_dials_only_its_own_hint_or_a_direct_address() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        let first = a.new_mesh("first").unwrap();
        a.add(first, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let second = a.new_mesh("second").unwrap();
        a.add(second, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        let id = b.info().id;
        let planted = EndpointAddr::new(id).with_ip_addr("127.0.0.1:9".parse().unwrap());
        a.shared.state.lock().unwrap().addresses.insert(
            id,
            crate::storage::AddressEntry::Current {
                direct: None,
                hints: BTreeMap::from([
                    (first, planted.clone()),
                    (second, b.shared.endpoint.addr()),
                ]),
            },
        );
        let local = a.info().id;
        TEST_DIALS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&local);
        a.shared
            .ping_with_sync_budget(id, Duration::from_secs(2))
            .await
            .unwrap();
        let dials = TEST_DIALS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&local)
            .unwrap();
        assert!(dials.contains(&(first, planted)));
        assert!(dials.contains(&(second, b.shared.endpoint.addr())));
        assert_eq!(
            a.shared.address_for_mesh(id, first),
            b.shared.endpoint.addr()
        );
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
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
        assert!(b
            .shared
            .sync(a.shared.endpoint.addr(), b.info().only_mesh().unwrap())
            .await
            .is_err());
        assert_eq!(a.info().members.len(), 1);
        let (_c_dir, c) = device("c").await;
        let valid = c.pair(Duration::from_secs(60)).await.unwrap();
        let mut forged = PairingTicket::decode(&valid).unwrap();
        forged.secret[0] ^= 1;
        assert!(a
            .add(a.info().only_mesh().unwrap(), &forged.encode().unwrap())
            .await
            .is_err());
        assert!(c.info().mesh_id.is_none());
        let replacement = c.pair(Duration::from_secs(60)).await.unwrap();
        assert!(a.add(a.info().only_mesh().unwrap(), &valid).await.is_err());
        a.add(a.info().only_mesh().unwrap(), &replacement)
            .await
            .unwrap();
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
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        assert!(!a.peers()[0].connected);
        assert!(a.peers()[0].last_received_ago_ms.is_none());
        assert!(!b.peers()[0].connected);
        assert!(b.peers()[0].last_received_ago_ms.is_none());

        a.shared
            .sync(b.shared.endpoint.addr(), a.info().only_mesh().unwrap())
            .await
            .unwrap();
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
            .add(a.info().only_mesh().unwrap(), &ticket)
            .await
            .unwrap_err()
            .to_string()
            .contains("could not join this mesh"));
        assert!(b.info().mesh_id.is_none());
        let mut snapshot = a.shared.snapshot(a.info().only_mesh().unwrap()).unwrap();
        snapshot
            .mesh
            .admit(
                b.shared.state.lock().unwrap().member.clone(),
                &a.shared.storage.key,
            )
            .unwrap();
        let local_error = b
            .shared
            .enroll(
                Enrollment {
                    secret: PairingTicket::decode(&ticket).unwrap().secret,
                    snapshot,
                },
                a.info().id,
            )
            .err()
            .unwrap();
        assert!(local_error
            .to_string()
            .contains("pairing ticket has expired"));
        b.shutdown().await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_stale_member_learns_the_departure_while_readmitting_the_device() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &c.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let m1 = a.info().only_mesh().unwrap();
        let b_id = b.info().id;
        assert!(c.info().members.iter().any(|member| member.id == b_id));
        c.shutdown().await.unwrap();
        drop(c);

        assert_eq!(
            b.leave(b.info().only_mesh().unwrap())
                .await
                .unwrap()
                .notified_members,
            1
        );
        let m2 = b.new_mesh("other").unwrap();
        assert!(b.shared.state.lock().unwrap().departed.contains_key(&m1));
        let c = Node::bind(c_dir.path(), NodeConfig::local()).await.unwrap();
        c.gossip.abort();
        assert!(c.info().members.iter().any(|member| member.id == b_id));

        let sync: Result<Snapshot> = c
            .shared
            .request(
                b.shared.endpoint.addr(),
                SYNC_ALPN,
                &c.shared.snapshot(c.info().only_mesh().unwrap()).unwrap(),
            )
            .await;
        assert!(sync.unwrap().mesh.departed(b_id));

        let ticket = b.pair(Duration::from_secs(60)).await.unwrap();
        let mut stale = c.shared.snapshot(c.info().only_mesh().unwrap()).unwrap();
        stale
            .mesh
            .admit(
                b.shared.state.lock().unwrap().member.clone(),
                &c.shared.storage.key,
            )
            .unwrap();
        let refusal: EnrollmentReply = c
            .shared
            .request(
                b.shared.endpoint.addr(),
                PAIR_ALPN,
                &Enrollment {
                    secret: PairingTicket::decode(&ticket).unwrap().secret,
                    snapshot: stale,
                },
            )
            .await
            .unwrap();
        assert_eq!(refusal.joined.unwrap_err(), "could not join this mesh");
        assert!(refusal.departure.unwrap().mesh.departed(b_id));
        assert!(b.shared.pending.lock().unwrap().is_some());
        c.add(c.info().only_mesh().unwrap(), &ticket).await.unwrap();
        let joined = b
            .shared
            .state
            .lock()
            .unwrap()
            .meshes
            .get(&m1)
            .cloned()
            .unwrap();
        assert!(joined
            .admissions
            .iter()
            .any(|admission| admission.member.id == b_id
                && admission.generation == 1
                && admission.issuer == c.info().id));
        assert_eq!(b.info().meshes.len(), 2);
        assert!(b.info().meshes.iter().any(|mesh| mesh.id == m2));
        assert!(b.shared.state.lock().unwrap().departed.is_empty());
        assert_eq!(c.ping(b_id).await.unwrap().id, b_id);
        a.shared
            .sync(c.shared.endpoint.addr(), a.info().only_mesh().unwrap())
            .await
            .unwrap();
        assert!(a.info().members.iter().any(|member| member.id == b_id));
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn departed_copy_is_not_sent_to_a_self_founded_mesh_reusing_its_id() {
        let (_a_dir, a) = device("founder-private-unique").await;
        let (_b_dir, b) = device("departed-private-unique").await;
        let (_c_dir, c) = device("c outsider nickname").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        let mesh_id = a.new_mesh("private mesh name").unwrap();
        a.add(mesh_id, &b.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        b.leave(mesh_id).await.unwrap();
        let forged = Mesh::create_with_id(
            mesh_id,
            "outsider mesh",
            c.shared.state.lock().unwrap().member.clone(),
            &c.shared.storage.key,
        )
        .unwrap();
        assert_ne!(
            forged.founder,
            b.shared.state.lock().unwrap().departed[&mesh_id].founder
        );
        c.shared
            .update(|state| {
                state.meshes.insert(mesh_id, forged);
                Ok(((), true))
            })
            .unwrap();
        let snapshot = c.shared.snapshot(mesh_id).unwrap();
        assert!(snapshot.verify().is_ok());
        assert_eq!(
            b.shared
                .answer_sync(&snapshot, c.info().id)
                .unwrap_err()
                .to_string(),
            "mesh unavailable"
        );
        let response: Result<Snapshot> = c
            .shared
            .request(b.shared.endpoint.addr(), SYNC_ALPN, &snapshot)
            .await;
        let error = response.unwrap_err().to_string();
        assert!(!error.contains("private mesh name"));
        assert!(!error.contains("founder-private-unique"));
        assert!(!error.contains("departed-private-unique"));
        assert!(!b.shared.state.lock().unwrap().meshes.contains_key(&mesh_id));
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn departure_announcements_must_come_from_the_departed_device() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &c.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let snapshot = b.shared.snapshot(b.info().only_mesh().unwrap()).unwrap();
        let reply: Result<String> = b
            .shared
            .request(a.shared.endpoint.addr(), DEPART_ALPN, &snapshot)
            .await;
        assert!(reply.is_err());
        assert_eq!(a.info().members.len(), 3);
        let left = b.leave(b.info().only_mesh().unwrap()).await.unwrap();
        let departure = b
            .shared
            .state
            .lock()
            .unwrap()
            .departure(left.mesh_id)
            .unwrap();
        let relayed: Result<String> = c
            .shared
            .request(a.shared.endpoint.addr(), DEPART_ALPN, &departure)
            .await;
        assert!(a
            .shared
            .record_departure(&departure, c.info().id)
            .unwrap_err()
            .to_string()
            .contains("peer has not left this mesh"));
        assert!(relayed.is_err());
        let different = Mesh::create(
            "different",
            b.shared.state.lock().unwrap().member.clone(),
            &b.shared.storage.key,
        )
        .unwrap();
        let mut different = different;
        different
            .admit(
                c.shared.state.lock().unwrap().member.clone(),
                &b.shared.storage.key,
            )
            .unwrap();
        different.depart(&b.shared.storage.key).unwrap();
        let other: Result<String> = b
            .shared
            .request(
                a.shared.endpoint.addr(),
                DEPART_ALPN,
                &Snapshot::without_addresses(different.clone()),
            )
            .await;
        assert!(a
            .shared
            .record_departure(&Snapshot::without_addresses(different), b.info().id)
            .unwrap_err()
            .to_string()
            .contains("device is not a member of this mesh"));
        assert!(other.is_err());
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn multiple_departures_keep_old_mesh_available_for_pull_and_readmission() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let original_id = a.info().mesh_id.unwrap();
        let b_id = b.info().id;
        a.shutdown().await.unwrap();
        drop(a);
        b.leave(b.info().only_mesh().unwrap()).await.unwrap();
        b.new_mesh("two").unwrap();
        b.add(
            b.info().only_mesh().unwrap(),
            &c.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let second_id = b.info().mesh_id.unwrap();
        b.leave(b.info().only_mesh().unwrap()).await.unwrap();
        {
            let state = b.shared.state.lock().unwrap();
            assert!(state.departed.contains_key(&original_id));
            assert!(state.departed.contains_key(&second_id));
            assert_eq!(state.departure_order, [original_id, second_id]);
        }
        let a = Node::bind(_a_dir.path(), NodeConfig::local())
            .await
            .unwrap();
        a.gossip.abort();
        assert!(a.info().members.iter().any(|member| member.id == b_id));
        a.shared
            .sync(b.shared.endpoint.addr(), a.info().only_mesh().unwrap())
            .await
            .unwrap();
        assert!(!a.info().members.iter().any(|member| member.id == b_id));
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(b.info().mesh_id, Some(original_id));
        {
            let state = b.shared.state.lock().unwrap();
            assert!(!state.departed.contains_key(&original_id));
            assert!(state.departed.contains_key(&second_id));
            assert_eq!(state.departure_order, [second_id]);
        }
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn evicted_departure_no_longer_prevents_stale_readmission() {
        let (a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        for node in [&a, &b, &c] {
            node.gossip.abort();
        }
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &c.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        a.shared
            .sync(b.shared.endpoint.addr(), a.info().only_mesh().unwrap())
            .await
            .unwrap();
        let mesh_id = a.info().mesh_id.unwrap();
        let b_id = b.info().id;
        a.shutdown().await.unwrap();
        drop(a);

        assert_eq!(
            b.leave(b.info().only_mesh().unwrap())
                .await
                .unwrap()
                .notified_members,
            1
        );
        assert!(!c.info().members.iter().any(|member| member.id == b_id));
        let peer = c.shared.state.lock().unwrap().member.clone();
        for _ in 0..64 {
            b.shared
                .update(|state| {
                    let mut mesh =
                        Mesh::create("later", state.member.clone(), &b.shared.storage.key)?;
                    mesh.admit(peer.clone(), &b.shared.storage.key)?;
                    let id = mesh.id;
                    state.meshes.insert(id, mesh);
                    state.leave(id, &b.shared.storage.key)?;
                    Ok(((), true))
                })
                .unwrap();
        }
        assert!(b.shared.state.lock().unwrap().departure(mesh_id).is_none());
        let a = Node::bind(a_dir.path(), NodeConfig::local()).await.unwrap();
        a.gossip.abort();
        assert!(a.info().members.iter().any(|member| member.id == b_id));
        assert!(a
            .shared
            .sync(b.shared.endpoint.addr(), a.info().only_mesh().unwrap())
            .await
            .is_err());
        assert!(a.info().members.iter().any(|member| member.id == b_id));

        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(b.info().mesh_id, Some(mesh_id));
        for node in [&a, &b] {
            let state = node.shared.state.lock().unwrap();
            assert_eq!(
                state
                    .meshes
                    .values()
                    .next()
                    .unwrap()
                    .admissions
                    .iter()
                    .filter(|admission| admission.member.id == b_id)
                    .map(|admission| admission.generation)
                    .max(),
                Some(0)
            );
        }
        assert!(!c.info().members.iter().any(|member| member.id == b_id));
        assert!(c
            .ping(b_id)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a member"));
        b.leave(b.info().only_mesh().unwrap()).await.unwrap();
        assert!(b.shared.state.lock().unwrap().departure(mesh_id).is_some());
        c.add(
            c.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        assert!(c.info().members.iter().any(|member| member.id == b_id));
        assert_eq!(
            c.shared
                .state
                .lock()
                .unwrap()
                .meshes
                .values()
                .next()
                .unwrap()
                .admissions
                .iter()
                .filter(|admission| admission.member.id == b_id)
                .map(|admission| admission.generation)
                .max(),
            Some(1)
        );
        for node in [&a, &b, &c] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn leaving_invalidates_a_pending_ticket() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        a.gossip.abort();
        b.gossip.abort();
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let pending = b.pair(Duration::from_secs(60)).await.unwrap();
        b.leave(b.info().only_mesh().unwrap()).await.unwrap();
        assert!(b.shared.pending.lock().unwrap().is_none());
        assert!(a
            .add(a.info().only_mesh().unwrap(), &pending)
            .await
            .is_err());
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn members_learn_an_offline_departure_when_they_next_reach_the_device() {
        let (_a_dir, a) = device("a").await;
        let (b_dir, b) = device("b").await;
        a.gossip.abort();
        a.new_mesh("one").unwrap();
        a.add(
            a.info().only_mesh().unwrap(),
            &b.pair(Duration::from_secs(60)).await.unwrap(),
        )
        .await
        .unwrap();
        let b_id = b.info().id;
        b.shutdown().await.unwrap();
        drop(b);

        let left = Node::leave_mesh(b_dir.path(), a.info().only_mesh().unwrap()).unwrap();
        assert_eq!(left.remaining_members, 1);
        assert_eq!(left.notified_members, 0);
        assert!(Node::leave_mesh(b_dir.path(), a.info().only_mesh().unwrap()).is_err());
        assert!(a.info().members.iter().any(|member| member.id == b_id));
        a.shared.heartbeat_received(b_id);
        assert!(a.shared.presence.lock().unwrap().contains_key(&b_id));

        let b = Node::bind(b_dir.path(), NodeConfig::local()).await.unwrap();
        a.shared.state.lock().unwrap().addresses.insert(
            b_id,
            crate::storage::AddressEntry::Current {
                direct: Some(b.shared.endpoint.addr()),
                hints: BTreeMap::new(),
            },
        );
        assert!(a.shared.ping(b_id).await.is_err());
        assert_eq!(a.info().members.len(), 1);
        assert!(a.peers().is_empty());
        assert!(!a.shared.presence.lock().unwrap().contains_key(&b_id));
        assert!(!a.shared.state.lock().unwrap().addresses.contains_key(&b_id));
        assert!(b.info().mesh_id.is_none());
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }
}
