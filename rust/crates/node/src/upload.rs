use crate::{
    network::{finish_incoming_result, Shared},
    MeshId, NodeError, NodeId,
};
use anyhow::{ensure, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use serde::{Deserialize, Serialize};
use spirit_core::BlobHash;
use std::{fmt, io::Read, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};

pub(crate) const BLOB_ALPN: &[u8] = b"spirit/blob/1";
pub(crate) const MAX_SHARES_PER_MESH: usize = 65_536;
pub(crate) const MAX_UPLOADS: usize = 4;
const MAX_UPLOADS_PER_PEER: usize = 2;
const CHUNK: usize = 64 * 1024;
pub(crate) const REFUSAL_CLOSE_WAIT: Duration = Duration::from_millis(100);
pub(crate) const CHANNEL_CAPACITY: usize = 2;
pub(crate) const CHUNK_IDLE: Duration = Duration::from_secs(30);
pub(crate) const REFUSED: u8 = 0;
pub(crate) const AVAILABLE: u8 = 1;

#[derive(Serialize, Deserialize)]
pub(crate) struct BlobRequest {
    pub(crate) mesh_id: MeshId,
    pub(crate) hash: String,
}

struct UploadSlot {
    shared: Arc<Shared>,
    peer: NodeId,
}

impl UploadSlot {
    fn acquire(shared: &Arc<Shared>, peer: NodeId) -> Result<Self> {
        let mut uploads = shared.uploads.lock().unwrap();
        ensure!(
            uploads.values().sum::<usize>() < MAX_UPLOADS,
            "node upload limit exceeded"
        );
        let count = uploads.entry(peer).or_default();
        ensure!(*count < MAX_UPLOADS_PER_PEER, "peer upload limit exceeded");
        *count += 1;
        Ok(Self {
            shared: shared.clone(),
            peer,
        })
    }
}

impl Drop for UploadSlot {
    fn drop(&mut self) {
        let mut uploads = self.shared.uploads.lock().unwrap();
        let count = uploads.get_mut(&self.peer).unwrap();
        *count -= 1;
        if *count == 0 {
            uploads.remove(&self.peer);
        }
    }
}

pub(crate) struct BlobProtocol(pub Arc<Shared>);

impl fmt::Debug for BlobProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BlobProtocol")
    }
}

