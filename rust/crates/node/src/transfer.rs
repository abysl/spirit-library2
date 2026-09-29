use crate::{
    network::Shared,
    upload::{BlobRequest, AVAILABLE, BLOB_ALPN, CHANNEL_CAPACITY, CHUNK_IDLE},
    MeshId, NodeError, NodeId,
};
use iroh::endpoint::Connection;
use spirit_core::{BlobHash, BlobStore, StoreError};
use std::{
    io::{self, Read},
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::sync::{mpsc, watch};

pub(crate) const MAX_FETCHES: usize = 4;
const RECEIVE_BUFFER: usize = 64 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const PROGRESS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error("fetch cancelled")]
    Cancelled,
    #[error("blob transfer interrupted")]
    Interrupted,
    #[error("blob is corrupt")]
    Corrupt,
    #[error("transfer timed out")]
    Timeout,
    #[error("blob transfer I/O: {0}")]
    Io(#[from] io::Error),
}

enum Piece {
    Bytes(Vec<u8>),
    Complete,
}

struct ChannelReader {
    receiver: mpsc::Receiver<Piece>,
    current: io::Cursor<Vec<u8>>,
}

impl Read for ChannelReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.current.read(output)?;
            if n != 0 {
                return Ok(n);
            }
            match self.receiver.blocking_recv() {
                Some(Piece::Bytes(bytes)) => self.current = io::Cursor::new(bytes),
                Some(Piece::Complete) => return Ok(0),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "blob stream interrupted",
                    ))
                }
            }
        }
    }
}

pub(crate) async fn fetch(
    shared: &Arc<Shared>,
    mesh: MeshId,
    provider: NodeId,
    hash: BlobHash,
    expected_size: Option<u64>,
    progress: impl Fn(u64, u64),
    cancel: &mut watch::Receiver<bool>,
) -> std::result::Result<u64, FetchError> {
    if !shared.both_current_members(mesh, provider) {
        return Err(NodeError::NotMember.into());
    }
    let store = shared.store.clone().ok_or(NodeError::StoreNotConfigured)?;
    if store.has(hash) {
        let size = store.size(hash).map_err(map_store_error)?;
        if expected_size.is_some_and(|expected| expected != size) {
            return Err(FetchError::Corrupt);
        }
        progress(size, size);
        return Ok(size);
    }
    let _permit = tokio::select! { biased; _ = cancelled(cancel) => return Err(FetchError::Cancelled), permit = shared.fetches.clone().acquire_owned() => permit }
        .map_err(|_| FetchError::Node(NodeError::Unavailable("fetch queue closed".into())))?;
    let address = shared.address_for_mesh(provider, mesh);
    let connection = tokio::select! { biased; _ = cancelled(cancel) => return Err(FetchError::Cancelled), result = tokio::time::timeout(shared.config.request_timeout, shared.endpoint.connect(address, BLOB_ALPN)) => result }
    .map_err(|_| unavailable())?
    .map_err(|_| unavailable())?;
    let result = receive(
        &connection,
        store,
        mesh,
        hash,
        expected_size,
        progress,
        cancel,
    )
    .await;
    connection.close(0u32.into(), b"done");
    result
}

