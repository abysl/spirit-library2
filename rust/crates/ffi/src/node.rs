use crate::FfiError;
use qrcode::{Color, QrCode};
use spirit_sdk::{MeshId, Node, NodeConfig};
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
    let detail = format!("{error:#}");
    match error.downcast_ref::<spirit_sdk::NodeError>() {
        Some(spirit_sdk::NodeError::MeshLimit) => FfiError::MeshLimit,
        Some(spirit_sdk::NodeError::NotMember) => FfiError::NotMember(detail),
        Some(spirit_sdk::NodeError::TicketRejected(_)) => FfiError::TicketRejected(detail),
        Some(spirit_sdk::NodeError::NodeClosed) => FfiError::NodeClosed,
        Some(spirit_sdk::NodeError::NodeBusy) => FfiError::NodeBusy,
        Some(spirit_sdk::NodeError::Invalid(_)) => FfiError::Invalid(detail),
        Some(spirit_sdk::NodeError::Unavailable(_)) => FfiError::Unavailable(detail),
        None => FfiError::Node(detail),
    }
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
pub struct MeshMember {
    pub id: String,
    pub name: String,
    pub generation: u32,
}

#[derive(uniffi::Record)]
pub struct MeshStatus {
    pub id: String,
    pub name: String,
    pub members: Vec<MeshMember>,
}

#[derive(uniffi::Record)]
pub struct NodeStatus {
    pub id: String,
    pub name: String,
    pub meshes: Vec<MeshStatus>,
    pub peers: Vec<MeshPeer>,
    pub ticket_pending: bool,
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
            .ok_or(FfiError::NodeClosed)
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

    pub fn status(&self) -> Result<NodeStatus, FfiError> {
        let node = self.active()?;
        let info = node.info();
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
        Ok(NodeStatus {
            id: info.id.to_string(),
            name: info.name,
            ticket_pending: node.has_pending_ticket(),
            meshes: info
                .meshes
                .into_iter()
                .map(|mesh| MeshStatus {
                    id: mesh.id.to_string(),
                    name: mesh.name,
                    members: mesh
                        .members
                        .into_iter()
                        .map(|member| MeshMember {
                            id: member.id.to_string(),
                            name: member.name,
                            generation: member.generation,
                        })
                        .collect(),
                })
                .collect(),
            peers,
        })
    }

    pub fn create_mesh(&self, name: String) -> Result<String, FfiError> {
        let node = self.active()?;
        let id = node.new_mesh(&name).map_err(node_error)?;
        Ok(id.to_string())
    }

