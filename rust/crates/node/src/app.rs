use crate::{
    network::{finish_incoming, Shared},
    MeshId, NodeError, NodeId,
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

use crate::membership::decode_signature;

const APP_SIGNATURE_DOMAIN: &str = "spirit/app-signature/1";

pub(crate) fn validate_app_name(name: &str) -> Result<()> {
    ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'/' | b'-')),
        NodeError::Invalid("application names must be 1–64 ASCII bytes from [a-z0-9._/-]".into())
    );
    ensure!(
        !name.starts_with("spirit/"),
        NodeError::Invalid("reserved application name".into())
    );
    Ok(())
}

fn signed_bytes(domain: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    validate_app_name(domain)?;
    Ok(postcard::to_stdvec(&(APP_SIGNATURE_DOMAIN, domain, bytes))?)
}

pub fn verify_app(device: NodeId, domain: &str, bytes: &[u8], signature: &str) -> Result<()> {
    let bytes = signed_bytes(domain, bytes)?;
    ensure!(signature.len() == 86, "invalid app signature length");
    device
        .verify(
            &bytes,
            &decode_signature(signature).context("invalid app signature")?,
        )
        .context("invalid app signature")
}

pub(crate) fn sign_app(key: &iroh::SecretKey, domain: &str, bytes: &[u8]) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(key.sign(&signed_bytes(domain, bytes)?).to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{membership::Mesh, Member, Node, NodeConfig};
    use iroh::SecretKey;

    async fn device(name: &str) -> (tempfile::TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), name).unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        node.gossip.abort();
        (dir, node)
    }

    #[tokio::test]
    async fn signatures_are_domain_separated_from_membership() {
        let (_dir, node) = device("signer").await;
        let mesh = Mesh::create(
            "mesh",
            Member {
                id: node.info().id,
                name: "signer".into(),
            },
            &node.shared.storage.key,
        )
        .unwrap();
        let entry = mesh.admissions[0].payload(&mesh).unwrap();
        let signature = node.sign_app("afm/catalog/op/1", &entry).unwrap();
        assert_eq!(signature.len(), 86);
        assert!(verify_app(
            node.info().id,
            "afm/catalog/op/1",
            &entry,
            &format!("{signature}=")
        )
        .is_err());
        for name in [
            "",
            "AFM/catalog",
            "afm/\u{202e}",
            "spirit/reserved",
            &"x".repeat(65),
        ] {
            assert!(node.sign_app(name, &entry).is_err(), "{name:?}");
            assert!(
                verify_app(node.info().id, name, &entry, &signature).is_err(),
                "{name:?}"
            );
        }
        verify_app(node.info().id, "afm/catalog/op/1", &entry, &signature).unwrap();
        assert!(verify_app(
            SecretKey::generate().public(),
            "afm/catalog/op/1",
            b"entry",
            &signature
        )
        .is_err());
        assert!(verify_app(node.info().id, "afm/catalog/op/2", &entry, &signature).is_err());
        assert!(verify_app(node.info().id, "afm/catalog/op/1", b"other", &signature).is_err());
        assert!(node.sign_app("spirit/mesh/admission/2", &entry).is_err());
        assert!(verify_app(
            node.info().id,
            "spirit/mesh/admission/2",
            b"entry",
            &signature
        )
        .is_err());
        let mut admission = serde_json::to_value(&mesh).unwrap();
        admission["admissions"][0]["signature"] = signature.into();
        let forged: Mesh = serde_json::from_value(admission).unwrap();
        assert!(forged.verify().is_err());
        let mut departed = mesh.clone();
        departed.depart(&node.shared.storage.key).unwrap();
        let departure = departed.departures[0].payload(&departed).unwrap();
        let departure_signature = node.sign_app("afm/catalog/op/1", &departure).unwrap();
        verify_app(
            node.info().id,
            "afm/catalog/op/1",
            &departure,
            &departure_signature,
        )
        .unwrap();
        let mut forged = serde_json::to_value(&departed).unwrap();
        forged["departures"][0]["signature"] = departure_signature.into();
        assert!(serde_json::from_value::<Mesh>(forged)
            .unwrap()
            .verify()
            .is_err());
        node.shutdown().await.unwrap();
    }
}
pub(crate) const APP_ALPN: &[u8] = b"spirit/app/1";
pub(crate) const MAX_APP_BYTES: usize = 256 * 1024;
pub(crate) const MAX_APP_IN_FLIGHT: usize = 16;
pub(crate) const MAX_APP_PER_PEER: usize = 4;
pub(crate) const MAX_APP_ENCODED: usize = base64::encoded_len(MAX_APP_BYTES, false).unwrap();
const APP_ENVELOPE_BYTES: usize = 256;
pub(crate) const MAX_APP_WIRE: usize = MAX_APP_ENCODED + APP_ENVELOPE_BYTES;
const DIAGNOSTICS_LIMIT: usize = 32;
const APP_REPLY_CLOSE_WAIT: Duration = Duration::from_millis(100);
const APP_REPLY_RESERVE: Duration = Duration::from_millis(250);
const HANDLER_SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
const APP_UNAVAILABLE: &str = "app unavailable";
#[derive(Clone)]
pub struct AppCallContext {
    mesh: MeshId,
    peer: NodeId,
    protocol: String,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl AppCallContext {
    pub fn mesh(&self) -> MeshId {
        self.mesh
    }
    pub fn peer(&self) -> NodeId {
        self.peer
    }
    pub fn protocol(&self) -> &str {
        &self.protocol
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub trait AppHandler: Send + Sync {
    fn handle(&self, context: AppCallContext, payload: Vec<u8>) -> Result<Vec<u8>>;
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub channel: &'static str,
    pub mesh: Option<MeshId>,
    pub peer: NodeId,
    pub protocol: Option<String>,
    pub cause: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct AppRequest {
    pub mesh_id: MeshId,
    pub protocol: String,
    pub payload: String,
}

#[derive(Default)]
pub(crate) struct DiagnosticLog {
    entries: Mutex<VecDeque<Diagnostic>>,
}

impl DiagnosticLog {
    pub(crate) fn entries(&self) -> Vec<Diagnostic> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    pub(crate) fn record(
        &self,
        channel: &'static str,
        mesh: Option<MeshId>,
        peer: NodeId,
        protocol: Option<&str>,
        cause: &str,
    ) {
        let mut diagnostics = self.entries.lock().unwrap();
        if diagnostics.len() == DIAGNOSTICS_LIMIT {
            diagnostics.pop_front();
        }
        diagnostics.push_back(Diagnostic {
            channel,
            mesh,
            peer,
            protocol: protocol.map(crate::network::bounded_diagnostic),
            cause: crate::network::bounded_diagnostic(cause),
        });
    }
}

pub(crate) struct AppState {
    handlers: Mutex<BTreeMap<String, Arc<dyn AppHandler>>>,
    total: Arc<Semaphore>,
    peers: Mutex<BTreeMap<NodeId, usize>>,
    active: Mutex<ActiveHandlers>,
}

#[derive(Default)]
struct ActiveHandlers {
    stopping: bool,
    cancellations: Vec<Weak<AtomicBool>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            handlers: Mutex::new(BTreeMap::new()),
            total: Arc::new(Semaphore::new(MAX_APP_IN_FLIGHT)),
            peers: Mutex::new(BTreeMap::new()),
            active: Mutex::new(ActiveHandlers::default()),
        }
    }
}

impl AppState {
    pub(crate) fn register(&self, protocol: &str, handler: Arc<dyn AppHandler>) -> Result<()> {
        validate_app_name(protocol)?;
        self.handlers
            .lock()
            .unwrap()
            .insert(protocol.into(), handler);
        Ok(())
    }

