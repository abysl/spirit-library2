use anyhow::{ensure, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use spirit_sdk::{LeftMesh, Member, MeshId, Node, NodeConfig, NodeInfo, Pong};
use std::{
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

const MAX_REQUEST: usize = 256 * 1024;
const MAX_REPLY: usize = 16 * 1024 * 1024;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize)]
pub enum Operation {
    Info,
    Create { name: String },
    Pair { ttl_seconds: u64 },
    Add { mesh_id: MeshId, ticket: String },
    Ping { device: String },
    Leave { mesh_id: MeshId },
}

#[derive(Serialize, Deserialize)]
pub enum Reply {
    Info(NodeInfo),
    Created(MeshId),
    Ticket(String),
    Added(Member),
    Pong(Pong),
    Left(LeftMesh),
}

#[derive(Serialize, Deserialize)]
struct ControlFile {
    address: SocketAddr,
    token: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: [u8; 32],
    operation: Operation,
}

#[derive(Serialize, Deserialize)]
struct Response {
    result: std::result::Result<Reply, String>,
}

struct ControlGuard(PathBuf);

impl Drop for ControlGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn read_frame<T: DeserializeOwned>(stream: &mut TcpStream, limit: usize) -> Result<T> {
    let size = stream.read_u32().await? as usize;
    ensure!(size <= limit, "local control frame is too large");
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write_frame<T: Serialize>(stream: &mut TcpStream, value: &T, limit: usize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= limit, "local control response is too large");
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

pub async fn request_if_running(root: &Path, operation: Operation) -> Result<Option<Reply>> {
    let bytes = match std::fs::read(root.join("control.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let control: ControlFile = serde_json::from_slice(&bytes)?;
    ensure!(
        control.address.ip().is_loopback(),
        "local control address must be loopback"
    );
    let stream = tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(control.address))
        .await
        .context("local node did not respond")?;
    let mut stream = match stream {
        Ok(stream) => stream,
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let response: Response = tokio::time::timeout(CONTROL_TIMEOUT, async {
        write_frame(
            &mut stream,
            &Request {
                token: control.token,
                operation,
            },
            MAX_REQUEST,
        )
        .await?;
        read_frame(&mut stream, MAX_REPLY).await
    })
    .await
    .context("local node operation timed out")??;
    Ok(Some(response.result.map_err(anyhow::Error::msg)?))
}

pub async fn request(root: &Path, operation: Operation) -> Result<Reply> {
    request_if_running(root, operation)
        .await?
        .context("node is not running; start spirit node serve in another terminal")
}

async fn execute(node: &Node, operation: Operation) -> Result<Reply> {
    match operation {
        Operation::Info => Ok(Reply::Info(node.info())),
        Operation::Create { name } => Ok(Reply::Created(node.new_mesh(&name)?)),
        Operation::Pair { ttl_seconds } => Ok(Reply::Ticket(
            node.pair(Duration::from_secs(ttl_seconds)).await?,
        )),
        Operation::Add { mesh_id, ticket } => Ok(Reply::Added(node.add(mesh_id, &ticket).await?)),
        Operation::Ping { device } => {
            let member = node.info().resolve(&device)?;
            Ok(Reply::Pong(node.ping(member.id).await?))
        }
        Operation::Leave { mesh_id } => Ok(Reply::Left(node.leave(mesh_id).await?)),
    }
}

async fn handle(mut stream: TcpStream, token: [u8; 32], node: Arc<Node>) -> Result<()> {
    let request: Request = read_frame(&mut stream, MAX_REQUEST).await?;
    ensure!(
        bool::from(token.ct_eq(&request.token)),
        "invalid local control credential"
    );
    let result = execute(&node, request.operation)
        .await
        .map_err(|error| format!("{error:#}"));
    write_frame(&mut stream, &Response { result }, MAX_REPLY).await
}

async fn interrupted() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

pub async fn serve(root: PathBuf, local: bool) -> Result<()> {
    let config = if local {
        NodeConfig::local()
    } else {
        NodeConfig::default()
    };
    let node = Arc::new(Node::bind(&root, config).await?);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let mut token = [0; 32];
    getrandom::fill(&mut token)
        .map_err(|error| anyhow::anyhow!("could not generate control credential: {error}"))?;
    let control = ControlFile {
        address: listener.local_addr()?,
        token,
    };
    let path = root.join("control.json");
    spirit_sdk::write_private(&path, &serde_json::to_vec(&control)?)?;
    let _guard = ControlGuard(path);
    println!(
        "Serving {}{}. Press Ctrl-C to stop.",
        node.info().name,
        if local { " on loopback" } else { "" }
    );
    std::io::stdout().flush()?;
    let mut tasks = JoinSet::new();
    let shutdown = interrupted();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            result = &mut shutdown => break result,
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) if tasks.len() < 32 => {
                        let node = node.clone();
                        tasks.spawn(async move {
                            let _ = tokio::time::timeout(CONTROL_TIMEOUT, handle(stream, token, node)).await;
                        });
                    }
                    Ok(_) => {}
                    Err(error) => break Err(error.into()),
                }
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if result.is_err() { break Err(anyhow::anyhow!("local control task failed")); }
            }
        }
    };
    tasks.shutdown().await;
    node.shutdown().await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn two_mesh_info_response_fits_maximum_membership_bounds() {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "root").unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        let Reply::Created(first) = execute(
            &node,
            Operation::Create {
                name: "first".into(),
            },
        )
        .await
        .unwrap() else {
            panic!("expected created")
        };
        let Reply::Created(second) = execute(
            &node,
            Operation::Create {
                name: "second".into(),
            },
        )
        .await
        .unwrap() else {
            panic!("expected created")
        };
        assert_ne!(first, second);
        let Reply::Info(mut info) = execute(&node, Operation::Info).await.unwrap() else {
            panic!("expected info")
        };
        assert_eq!(info.meshes.len(), 2);
        let mut previous = serde_json::to_value(&info).unwrap();
        previous.as_object_mut().unwrap().remove("meshes");
        let restored: NodeInfo = serde_json::from_value(previous).unwrap();
        assert!(restored.meshes.is_empty());
        let mut meshes = Vec::new();
        let mut members = std::collections::BTreeMap::new();
        for mesh_index in 0..64 {
            let mut mesh = info.meshes[0].clone();
            mesh.id = format!(
                "mesh1_{}{}",
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"[mesh_index]
                    as char,
                "A".repeat(42)
            )
            .parse()
            .unwrap();
            mesh.name = "\"".repeat(128);
            mesh.members = (0..256)
                .map(|_| {
                    let id = iroh::SecretKey::generate().public();
                    let member = spirit_sdk::MeshMember {
                        id,
                        name: "\"".repeat(128),
                        generation: 0,
                    };
                    members.insert(
                        id,
                        spirit_sdk::Member {
                            id,
                            name: member.name.clone(),
                        },
                    );
                    member
                })
                .collect();
            meshes.push(mesh);
        }
        info.meshes = meshes;
        info.members = members.into_values().collect();
        let response = serde_json::to_vec(&Response {
            result: Ok(Reply::Info(info)),
        })
        .unwrap();
        assert!(response.len() > MAX_REQUEST);
        assert!(response.len() < MAX_REPLY);
        println!("worst-case control reply: {} bytes", response.len());
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn local_control_rejects_an_invalid_credential() {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "desktop").unwrap();
        let node = Arc::new(Node::bind(dir.path(), NodeConfig::local()).await.unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        write_frame(
            &mut client,
            &Request {
                token: [0; 32],
                operation: Operation::Pair { ttl_seconds: 60 },
            },
            MAX_REQUEST,
        )
        .await
        .unwrap();
        let error = handle(server, [1; 32], node.clone()).await.unwrap_err();
        assert!(error.to_string().contains("credential"));
        assert!(read_frame::<Response>(&mut client, MAX_REPLY)
            .await
            .is_err());
        node.shutdown().await.unwrap();
    }
}
