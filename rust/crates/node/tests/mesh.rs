use spirit_node::{Node, NodeConfig, NodeId};
use std::time::Duration;
use tempfile::TempDir;

fn initialized(name: &str) -> (TempDir, NodeId) {
    let dir = tempfile::tempdir().unwrap();
    let info = Node::init(dir.path(), name).unwrap();
    (dir, info.id)
}

async fn local(dir: &TempDir) -> Node {
    Node::bind(dir.path(), NodeConfig::local()).await.unwrap()
}

#[test]
fn identity_is_persistent_and_distinct() {
    let (dir, id) = initialized("desktop");
    assert_eq!(Node::init(dir.path(), "desktop").unwrap().id, id);
    assert_eq!(Node::read_info(dir.path()).unwrap().id, id);
    let (_other, other_id) = initialized("laptop");
    assert_ne!(id, other_id);
}

#[test]
fn corrupted_identity_is_not_replaced() {
    let (dir, _) = initialized("desktop");
    std::fs::write(dir.path().join("secret.key"), b"broken").unwrap();
    assert!(Node::init(dir.path(), "desktop").is_err());
    assert_eq!(
        std::fs::read(dir.path().join("secret.key")).unwrap(),
        b"broken"
    );
}

#[cfg(unix)]
#[test]
fn identity_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, _) = initialized("desktop");
    for file in ["secret.key", "state.json"] {
        let mode = std::fs::metadata(dir.path().join(file))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
}

#[tokio::test]
async fn members_communicate_after_introducer_shuts_down_and_after_restart() {
    let (phone_dir, phone_id) = initialized("phone");
    let (desktop_dir, desktop_id) = initialized("desktop");
    let (laptop_dir, laptop_id) = initialized("laptop");
    Node::create_mesh(phone_dir.path(), "personal").unwrap();
    let phone = local(&phone_dir).await;
    let desktop = local(&desktop_dir).await;
    let laptop = local(&laptop_dir).await;
    phone
        .add(&desktop.pair(Duration::from_secs(60)).await.unwrap())
        .await
        .unwrap();
    phone
        .add(&laptop.pair(Duration::from_secs(60)).await.unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        while desktop.info().members.len() != 3 || laptop.info().members.len() != 3 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(desktop.info().mesh_id, Some(phone_id));
    phone.shutdown().await.unwrap();
    drop(phone);
    assert_eq!(desktop.ping(laptop_id).await.unwrap().id, laptop_id);
    assert_eq!(laptop.ping(desktop_id).await.unwrap().id, desktop_id);
    desktop.shutdown().await.unwrap();
    drop(desktop);
    let desktop = local(&desktop_dir).await;
    assert_eq!(desktop.info().id, desktop_id);
    assert_eq!(desktop.ping(laptop_id).await.unwrap().id, laptop_id);
    let (fourth_dir, fourth_id) = initialized("fourth");
    let fourth = local(&fourth_dir).await;
    desktop
        .add(&fourth.pair(Duration::from_secs(60)).await.unwrap())
        .await
        .unwrap();
    assert_eq!(fourth.info().mesh_id, Some(phone_id));
    assert_eq!(fourth.ping(laptop_id).await.unwrap().id, laptop_id);
    assert_eq!(desktop.ping(fourth_id).await.unwrap().id, fourth_id);
    fourth.shutdown().await.unwrap();
    desktop.shutdown().await.unwrap();
    laptop.shutdown().await.unwrap();
}

#[tokio::test]
async fn enrollment_rejects_wrong_mesh_replays_and_expired_tickets() {
    let (a_dir, _) = initialized("a");
    let (b_dir, _) = initialized("b");
    let (c_dir, _) = initialized("c");
    Node::create_mesh(a_dir.path(), "one").unwrap();
    Node::create_mesh(b_dir.path(), "two").unwrap();
    let a = local(&a_dir).await;
    let b = local(&b_dir).await;
    let c = local(&c_dir).await;
    let ticket = c.pair(Duration::from_secs(60)).await.unwrap();
    a.add(&ticket).await.unwrap();
    assert!(a.add(&ticket).await.is_err());
    let ticket = c.pair(Duration::from_secs(60)).await.unwrap();
    assert!(b.add(&ticket).await.is_err());
    a.add(&ticket).await.unwrap();
    assert_eq!(a.info().members.len(), 2);
    let ticket = c.pair(Duration::from_secs(1)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(a.add(&ticket).await.is_err());
    assert!(c.pair(Duration::ZERO).await.is_err());
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn outsiders_cannot_ping_and_one_process_owns_a_node() {
    let (a_dir, a_id) = initialized("a");
    let (b_dir, b_id) = initialized("b");
    Node::create_mesh(a_dir.path(), "personal").unwrap();
    let a = local(&a_dir).await;
    let b = local(&b_dir).await;
    assert!(Node::bind(a_dir.path(), NodeConfig::local()).await.is_err());
    assert!(Node::create_mesh(a_dir.path(), "replacement").is_err());
    assert!(a.ping(b_id).await.is_err());
    assert!(b.ping(a_id).await.is_err());
    assert!(b
        .add(&a.pair(Duration::from_secs(60)).await.unwrap())
        .await
        .is_err());
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
}