    pub(crate) fn unregister(&self, protocol: &str) -> Result<()> {
        validate_app_name(protocol)?;
        self.handlers.lock().unwrap().remove(protocol);
        Ok(())
    }

    fn enter(self: &Arc<Self>, peer: NodeId) -> Result<AppPermit> {
        let mut peers = self.peers.lock().unwrap();
        let count = peers.entry(peer).or_default();
        if *count >= MAX_APP_PER_PEER {
            anyhow::bail!("app per-peer concurrency limit reached");
        }
        *count += 1;
        Ok(AppPermit {
            state: self.clone(),
            peer,
            _total: None,
        })
    }

    pub(crate) async fn shutdown(&self) {
        let tasks = {
            let mut active = self.active.lock().unwrap();
            active.stopping = true;
            for token in active.cancellations.drain(..) {
                if let Some(token) = token.upgrade() {
                    token.store(true, Ordering::Release);
                }
            }
            std::mem::take(&mut active.tasks)
        };
        let deadline = tokio::time::Instant::now() + HANDLER_SHUTDOWN_GRACE;
        for task in tasks {
            if tokio::time::timeout_at(deadline, task).await.is_err() {
                break;
            }
        }
    }

    async fn dispatch(
        self: &Arc<Self>,
        shared: &Shared,
        peer: NodeId,
        request: AppRequest,
        permit: AppPermit,
        cancelled: Arc<AtomicBool>,
        deadline: Instant,
    ) -> Result<String> {
        ensure!(
            shared.both_current_members(request.mesh_id, peer),
            "mesh unavailable"
        );
        validate_app_name(&request.protocol)?;
        ensure!(
            request.payload.len() <= MAX_APP_ENCODED,
            "app encoded payload is too large"
        );
        let payload = URL_SAFE_NO_PAD
            .decode(&request.payload)
            .context("invalid app payload")?;
        ensure!(payload.len() <= MAX_APP_BYTES, "app payload is too large");
        let handler = self
            .handlers
            .lock()
            .unwrap()
            .get(&request.protocol)
            .cloned()
            .context("unknown app protocol")?;
        let total = self
            .total
            .clone()
            .try_acquire_owned()
            .context("app concurrency limit reached")?;
        let mut permit = permit;
        permit._total = Some(total);
        let context = AppCallContext {
            mesh: request.mesh_id,
            peer,
            protocol: request.protocol,
            deadline,
            cancelled: cancelled.clone(),
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut active = self.active.lock().unwrap();
            ensure!(!active.stopping, "app shutting down");
            active
                .cancellations
                .retain(|token| token.strong_count() > 0);
            active.cancellations.push(Arc::downgrade(&cancelled));
            active.tasks.retain(|task| !task.is_finished());
            active.tasks.push(tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handler.handle(context, payload)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("app handler panicked")));
                let _ = tx.send(result);
            }));
        }
        let response = rx.await.context("app handler stopped")??;
        ensure!(response.len() <= MAX_APP_BYTES, "app response is too large");
        Ok(URL_SAFE_NO_PAD.encode(response))
    }
}

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct AppPermit {
    state: Arc<AppState>,
    peer: NodeId,
    _total: Option<OwnedSemaphorePermit>,
}

