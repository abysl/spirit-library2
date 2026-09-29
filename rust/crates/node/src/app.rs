use crate::{membership::decode_signature, NodeId};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

const APP_SIGNATURE_DOMAIN: &str = "spirit/app-signature/1";

pub(crate) fn validate_app_name(name: &str) -> Result<()> {
    ensure!(
        (1..=64).contains(&name.len())
            && name
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'/' | b'-')),
        "application names must be 1–64 ASCII bytes from [a-z0-9._/-]"
    );
    ensure!(!name.starts_with("spirit/"), "reserved application name");
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