async fn receive(
    connection: &Connection,
    store: Arc<BlobStore>,
    mesh: MeshId,
    hash: BlobHash,
    expected_size: Option<u64>,
    progress: impl Fn(u64, u64),
    cancel: &mut watch::Receiver<bool>,
) -> std::result::Result<u64, FetchError> {
    let (mut recv, total) = tokio::select! {
        biased;
        _ = cancelled(cancel) => return Err(FetchError::Cancelled),
        result = open_stream(connection, mesh, hash, expected_size) => result?,
    };
    let (tx, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    let writer = tokio::task::spawn_blocking(move || {
        store.write_verified(
            hash,
            ChannelReader {
                receiver,
                current: io::Cursor::new(Vec::new()),
            },
        )
    });
    let streamed = tokio::select! {
        biased;
        _ = cancelled(cancel) => Err(FetchError::Cancelled),
        result = stream_chunks(&mut recv, total, &tx, &progress) => result,
    };
    drop(tx);
    let written = writer
        .await
        .map_err(|e| FetchError::Io(io::Error::other(e)))?
        .map_err(map_store_error);
    let reported = match streamed {
        Ok(reported) => reported,
        Err(error) if matches!(error, FetchError::Io(_)) => {
            return Err(written.err().unwrap_or(error));
        }
        Err(error) => return Err(error),
    };
    let size = written?;
    if reported != size {
        progress(size, total);
    }
    Ok(size)
}

async fn open_stream(
    connection: &Connection,
    mesh: MeshId,
    hash: BlobHash,
    expected_size: Option<u64>,
) -> std::result::Result<(noq::RecvStream, u64), FetchError> {
    let (mut send, mut recv) = tokio::time::timeout(CHUNK_IDLE, connection.open_bi())
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| unavailable())?;
    let request = serde_json::to_vec(&BlobRequest {
        mesh_id: mesh,
        hash: hash.to_string(),
    })
    .map_err(|e| FetchError::Io(io::Error::other(e)))?;
    tokio::time::timeout(CHUNK_IDLE, send.write_all(&request))
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| unavailable())?;
    send.finish().map_err(|_| unavailable())?;
    let mut status = [0];
    tokio::time::timeout(CHUNK_IDLE, recv.read_exact(&mut status))
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| unavailable())?;
    if status[0] != AVAILABLE {
        return Err(unavailable());
    }
    let mut header = [0; 8];
    tokio::time::timeout(CHUNK_IDLE, recv.read_exact(&mut header))
        .await
        .map_err(|_| FetchError::Timeout)?
        .map_err(|_| FetchError::Interrupted)?;
    let total = u64::from_be_bytes(header);
    if expected_size.is_some_and(|expected| expected != total) {
        return Err(FetchError::Corrupt);
    }
    Ok((recv, total))
}