impl Drop for AppPermit {
    fn drop(&mut self) {
        let mut peers = self.state.peers.lock().unwrap();
        let count = peers.get_mut(&self.peer).unwrap();
        *count -= 1;
        if *count == 0 {
            peers.remove(&self.peer);
        }
    }
}

pub(crate) struct AppProtocol {
    shared: Arc<Shared>,
    app: Arc<AppState>,
}

impl fmt::Debug for AppProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AppProtocol")
    }
}

impl AppProtocol {
    pub(crate) fn new(shared: Arc<Shared>, app: Arc<AppState>) -> Self {
        Self { shared, app }
    }

    fn transport<T, E: Into<anyhow::Error>>(
        &self,
        peer: NodeId,
        result: std::result::Result<T, E>,
    ) -> Result<T> {
        result.map_err(|error| {
            let error = error.into();
            self.shared.failed(peer, &error);
            error
        })
    }

    async fn respond(&self, connection: &Connection) -> Result<()> {
        let peer = connection.remote_id();
        let deadline = Instant::now()
            + self
                .shared
                .config
                .request_timeout
                .saturating_sub(APP_REPLY_RESERVE);
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel = CancelOnDrop(cancelled.clone());
        let (mut send, mut recv) = self.transport(peer, connection.accept_bi().await)?;
        let permit = self.app.enter(peer);
        let mut mesh = None;
        let mut protocol = None;
        let result = async {
            let permit = permit?;
            let bytes = recv
                .read_to_end(MAX_APP_WIRE)
                .await
                .context("app request wire limit exceeded")?;
            let request: AppRequest =
                serde_json::from_slice(&bytes).context("invalid app request")?;
            mesh = Some(request.mesh_id);
            if validate_app_name(&request.protocol).is_ok() {
                protocol = Some(request.protocol.clone());
            }
            tokio::select! {
                result = self.app.dispatch(&self.shared, peer, request, permit, cancelled, deadline) => result,
                _ = connection.closed() => anyhow::bail!("app requester disconnected"),
            }
        }
        .await;
        if let Err(error) = &result {
            self.shared.diagnostics.record(
                "spirit/app/1",
                mesh,
                peer,
                protocol.as_deref(),
                &format!("{error:#}"),
            );
        }
        let accepted = result.is_ok();
        let reply: Result<String, &str> = result.map_err(|_| APP_UNAVAILABLE);
        self.transport(peer, send.write_all(&serde_json::to_vec(&reply)?).await)?;
        self.transport(peer, send.finish())?;
        if accepted {
            connection.closed().await;
        } else {
            let _ = tokio::time::timeout(APP_REPLY_CLOSE_WAIT, connection.closed()).await;
        }
        Ok(())
    }
}