    pub fn leave_mesh(&self, mesh_id: String) -> Result<LeftMesh, FfiError> {
        let node = self.active()?;
        let mesh_id: MeshId = mesh_id
            .parse()
            .map_err(|error: anyhow::Error| FfiError::Invalid(error.to_string()))?;
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
            .block_on(node.pair(Duration::from_secs(300)))
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

    pub fn add(&self, mesh_id: String, ticket: String) -> Result<String, FfiError> {
        let node = self.active()?;
        let mesh_id: MeshId = mesh_id
            .parse()
            .map_err(|error: anyhow::Error| FfiError::Invalid(error.to_string()))?;
        let ticket = ticket.trim();
        if ticket.len() > 8_192
            || !ticket.starts_with("spirit1")
            || ticket.len() <= 7
            || !ticket[7..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(FfiError::Invalid("invalid pairing ticket".into()));
        }
        let member = runtime()?
            .block_on(node.add(mesh_id, ticket))
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
    fn maps_group_limit_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::MeshLimit.into()),
            FfiError::MeshLimit
        ));
    }

    #[test]
    fn maps_not_member_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::NotMember.into()),
            FfiError::NotMember(_)
        ));
    }

    #[test]
    fn maps_ticket_refusal_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::TicketRejected("refused".into()).into()),
            FfiError::TicketRejected(_)
        ));
    }

    #[test]
    fn maps_node_closed_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::NodeClosed.into()),
            FfiError::NodeClosed
        ));
    }

    #[test]
    fn maps_invalid_input_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::Invalid("bad".into()).into()),
            FfiError::Invalid(_)
        ));
    }

    #[test]
    fn maps_unavailable_by_type() {
        assert!(matches!(
            node_error(spirit_sdk::NodeError::Unavailable("dial failed".into()).into()),
            FfiError::Unavailable(_)
        ));
    }

    #[test]
    fn malformed_mesh_ids_and_closed_node_have_consistent_errors() {
        let dir = tempfile::tempdir().unwrap();
        let node = SpiritNode::open(dir.path().to_str().unwrap().into(), "a".into(), true).unwrap();
        assert!(matches!(
            node.add("bad id".into(), "spirit1valid".into()),
            Err(FfiError::Invalid(_))
        ));
        assert!(matches!(
            node.leave_mesh("bad id".into()),
            Err(FfiError::Invalid(_))
        ));
        node.shutdown().unwrap();
        assert!(matches!(
            node.create_mesh("new".into()),
            Err(FfiError::NodeClosed)
        ));
    }

    #[test]
    fn two_meshes_select_add_status_and_leave() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = SpiritNode::open(a_dir.path().to_str().unwrap().into(), "a".into(), true).unwrap();
        let b = SpiritNode::open(b_dir.path().to_str().unwrap().into(), "b".into(), true).unwrap();
        let first = a.create_mesh("one".into()).unwrap();
        let second = a.create_mesh("two".into()).unwrap();
        let ticket = b.pair().unwrap();
        assert_eq!(a.add(second.clone(), ticket.ticket).unwrap(), "b");
        let status = a.status().unwrap();
        assert_eq!(status.meshes.len(), 2);
        assert!(status
            .meshes
            .iter()
            .any(|mesh| mesh.id == first && mesh.members.len() == 1));
        assert!(status.meshes.iter().any(|mesh| mesh.id == second
            && mesh
                .members
                .iter()
                .any(|member| member.name == "b" && member.generation == 0)));
        assert_eq!(a.status().unwrap().peers.len(), 1);
        let still_valid = b.pair().unwrap();
        b.create_mesh("three".into()).unwrap();
        assert_eq!(a.add(first.clone(), still_valid.ticket).unwrap(), "b");
        assert_eq!(a.status().unwrap().peers.len(), 1);
        assert_eq!(a.leave_mesh(first).unwrap().mesh_name, "one");
        assert_eq!(a.status().unwrap().meshes.len(), 1);
        a.shutdown().unwrap();
        b.shutdown().unwrap();
    }

    #[test]
    fn creating_first_mesh_withdraws_ticket() {
        let a_dir = tempfile::tempdir().unwrap();
        let b_dir = tempfile::tempdir().unwrap();
        let a = SpiritNode::open(a_dir.path().to_str().unwrap().into(), "a".into(), true).unwrap();
        let b = SpiritNode::open(b_dir.path().to_str().unwrap().into(), "b".into(), true).unwrap();
        let ticket = b.pair().unwrap();
        assert!(b.status().unwrap().ticket_pending);
        b.create_mesh("own".into()).unwrap();
        assert!(!b.status().unwrap().ticket_pending);
        let mesh = a.create_mesh("other".into()).unwrap();
        assert!(matches!(
            a.add(mesh, ticket.ticket),
            Err(FfiError::TicketRejected(_))
        ));
        a.shutdown().unwrap();
        b.shutdown().unwrap();
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
        let mesh = a.create_mesh("personal".into()).unwrap();
        let code = b.pair().unwrap();
        assert_eq!(code.modules.len(), (code.width * code.width) as usize);
        assert_eq!(a.add(mesh.clone(), code.ticket).unwrap(), "phone");
        assert_eq!(a.ping("phone".into()).unwrap().name, "phone");
        assert!(a.status().unwrap().peers[0].connected);
        assert!(b.status().unwrap().peers[0].connected);
        let left = b.leave_mesh(mesh.clone()).unwrap();
        assert_eq!(left.mesh_name, "personal");
        assert_eq!((left.remaining_members, left.notified_members), (1, 1));
        assert!(b.status().unwrap().meshes.is_empty());
        assert!(a.status().unwrap().peers.is_empty());
        assert!(matches!(
            b.leave_mesh(mesh.clone()),
            Err(FfiError::NotMember(_))
        ));
        a.shutdown().unwrap();
        b.shutdown().unwrap();
        assert!(matches!(a.status(), Err(FfiError::NodeClosed)));
        let reopened = SpiritNode::open(
            a_dir.path().to_str().unwrap().into(),
            "desktop".into(),
            true,
        )
        .unwrap();
        assert_eq!(reopened.status().unwrap().meshes[0].name, "personal");
        reopened.shutdown().unwrap();
    }
}