impl BlobProtocol {
    async fn serve(&self, connection: &Connection, mesh: &mut Option<MeshId>) -> Result<()> {
        let remote = connection.remote_id();
        let deadline = tokio::time::Instant::now() + self.0.config.request_timeout;
        let (mut send, mut recv) =
            tokio::time::timeout_at(deadline, connection.accept_bi()).await??;
        let mut header_sent = false;
        let result: Result<()> = async {
            let (mut chunk_rx, slot) = tokio::time::timeout_at(deadline, async {
                let bytes = recv.read_to_end(256).await?;
                let request: BlobRequest = serde_json::from_slice(&bytes)?;
                *mesh = Some(request.mesh_id);
                let hash: BlobHash = request.hash.parse()?;
                ensure!(
                    self.0.both_current_members(request.mesh_id, remote),
                    NodeError::NotMember
                );
                ensure!(
                    self.0
                        .shares
                        .lock()
                        .unwrap()
                        .get(&request.mesh_id)
                        .is_some_and(|set| set.contains(&hash)),
                    "hash is not shared in this mesh"
                );
                let store = self.0.store.clone().ok_or(NodeError::StoreNotConfigured)?;
                let slot = UploadSlot::acquire(&self.0, remote)?;
                let (size_tx, size_rx) = oneshot::channel();
                let (chunk_tx, chunk_rx) = mpsc::channel(CHANNEL_CAPACITY);
                tokio::task::spawn_blocking(move || {
                    let opened = store.open_reader(hash);
                    match opened {
                        Ok((size, mut reader)) => {
                            if size_tx.send(Ok(size)).is_err() {
                                return;
                            }
                            loop {
                                let mut chunk = vec![0; CHUNK];
                                match reader.read(&mut chunk) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        chunk.truncate(n);
                                        if chunk_tx.blocking_send(Ok(chunk)).is_err() {
                                            break;
                                        }
                                    }
                                    Err(error) => {
                                        let _ = chunk_tx.blocking_send(Err(error));
                                        break;
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            let _ = size_tx.send(Err(error));
                        }
                    }
                });
                let size = size_rx.await??;
                send.write_all(&[AVAILABLE]).await?;
                send.write_all(&size.to_be_bytes()).await?;
                header_sent = true;
                Ok::<_, anyhow::Error>((chunk_rx, slot))
            })
            .await??;
            while let Some(chunk) = chunk_rx.recv().await {
                tokio::time::timeout(CHUNK_IDLE, send.write_all(&chunk?)).await??;
            }
            send.finish()?;
            drop(slot);
            Ok(())
        }
        .await;
        if result.is_err() {
            if header_sent {
                let _ = send.reset(0u32.into());
            } else {
                let _ = tokio::time::timeout(REFUSAL_CLOSE_WAIT, send.write_all(&[REFUSED])).await;
                let _ = send.finish();
            }
        }
        result
    }
}

impl ProtocolHandler for BlobProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if !self.0.is_member(connection.remote_id()) {
            connection.close(0u32.into(), b"finished");
            return Ok(());
        }
        let mut mesh = None;
        let result = self.serve(&connection, &mut mesh).await;
        if result.is_ok() {
            let _ = tokio::time::timeout(CHUNK_IDLE, connection.closed()).await;
        } else {
            let _ = tokio::time::timeout(REFUSAL_CLOSE_WAIT, connection.closed()).await;
        }
        finish_incoming_result(&connection, result, |error| {
            self.0.diagnostics.record(
                "spirit/blob/1",
                mesh,
                connection.remote_id(),
                None,
                &format!("{error:#}"),
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, NodeConfig};
    use tempfile::TempDir;

    async fn device(name: &str) -> (TempDir, Node) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), name).unwrap();
        let node = Node::bind(
            dir.path(),
            NodeConfig::local().with_store(dir.path().join("store")),
        )
        .await
        .unwrap();
        node.gossip.abort();
        (dir, node)
    }

    async fn add(provider: &Node, mesh: MeshId, peer: &Node) {
        let ticket = peer.pair(Duration::from_secs(60)).await.unwrap();
        provider.add(mesh, &ticket).await.unwrap();
    }

    fn data(mebibytes: usize) -> Vec<u8> {
        (0..mebibytes * 1024 * 1024)
            .map(|i| (i.wrapping_mul(37) >> 9) as u8)
            .collect()
    }

    async fn raw_status(requester: &Node, provider: &Node, mesh: MeshId, hash: BlobHash) -> u8 {
        let result: Result<u8> = async {
            let connection = requester
                .shared
                .endpoint
                .connect(provider.shared.endpoint.addr(), BLOB_ALPN)
                .await?;
            let (mut send, mut recv) = connection.open_bi().await?;
            send.write_all(&serde_json::to_vec(&BlobRequest {
                mesh_id: mesh,
                hash: hash.to_string(),
            })?)
            .await?;
            send.finish()?;
            let mut status = [9];
            recv.read_exact(&mut status).await?;
            connection.close(0u32.into(), b"done");
            Ok(status[0])
        }
        .await;
        result.unwrap_or(255)
    }

    #[tokio::test]
    async fn isolation_unshared_departed_and_missing_are_identical() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        let m1 = a.new_mesh("m1").unwrap();
        let m2 = a.new_mesh("m2").unwrap();
        add(&a, m1, &b).await;
        add(&a, m2, &c).await;
        let shared = a.import_reader(data(2).as_slice()).unwrap();
        let unshared = a.import_reader(&b"unshared"[..]).unwrap();
        let absent = BlobHash::of(b"missing");
        a.share(m1, shared).unwrap();
        a.share(m1, absent).unwrap();
        for (peer, mesh, hash) in [(&c, m2, shared), (&b, m1, unshared), (&b, m1, absent)] {
            assert_eq!(raw_status(peer, &a, mesh, hash).await, REFUSED);
        }
        assert_eq!(raw_status(&c, &a, m1, shared).await, REFUSED);
        b.leave(m1).await.unwrap();
        assert_eq!(raw_status(&b, &a, m1, shared).await, 255);
        a.unshare(m1, shared).unwrap();
        assert_eq!(raw_status(&c, &a, m1, shared).await, REFUSED);
        a.leave(m1).await.unwrap();
        assert!(!a.shared.shares.lock().unwrap().contains_key(&m1));
        assert!(a.share(m1, shared).is_err());
    }

    #[tokio::test]
    async fn upload_limits_are_global_and_per_peer() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let (_c_dir, c) = device("c").await;
        let (_d_dir, d) = device("d").await;
        let mesh = a.new_mesh("m").unwrap();
        for peer in [&b, &c, &d] {
            add(&a, mesh, peer).await;
        }
        let hash = a.import_reader(data(12).as_slice()).unwrap();
        a.share(mesh, hash).unwrap();
        let mut held = Vec::new();
        for peer in [&b, &b, &c, &c] {
            let connection = peer
                .shared
                .endpoint
                .connect(a.shared.endpoint.addr(), BLOB_ALPN)
                .await
                .unwrap();
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(
                &serde_json::to_vec(&BlobRequest {
                    mesh_id: mesh,
                    hash: hash.to_string(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
            send.finish().unwrap();
            let mut header = [0; 9];
            recv.read_exact(&mut header).await.unwrap();
            assert_eq!(header[0], AVAILABLE);
            held.push((connection, recv));
            if held.len() == 2 {
                assert_eq!(raw_status(&b, &a, mesh, hash).await, REFUSED);
            }
        }
        assert_eq!(raw_status(&b, &a, mesh, hash).await, REFUSED);
        assert_eq!(raw_status(&d, &a, mesh, hash).await, REFUSED);
        assert_eq!(
            a.shared.uploads.lock().unwrap().values().sum::<usize>(),
            MAX_UPLOADS
        );
        for (connection, _) in held {
            connection.close(0u32.into(), b"done");
        }
    }

    #[tokio::test]
    async fn blob_refusals_do_not_pollute_presence_or_upload_slots() {
        let (_a_dir, a) = device("provider").await;
        let (_b_dir, b) = device("member").await;
        let (_c_dir, outsider) = device("outsider").await;
        let mesh = a.new_mesh("m").unwrap();
        add(&a, mesh, &b).await;
        let unknown = outsider.new_mesh("unknown").unwrap();
        let hash = a.import_reader(&b"shared"[..]).unwrap();
        a.share(mesh, hash).unwrap();
        let prior = a
            .peers()
            .into_iter()
            .find(|p| p.id == b.info().id)
            .unwrap()
            .last_error;
        assert_eq!(raw_status(&b, &a, unknown, hash).await, REFUSED);
        assert_eq!(
            raw_status(&b, &a, mesh, BlobHash::of(b"missing")).await,
            REFUSED
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if a.diagnostics().iter().any(|d| d.channel == "spirit/blob/1") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            a.peers()
                .into_iter()
                .find(|p| p.id == b.info().id)
                .unwrap()
                .last_error,
            prior
        );
        let mut attempts = tokio::task::JoinSet::new();
        for _ in 0..32 {
            let endpoint = outsider.shared.endpoint.clone();
            let address = a.shared.endpoint.addr();
            attempts.spawn(async move {
                if let Ok(connection) = endpoint.connect(address, BLOB_ALPN).await {
                    let _ = connection.open_bi().await;
                    connection.close(0u32.into(), b"done");
                }
            });
        }
        while let Some(result) = attempts.join_next().await {
            result.unwrap();
        }
        assert!(a.shared.uploads.lock().unwrap().is_empty());
        assert_eq!(raw_status(&outsider, &a, mesh, hash).await, 255);
        assert_eq!(raw_status(&b, &a, mesh, hash).await, AVAILABLE);
        assert!(a
            .diagnostics()
            .iter()
            .any(|item| item.channel == "spirit/blob/1" && item.mesh == Some(mesh)));
    }

    #[tokio::test]
    async fn shares_require_store_and_current_membership_and_clear_on_leave() {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "owner").unwrap();
        let node = Node::bind(dir.path(), NodeConfig::local()).await.unwrap();
        let mesh = node.new_mesh("m").unwrap();
        assert!(node.import_reader(&b"bytes"[..]).is_err());
        assert!(matches!(
            node.share(mesh, BlobHash::of(b"bytes"))
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::StoreNotConfigured)
        ));
        node.shutdown().await.unwrap();
        assert!(matches!(
            node.import_reader(&b"bytes"[..])
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::NodeClosed)
        ));
        drop(node);
        let node = Node::bind(
            dir.path(),
            NodeConfig::local().with_store(dir.path().join("store")),
        )
        .await
        .unwrap();
        let hash = node.import_reader(&b"bytes"[..]).unwrap();
        assert_eq!(node.blob_size(hash).unwrap(), 5);
        let other = MeshId::generate().unwrap();
        assert!(node.share(other, hash).is_err());
        node.set_shares(mesh, [hash]).unwrap();
        assert!(node.shared.shares.lock().unwrap()[&mesh].contains(&hash));
        let many: Vec<_> = (0..=MAX_SHARES_PER_MESH)
            .map(|n| BlobHash::of(&n.to_be_bytes()))
            .collect();
        assert!(matches!(
            node.set_shares(mesh, many.iter().copied())
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::Invalid(_))
        ));
        node.set_shares(mesh, many[..MAX_SHARES_PER_MESH].iter().copied())
            .unwrap();
        assert!(matches!(
            node.share(mesh, many[MAX_SHARES_PER_MESH])
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::Invalid(_))
        ));
        node.leave(mesh).await.unwrap();
        assert!(!node.shared.shares.lock().unwrap().contains_key(&mesh));
        assert!(node.share(mesh, hash).is_err());
        assert!(node.has_blob(hash).unwrap());
        node.shutdown().await.unwrap();
        assert!(matches!(
            node.import_reader(&b"bytes"[..])
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::NodeClosed)
        ));
        assert!(matches!(
            node.export_file(hash, dir.path().join("out"))
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::NodeClosed)
        ));
        assert!(matches!(
            node.share(mesh, hash).unwrap_err().downcast::<NodeError>(),
            Ok(NodeError::NodeClosed)
        ));
        assert!(matches!(
            node.set_shares(mesh, [hash])
                .unwrap_err()
                .downcast::<NodeError>(),
            Ok(NodeError::NodeClosed)
        ));
    }
}