async fn stream_chunks(
    recv: &mut noq::RecvStream,
    total: u64,
    tx: &mpsc::Sender<Piece>,
    progress: &impl Fn(u64, u64),
) -> std::result::Result<u64, FetchError> {
    let mut received = 0u64;
    let mut reported = 0u64;
    let mut last = Instant::now();
    let mut buffer = vec![0; RECEIVE_BUFFER];
    loop {
        let chunk = tokio::time::timeout(CHUNK_IDLE, recv.read(&mut buffer))
            .await
            .map_err(|_| FetchError::Timeout)?
            .map_err(|_| FetchError::Interrupted)?;
        let Some(n) = chunk else {
            break;
        };
        received = received.checked_add(n as u64).ok_or(FetchError::Corrupt)?;
        if received > total {
            return Err(FetchError::Corrupt);
        }
        tx.send(Piece::Bytes(buffer[..n].to_vec()))
            .await
            .map_err(|_| FetchError::Io(io::Error::other("store writer stopped")))?;
        if received - reported >= PROGRESS_BYTES || last.elapsed() >= PROGRESS_INTERVAL {
            progress(received, total);
            reported = received;
            last = Instant::now();
        }
    }
    if received != total {
        return Err(FetchError::Interrupted);
    }
    tx.send(Piece::Complete)
        .await
        .map_err(|_| FetchError::Io(io::Error::other("store writer stopped")))?;
    Ok(reported)
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    while !*cancel.borrow() {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn unavailable() -> FetchError {
    NodeError::Unavailable("blob unavailable".into()).into()
}

fn map_store_error(error: StoreError) -> FetchError {
    match error {
        StoreError::Corrupt { .. } => FetchError::Corrupt,
        StoreError::NotFound(_) => unavailable(),
        StoreError::Io(error) | StoreError::Destination(error) => FetchError::Io(error),
        StoreError::InvalidHash(error) => FetchError::Io(io::Error::other(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{upload::REFUSAL_CLOSE_WAIT, Node, NodeConfig};
    use anyhow::Result;
    use iroh::protocol::{AcceptError, ProtocolHandler};
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    #[tokio::test]
    async fn authorized_fetch_and_local_hit() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let mesh = a.new_mesh("m").unwrap();
        add(&a, mesh, &b).await;
        let bytes = data(5);
        let hash = a.import_reader(bytes.as_slice()).unwrap();
        a.set_shares(mesh, [hash]).unwrap();
        let count = AtomicUsize::new(0);
        let size = b
            .fetch(mesh, a.info().id, hash, None, |received, total| {
                assert!(received <= total);
                count.fetch_add(1, Ordering::Relaxed);
            })
            .await
            .unwrap();
        assert_eq!(size, bytes.len() as u64);
        assert!(count.load(Ordering::Relaxed) > 0);
        assert_eq!(
            std::fs::read(b.shared.store.as_ref().unwrap().path_of(hash)).unwrap(),
            bytes
        );
        a.shutdown().await.unwrap();
        let local = AtomicUsize::new(0);
        assert_eq!(
            b.fetch(mesh, a.info().id, hash, None, |received, total| {
                assert_eq!((received, total), (size, size));
                local.fetch_add(1, Ordering::Relaxed);
            })
            .await
            .unwrap(),
            size
        );
        assert_eq!(local.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn local_hit_checks_the_expected_size_and_closed_node() {
        let (_dir, node) = device("receiver").await;
        let mesh = node.new_mesh("mesh").unwrap();
        let hash = node.import_reader(&b"local"[..]).unwrap();
        assert!(matches!(
            node.fetch(mesh, node.info().id, hash, Some(6), |_, _| {})
                .await,
            Err(FetchError::Corrupt)
        ));
        assert_eq!(
            node.fetch(mesh, node.info().id, hash, Some(5), |_, _| {})
                .await
                .unwrap(),
            5
        );
        node.shutdown().await.unwrap();
        assert!(matches!(
            node.fetch(mesh, node.info().id, hash, Some(5), |_, _| {})
                .await,
            Err(FetchError::Node(NodeError::NodeClosed))
        ));
    }

    #[tokio::test]
    async fn fifth_fetch_waits_and_cancelling_a_waiter_does_not_release_a_permit() {
        let (_a_dir, a) = device("provider").await;
        let (_b_dir, b) = device("requester").await;
        let mesh = a.new_mesh("mesh").unwrap();
        add(&a, mesh, &b).await;
        let hash = a.import_reader(data(1).as_slice()).unwrap();
        a.share(mesh, hash).unwrap();
        let permits: Vec<_> = (0..MAX_FETCHES)
            .map(|_| b.shared.fetches.clone().try_acquire_owned().unwrap())
            .collect();
        {
            let cancelled = b.fetch(mesh, a.info().id, hash, None, |_, _| {});
            tokio::pin!(cancelled);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), &mut cancelled)
                    .await
                    .is_err()
            );
        }
        assert_eq!(b.shared.fetches.available_permits(), 0);
        let fifth = b.fetch(mesh, a.info().id, hash, None, |_, _| {});
        tokio::pin!(fifth);
        assert!(tokio::time::timeout(Duration::from_millis(100), &mut fifth)
            .await
            .is_err());
        assert_eq!(b.shared.fetches.available_permits(), 0);
        drop(permits);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), fifth)
                .await
                .unwrap()
                .unwrap(),
            1024 * 1024
        );
        assert_eq!(b.shared.fetches.available_permits(), MAX_FETCHES);
    }

    #[tokio::test]
    async fn corrupt_provider_never_installs_file() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let mesh = a.new_mesh("m").unwrap();
        add(&a, mesh, &b).await;
        let bytes = data(3);
        let hash = a.import_reader(bytes.as_slice()).unwrap();
        a.share(mesh, hash).unwrap();
        std::fs::write(
            a.shared.store.as_ref().unwrap().path_of(hash),
            vec![99; bytes.len()],
        )
        .unwrap();
        assert!(matches!(
            b.fetch(mesh, a.info().id, hash, None, |_, _| {}).await,
            Err(FetchError::Corrupt)
        ));
        let store = b.shared.store.as_ref().unwrap();
        assert!(!store.has(hash));
        assert_eq!(
            std::fs::read_dir(store.root().join("tmp")).unwrap().count(),
            0
        );
    }

    #[tokio::test]
    async fn cancel_frees_upload_slot_and_temp_file() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let mesh = a.new_mesh("m").unwrap();
        add(&a, mesh, &b).await;
        let hash = a.import_reader(data(16).as_slice()).unwrap();
        a.share(mesh, hash).unwrap();
        let (tx, rx) = oneshot::channel();
        let signal = std::sync::Mutex::new(Some(tx));
        {
            let fetch = b.fetch(mesh, a.info().id, hash, None, |_, _| {
                if let Some(tx) = signal.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            });
            tokio::pin!(fetch);
            tokio::select! { _ = &mut fetch => panic!("fetch completed before cancellation"), _ = rx => {} }
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if a.shared.uploads.lock().unwrap().is_empty()
                    && std::fs::read_dir(b.shared.store.as_ref().unwrap().root().join("tmp"))
                        .unwrap()
                        .count()
                        == 0
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(!b.shared.store.as_ref().unwrap().has(hash));
        assert_eq!(
            b.fetch(mesh, a.info().id, hash, None, |_, _| {})
                .await
                .unwrap(),
            16 * 1024 * 1024
        );
    }

    #[derive(Debug)]
    struct ScriptedBlob {
        size: u64,
        bytes: Vec<u8>,
        reset: bool,
    }

    impl ProtocolHandler for ScriptedBlob {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let result: Result<()> = async {
                let (mut send, mut recv) = connection.accept_bi().await?;
                recv.read_to_end(256).await?;
                send.write_all(&[AVAILABLE]).await?;
                send.write_all(&self.size.to_be_bytes()).await?;
                send.write_all(&self.bytes).await?;
                if self.reset {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    send.reset(0u32.into())?;
                } else {
                    send.finish()?;
                }
                let _ = tokio::time::timeout(REFUSAL_CLOSE_WAIT, connection.closed()).await;
                Ok(())
            }
            .await;
            result.map_err(|error| AcceptError::from_boxed(error.into()))
        }
    }

    async fn scripted_fetch(
        node: &Node,
        mesh: MeshId,
        hash: BlobHash,
        size: u64,
        bytes: Vec<u8>,
        reset: bool,
        expected_size: Option<u64>,
    ) -> std::result::Result<u64, FetchError> {
        let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .unwrap()
            .secret_key(iroh::SecretKey::generate())
            .bind()
            .await
            .unwrap();
        let router = iroh::protocol::Router::builder(endpoint.clone())
            .accept(BLOB_ALPN, ScriptedBlob { size, bytes, reset })
            .spawn();
        let connection = node
            .shared
            .endpoint
            .connect(endpoint.addr(), BLOB_ALPN)
            .await
            .unwrap();
        let (_sender, mut cancel) = watch::channel(false);
        let result = receive(
            &connection,
            node.shared.store.as_ref().unwrap().clone(),
            mesh,
            hash,
            expected_size,
            |_, _| {},
            &mut cancel,
        )
        .await;
        connection.close(0u32.into(), b"done");
        router.shutdown().await.unwrap();
        result
    }

    #[tokio::test]
    async fn size_mismatch_and_interrupted_streams_leave_no_temp_files() {
        let (_dir, node) = device("receiver").await;
        let mesh = node.new_mesh("m").unwrap();
        let body = b"the full blob is longer".to_vec();
        let hash = BlobHash::of(&body);
        for (header, sent, reset, known, class) in [
            (
                body.len() as u64 + 1,
                body.clone(),
                false,
                Some(body.len() as u64),
                "corrupt",
            ),
            (body.len() as u64 - 1, body.clone(), false, None, "corrupt"),
            (
                body.len() as u64 + 1,
                body.clone(),
                false,
                None,
                "interrupted",
            ),
            (
                body.len() as u64,
                body[..4].to_vec(),
                true,
                None,
                "interrupted",
            ),
        ] {
            let result = scripted_fetch(&node, mesh, hash, header, sent, reset, known).await;
            assert!(
                match class {
                    "corrupt" => matches!(result, Err(FetchError::Corrupt)),
                    _ => matches!(result, Err(FetchError::Interrupted)),
                },
                "{class}: {result:?}"
            );
            let store = node.shared.store.as_ref().unwrap();
            assert!(!store.has(hash));
            assert_eq!(
                std::fs::read_dir(store.root().join("tmp")).unwrap().count(),
                0
            );
        }
    }

    #[tokio::test]
    #[ignore]
    async fn loopback_64_mib_release_throughput() {
        let (_a_dir, a) = device("a").await;
        let (_b_dir, b) = device("b").await;
        let mesh = a.new_mesh("m").unwrap();
        add(&a, mesh, &b).await;
        let hash = a.import_reader(data(64).as_slice()).unwrap();
        a.share(mesh, hash).unwrap();
        let start = Instant::now();
        let bytes = b
            .fetch(mesh, a.info().id, hash, None, |_, _| {})
            .await
            .unwrap();
        println!(
            "64 MiB in {:.3}s: {:.1} MiB/s",
            start.elapsed().as_secs_f64(),
            bytes as f64 / 1048576.0 / start.elapsed().as_secs_f64()
        );
    }
}
