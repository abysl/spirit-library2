use crate::FfiError;
use qrcode::{Color, QrCode};
use spirit_sdk::{Node, NodeConfig};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

fn runtime() -> Result<&'static Runtime, FfiError> {
    static RUNTIME: OnceLock<Result<Runtime, std::io::Error>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
        })
        .as_ref()
        .map_err(|error| FfiError::Node(error.to_string()))
}

fn node_error(error: anyhow::Error) -> FfiError {
    FfiError::Node(format!("{error:#}"))
}

#[derive(Clone, uniffi::Record)]
pub struct MeshPeer {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub last_received_ago_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(uniffi::Record)]
pub struct MeshStatus {
    pub id: String,
    pub name: String,
    pub mesh_id: Option<String>,
    pub mesh_name: Option<String>,
    pub peers: Vec<MeshPeer>,
}

#[derive(uniffi::Record)]
pub struct PairingCode {
    pub ticket: String,
    pub width: u32,
    pub modules: Vec<u8>,
    pub lifetime_seconds: u32,
}

#[derive(uniffi::Record)]
pub struct LeftMesh {
    pub mesh_id: String,
    pub mesh_name: String,
    pub remaining_members: u32,
    pub notified_members: u32,
}

#[derive(uniffi::Record)]
pub struct PingReply {
    pub name: String,
    pub elapsed_ms: u64,
}

#[derive(uniffi::Object)]
pub struct SpiritNode {
    node: Mutex<Option<Arc<Node>>>,
}

impl SpiritNode {
    fn active(&self) -> Result<Arc<Node>, FfiError> {
        self.node
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| FfiError::Node("node is closed".into()))
    }
}

#[uniffi::export]
impl SpiritNode {
    #[uniffi::constructor]
    pub fn open(node_dir: String, nickname: String, local: bool) -> Result<Self, FfiError> {
        Node::init(&node_dir, &nickname).map_err(node_error)?;
        let config = if local {
            NodeConfig::local()
        } else {
            NodeConfig::default()
        };
        let node = runtime()?
            .block_on(Node::bind(node_dir, config))
            .map_err(node_error)?;
        Ok(Self {
            node: Mutex::new(Some(Arc::new(node))),
        })
    }

    pub fn status(&self) -> Result<MeshStatus, FfiError> {
        let node = self.active()?;
        let info = node.info();
        let mesh_id = info.mesh_id;
        let peers = node
            .peers()
            .into_iter()
            .map(|peer| MeshPeer {
                id: peer.id.to_string(),
                name: peer.name,
                connected: peer.connected,
                last_received_ago_ms: peer.last_received_ago_ms,
                last_error: peer.last_error,
            })
            .collect();
        Ok(MeshStatus {
            id: info.id.to_string(),
            name: info.name,
            mesh_id: mesh_id.map(|id| id.to_string()),
            mesh_name: info.mesh_name,
            peers,
        })
    }

    pub fn create_mesh(&self, name: String) -> Result<(), FfiError> {
        let guard = self.node.lock().unwrap();
        let node = guard
            .as_ref()
            .ok_or_else(|| FfiError::Node("node is closed".into()))?;
        node.create_first_mesh(&name).map_err(node_error)?;
        Ok(())
    }

    pub fn leave_mesh(&self) -> Result<LeftMesh, FfiError> {
        let node = self.active()?;
        let mesh_id = node.info().only_mesh().map_err(node_error)?;
        let left = runtime()?
            .block_on(node.leave(mesh_id))
            .map_err(node_error)?;
        Ok(LeftMesh {
            mesh_id: left.mesh_id.to_string(),
            mesh_name: left.mesh_name,
            remaining_members: left.remaining_members as u32,
            notified_members: left.notified_members as u32,
        })
    }

    pub fn pair(&self) -> Result<PairingCode, FfiError> {
        let node = self.active()?;
        let ticket = runtime()?
            .block_on(node.pair_when_unenrolled(Duration::from_secs(300)))
            .map_err(node_error)?;
        let qr =
            QrCode::new(ticket.as_bytes()).map_err(|error| FfiError::Node(error.to_string()))?;
        Ok(PairingCode {
            ticket,
            width: qr.width() as u32,
            modules: qr
                .to_colors()
                .into_iter()
                .map(|color| u8::from(color == Color::Dark))
                .collect(),
            lifetime_seconds: 300,
        })
    }

    pub fn add(&self, ticket: String) -> Result<String, FfiError> {
        let node = self.active()?;
        let mesh_id = node.info().only_mesh().map_err(node_error)?;
        let member = runtime()?
            .block_on(node.add(mesh_id, ticket.trim()))
            .map_err(node_error)?;
        Ok(member.name)
    }