impl ProtocolHandler for AppProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if !self.shared.is_member(connection.remote_id()) {
            connection.close(0u32.into(), b"finished");
            return Ok(());
        }
        finish_incoming(
            &connection,
            self.shared.config.request_timeout,
            self.respond(&connection),
            |error| {
                self.shared.diagnostics.record(
                    "spirit/app/1",
                    None,
                    connection.remote_id(),
                    None,
                    &format!("{error:#}"),
                )
            },
        )
        .await
    }

    async fn shutdown(&self) {
        self.app.shutdown().await;
    }
}

#[cfg(test)]
mod channel_tests {
    use super::*;
    use crate::{Member, Node, NodeConfig};
    use iroh::SecretKey;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    async fn device(name: &str) -> (tempfile::TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), name).unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        node.gossip.abort();
        (dir, node)
    }

    async fn join(founder: &Node, mesh: MeshId, peer: &Node) {
        founder
            .add(mesh, &peer.pair(Duration::from_secs(60)).await.unwrap())
            .await
            .unwrap();
        peer.shared
            .sync(founder.shared.endpoint.addr(), mesh)
            .await
            .unwrap();
    }

    fn request(mesh_id: MeshId) -> AppRequest {
        AppRequest {
            mesh_id,
            protocol: "afm/echo".into(),
            payload: URL_SAFE_NO_PAD.encode(b"hello"),
        }
    }

    async fn raw(
        sender: &Node,
        receiver: &Node,
        request: &AppRequest,
    ) -> Result<Result<String, String>> {
        sender
            .shared
            .request(receiver.shared.endpoint.addr(), APP_ALPN, request)
            .await
    }

    async fn assert_refused(sender: &Node, receiver: &Node, request: &AppRequest) {
        let answer = raw(sender, receiver, request).await;
        assert!(
            matches!(answer, Ok(Err(ref reason)) if reason == APP_UNAVAILABLE),
            "{answer:?}"
        );
    }

    async fn free_slots(node: &Node) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while node.app.total.available_permits() != MAX_APP_IN_FLIGHT
                || !node.app.peers.lock().unwrap().is_empty()
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    struct Echo;
    impl AppHandler for Echo {
        fn handle(&self, _: AppCallContext, payload: Vec<u8>) -> Result<Vec<u8>> {
            Ok(payload)
        }
    }

    #[tokio::test]
    async fn mesh_isolation_and_generic_failures() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("shared").unwrap();
        join(&b, mesh, &a).await;
        let other = b.new_mesh("not shared").unwrap();
        b.register_app_handler("afm/echo", Arc::new(Echo)).unwrap();
        assert_eq!(
            a.app_request(mesh, b.info().id, "afm/echo", b"hello".to_vec())
                .await
                .unwrap(),
            b"hello"
        );
        assert_refused(&a, &b, &request(other)).await;
        assert!(a
            .app_request(other, b.info().id, "afm/echo", vec![])
            .await
            .is_err());
        for node in [&a, &b] {
            node.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn large_reply_survives_a_slow_reader() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        b.register_app_handler("afm/echo", Arc::new(Echo)).unwrap();
        let connection = a
            .shared
            .endpoint
            .connect(b.shared.endpoint.addr(), APP_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        let payload = vec![42; MAX_APP_BYTES - 16];
        let request = AppRequest {
            payload: URL_SAFE_NO_PAD.encode(&payload),
            ..request(mesh)
        };
        send.write_all(&serde_json::to_vec(&request).unwrap())
            .await
            .unwrap();
        send.finish().unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut response = Vec::new();
        let mut chunk = [0; 4096];
        while let Some(count) = recv.read(&mut chunk).await.unwrap() {
            response.extend_from_slice(&chunk[..count]);
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        let reply: Result<String, String> = serde_json::from_slice(&response).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(reply.unwrap()).unwrap(), payload);
        connection.close(0u32.into(), b"done");
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    struct Oversize;
    impl AppHandler for Oversize {
        fn handle(&self, _: AppCallContext, _: Vec<u8>) -> Result<Vec<u8>> {
            Ok(vec![0; MAX_APP_BYTES + 1])
        }
    }

    #[tokio::test]
    async fn sizes_and_protocol_validation_precede_dispatch_and_log() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("shared").unwrap();
        join(&b, mesh, &a).await;
        b.register_app_handler("afm/echo", Arc::new(Echo)).unwrap();
        assert_refused(
            &a,
            &b,
            &AppRequest {
                payload: URL_SAFE_NO_PAD.encode(vec![0; MAX_APP_BYTES + 1]),
                ..request(mesh)
            },
        )
        .await;
        assert_refused(
            &a,
            &b,
            &AppRequest {
                payload: URL_SAFE_NO_PAD.encode(vec![0; 300_000]),
                ..request(mesh)
            },
        )
        .await;
        assert_refused(
            &a,
            &b,
            &AppRequest {
                protocol: "\u{202e}secret".into(),
                ..request(mesh)
            },
        )
        .await;
        assert_eq!(b.diagnostics().last().unwrap().protocol, None);
        b.register_app_handler("afm/echo", Arc::new(Oversize))
            .unwrap();
        assert_refused(&a, &b, &request(mesh)).await;
        assert_eq!(
            b.diagnostics().last().unwrap().cause,
            "app response is too large"
        );
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    struct Slow {
        started: AtomicUsize,
        release: AtomicBool,
    }
    impl AppHandler for Slow {
        fn handle(&self, _: AppCallContext, _: Vec<u8>) -> Result<Vec<u8>> {
            self.started.fetch_add(1, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(Vec::new())
        }
    }

    async fn spawn_exchange(
        sender: &Node,
        receiver: &Node,
        mesh: MeshId,
    ) -> tokio::task::JoinHandle<Result<Result<String, String>>> {
        let shared = sender.shared.clone();
        let address = receiver.shared.endpoint.addr();
        tokio::spawn(async move {
            shared
                .app_exchange(address, &request(mesh), MAX_APP_WIRE)
                .await
        })
    }

    #[tokio::test]
    async fn slots_stay_bounded_through_reads_and_blocking_handlers() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        let connection = a
            .shared
            .endpoint
            .connect(b.shared.endpoint.addr(), APP_ALPN)
            .await
            .unwrap();
        let (mut send, _recv) = connection.open_bi().await.unwrap();
        send.write_all(b"{").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while b.app.peers.lock().unwrap().get(&a.info().id) != Some(&1) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(b.app.total.available_permits(), MAX_APP_IN_FLIGHT);
        connection.close(0u32.into(), b"done");
        free_slots(&b).await;
        let slow = Arc::new(Slow {
            started: AtomicUsize::new(0),
            release: AtomicBool::new(false),
        });
        b.register_app_handler("afm/echo", slow.clone()).unwrap();
        let mut tasks = Vec::new();
        for _ in 0..MAX_APP_PER_PEER {
            tasks.push(spawn_exchange(&a, &b, mesh).await);
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            while slow.started.load(Ordering::SeqCst) != MAX_APP_PER_PEER {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_refused(&a, &b, &request(mesh)).await;
        slow.release.store(true, Ordering::SeqCst);
        for task in tasks {
            assert!(task.await.unwrap().unwrap().is_ok());
        }
        let state = Arc::new(AppState::default());
        let permits: Vec<_> = (0..MAX_APP_IN_FLIGHT)
            .map(|_| state.total.clone().try_acquire_owned().unwrap())
            .collect();
        assert!(state.total.clone().try_acquire_owned().is_err());
        drop(permits);
        free_slots(&b).await;
        b.register_app_handler("afm/echo", Arc::new(Panics))
            .unwrap();
        assert_refused(&a, &b, &request(mesh)).await;
        free_slots(&b).await;
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    struct Panics;
    impl AppHandler for Panics {
        fn handle(&self, _: AppCallContext, _: Vec<u8>) -> Result<Vec<u8>> {
            panic!("private panic details")
        }
    }
    #[tokio::test]
    async fn outsider_flood_closes_promptly_without_evicting_member_log() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let (_od, outsider) = device("outsider").await;
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        assert_refused(&a, &b, &request(mesh)).await;
        let mut connections = Vec::new();
        for _ in 0..96 {
            connections.push(
                outsider
                    .shared
                    .endpoint
                    .connect(b.shared.endpoint.addr(), APP_ALPN)
                    .await
                    .unwrap(),
            );
        }
        for connection in connections {
            tokio::time::timeout(Duration::from_secs(2), connection.closed())
                .await
                .unwrap();
        }
        assert_eq!(b.diagnostics()[0].peer, a.info().id);
        for node in [&a, &b, &outsider] {
            node.shutdown().await.unwrap();
        }
    }

    #[test]
    fn diagnostics_are_local_bounded_and_sanitized() {
        let state = DiagnosticLog::default();
        let peer = SecretKey::generate().public();
        for index in 0..33 {
            state.record(
                "spirit/blob/1",
                None,
                peer,
                None,
                &format!("{index}:{}\u{202e}\n", "x".repeat(300)),
            );
        }
        let entries = state.entries();
        assert_eq!(entries.len(), DIAGNOSTICS_LIMIT);
        assert!(entries[0].cause.starts_with("1:"));
        assert!(entries.iter().all(|item| item.channel == "spirit/blob/1"
            && item.cause.len() <= 256
            && !item.cause.contains('\u{202e}')
            && !item.cause.contains('\n')));
    }

    #[tokio::test]
    #[ignore]
    async fn app_membership_refusal_timing_at_256_admissions() {
        let (_dir, node) = device("root").await;
        let id = node.new_mesh("mesh").unwrap();
        node.shared
            .update(|state| {
                let mesh = state.meshes.get_mut(&id).unwrap();
                for index in 1..256 {
                    mesh.admit(
                        Member {
                            id: SecretKey::generate().public(),
                            name: index.to_string(),
                        },
                        &node.shared.storage.key,
                    )?;
                }
                Ok(((), true))
            })
            .unwrap();
        let outsider = SecretKey::generate().public();
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            std::hint::black_box(node.shared.both_current_members(id, outsider));
        }
        println!(
            "known mesh outsider: {:.1} ns/call",
            start.elapsed().as_nanos() as f64 / 100_000.0
        );
        node.shutdown().await.unwrap();
    }

    struct WaitForCancellation {
        started: AtomicUsize,
        cancelled: AtomicBool,
        initial_budget_ms: AtomicU64,
    }

    impl AppHandler for WaitForCancellation {
        fn handle(&self, context: AppCallContext, _: Vec<u8>) -> Result<Vec<u8>> {
            self.initial_budget_ms
                .store(context.remaining().as_millis() as u64, Ordering::SeqCst);
            self.started.fetch_add(1, Ordering::SeqCst);
            while !context.is_cancelled() && !context.remaining().is_zero() {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.cancelled
                .store(context.is_cancelled(), Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    async fn started(handler: &WaitForCancellation, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while handler.started.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn responder_deadline_frees_slot_while_connection_remains_open() {
        let (_ad, a) = device("a").await;
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "b").unwrap();
        let b = Node::bind(
            dir.path(),
            NodeConfig {
                request_timeout: Duration::from_millis(650),
                ..NodeConfig::local()
            },
        )
        .await
        .unwrap();
        b.gossip.abort();
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        let handler = Arc::new(WaitForCancellation {
            started: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            initial_budget_ms: AtomicU64::new(0),
        });
        b.register_app_handler("afm/echo", handler.clone()).unwrap();
        let connection = a
            .shared
            .endpoint
            .connect(b.shared.endpoint.addr(), APP_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&serde_json::to_vec(&request(mesh)).unwrap())
            .await
            .unwrap();
        send.finish().unwrap();
        started(&handler, 1).await;
        let answer: Result<String, String> = serde_json::from_slice(
            &tokio::time::timeout(Duration::from_secs(2), recv.read_to_end(MAX_APP_WIRE))
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(answer.is_ok());
        assert!(!handler.cancelled.load(Ordering::SeqCst));
        assert!((1..=400).contains(&handler.initial_budget_ms.load(Ordering::SeqCst)));
        free_slots(&b).await;
        connection.close(0u32.into(), b"finished");
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn disconnected_requester_cancels_handler() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        let handler = Arc::new(WaitForCancellation {
            started: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            initial_budget_ms: AtomicU64::new(0),
        });
        b.register_app_handler("afm/echo", handler.clone()).unwrap();
        let connection = a
            .shared
            .endpoint
            .connect(b.shared.endpoint.addr(), APP_ALPN)
            .await
            .unwrap();
        let (mut send, _recv) = connection.open_bi().await.unwrap();
        send.write_all(&serde_json::to_vec(&request(mesh)).unwrap())
            .await
            .unwrap();
        send.finish().unwrap();
        started(&handler, 1).await;
        connection.close(0u32.into(), b"disconnected");
        free_slots(&b).await;
        assert!(handler.cancelled.load(Ordering::SeqCst));
        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unregister_keeps_in_flight_snapshot_and_shutdown_cancels() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        let slow = Arc::new(Slow {
            started: AtomicUsize::new(0),
            release: AtomicBool::new(false),
        });
        b.register_app_handler("afm/echo", slow.clone()).unwrap();
        let task = spawn_exchange(&a, &b, mesh).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while slow.started.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        b.unregister_app_handler("afm/echo").unwrap();
        slow.release.store(true, Ordering::SeqCst);
        assert!(task.await.unwrap().unwrap().is_ok());
        assert_refused(&a, &b, &request(mesh)).await;
        free_slots(&b).await;
        let handler = Arc::new(WaitForCancellation {
            started: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            initial_budget_ms: AtomicU64::new(0),
        });
        b.register_app_handler("afm/echo", handler.clone()).unwrap();
        let task = spawn_exchange(&a, &b, mesh).await;
        started(&handler, 1).await;
        b.shutdown().await.unwrap();
        free_slots(&b).await;
        assert!(handler.cancelled.load(Ordering::SeqCst));
        let _ = task.await.unwrap();
        a.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_rejects_grace_period_requests_and_detaches_noncooperative_handler() {
        let (_ad, a) = device("a").await;
        let (_bd, b) = device("b").await;
        let b = Arc::new(b);
        let mesh = b.new_mesh("mesh").unwrap();
        join(&b, mesh, &a).await;
        let slow = Arc::new(Slow {
            started: AtomicUsize::new(0),
            release: AtomicBool::new(false),
        });
        struct ReleaseOnDrop(Arc<Slow>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.release.store(true, Ordering::SeqCst);
            }
        }
        let _release = ReleaseOnDrop(slow.clone());
        b.register_app_handler("afm/echo", slow.clone()).unwrap();
        let first = spawn_exchange(&a, &b, mesh).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while slow.started.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let start = Instant::now();
        let shutdown = tokio::spawn({
            let b = b.clone();
            async move { b.shutdown().await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(b.app.active.lock().unwrap().stopping);
        let second =
            tokio::time::timeout(Duration::from_secs(4), spawn_exchange(&a, &b, mesh).await)
                .await
                .unwrap();
        assert!(!matches!(second, Ok(Ok(_))));
        shutdown.await.unwrap();
        assert!(start.elapsed() >= HANDLER_SHUTDOWN_GRACE);
        assert_eq!(b.app.total.available_permits(), MAX_APP_IN_FLIGHT - 1);
        slow.release.store(true, Ordering::SeqCst);
        let _ = first.await;
        free_slots(&b).await;
        a.shutdown().await.unwrap();
    }
}