    pub fn ping(&self, device: String) -> Result<PingReply, FfiError> {
        let node = self.active()?;
        let member = node.info().resolve(&device).map_err(node_error)?;
        let pong = runtime()?
            .block_on(node.ping(member.id))
            .map_err(node_error)?;
        Ok(PingReply {
            name: pong.name,
            elapsed_ms: pong.elapsed_ms.min(u64::MAX as u128) as u64,
        })
    }

    pub fn shutdown(&self) -> Result<(), FfiError> {
        let node = self.node.lock().unwrap().take();
        if let Some(node) = node {
            runtime()?.block_on(node.shutdown()).map_err(node_error)?;
        }
        Ok(())
    }
}

impl Drop for SpiritNode {
    fn drop(&mut self) {
        if let Some(node) = self.node.get_mut().unwrap().take() {
            if let Ok(runtime) = runtime() {
                runtime.spawn(async move {
                    let _ = node.shutdown().await;
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_mesh_refuses_a_second_mesh_and_withdraws_a_ticket() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = SpiritNode::open(a_dir.path().to_str().unwrap().into(), "a".into(), true).unwrap();
        let b = SpiritNode::open(b_dir.path().to_str().unwrap().into(), "b".into(), true).unwrap();
        a.create_mesh("one".into()).unwrap();
        let stale = b.pair().unwrap();
        b.create_mesh("two".into()).unwrap();
        assert!(format!("{}", b.pair().err().unwrap()).contains("single-mesh until S3"));
        assert!(a.add(stale.ticket).is_err());
        assert!(format!("{}", b.create_mesh("three".into()).unwrap_err())
            .contains("single-mesh until S3"));
        assert_eq!(b.status().unwrap().mesh_name.as_deref(), Some("two"));
        a.shutdown().unwrap();
        b.shutdown().unwrap();
    }

    #[test]
    fn concurrent_create_mesh_calls_leave_one_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let node = Arc::new(
            SpiritNode::open(dir.path().to_str().unwrap().into(), "desktop".into(), true).unwrap(),
        );
        let results: Vec<_> = (0..2)
            .map(|index| {
                let node = node.clone();
                std::thread::spawn(move || node.create_mesh(format!("mesh-{index}")))
            })
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(node.status().unwrap().mesh_id.is_some());
        node.shutdown().unwrap();
    }

    #[test]
    fn mesh_specific_bindings_need_selection_for_multiple_meshes() {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "desktop").unwrap();
        Node::create_mesh(dir.path(), "one").unwrap();
        Node::create_mesh(dir.path(), "two").unwrap();
        let ffi =
            SpiritNode::open(dir.path().to_str().unwrap().into(), "desktop".into(), true).unwrap();
        let status = ffi.status().unwrap();
        assert!(status.mesh_id.is_none());
        assert!(status.mesh_name.is_none());
        assert!(format!("{}", ffi.add("not-a-ticket".into()).unwrap_err()).contains("choose one"));
        assert!(format!("{}", ffi.leave_mesh().err().unwrap()).contains("choose one"));
        assert!(format!("{}", ffi.pair().err().unwrap()).contains("single-mesh until S3"));
        ffi.shutdown().unwrap();
    }

    #[test]
    fn node_bindings_enroll_ping_and_close() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = SpiritNode::open(
            a_dir.path().to_str().unwrap().into(),
            "desktop".into(),
            true,
        )
        .unwrap();
        let b =
            SpiritNode::open(b_dir.path().to_str().unwrap().into(), "phone".into(), true).unwrap();
        a.create_mesh("personal".into()).unwrap();
        let code = b.pair().unwrap();
        assert_eq!(code.modules.len(), (code.width * code.width) as usize);
        assert_eq!(a.add(code.ticket).unwrap(), "phone");
        assert_eq!(a.ping("phone".into()).unwrap().name, "phone");
        assert!(a.status().unwrap().peers[0].connected);
        assert!(b.status().unwrap().peers[0].connected);
        let left = b.leave_mesh().unwrap();
        assert_eq!(left.mesh_name, "personal");
        assert_eq!((left.remaining_members, left.notified_members), (1, 1));
        assert!(b.status().unwrap().mesh_id.is_none());
        assert!(a.status().unwrap().peers.is_empty());
        assert!(b.leave_mesh().is_err());
        a.shutdown().unwrap();
        b.shutdown().unwrap();
        assert!(a.status().is_err());
        let reopened = SpiritNode::open(
            a_dir.path().to_str().unwrap().into(),
            "desktop".into(),
            true,
        )
        .unwrap();
        assert_eq!(
            reopened.status().unwrap().mesh_name.as_deref(),
            Some("personal")
        );
        reopened.shutdown().unwrap();
    }
}
