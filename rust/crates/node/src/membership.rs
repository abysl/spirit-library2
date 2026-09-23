use crate::mesh_id::MeshId;
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{EndpointAddr, EndpointId, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_MEMBERS: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub id: EndpointId,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Admission {
    pub member: Member,
    pub issuer: EndpointId,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Mesh {
    pub id: MeshId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founder: Option<EndpointId>,
    pub admissions: Vec<Admission>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub mesh: Mesh,
    pub addresses: BTreeMap<EndpointId, EndpointAddr>,
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
        "names must contain 1–128 bytes and no control characters"
    );
    Ok(())
}

impl Admission {
    fn payload(&self, mesh: &Mesh) -> Result<Vec<u8>> {
        match mesh.id.random_bytes() {
            Some(id) => Ok(postcard::to_stdvec(&(
                "spirit/mesh/admission/2",
                id,
                mesh.founder.context("missing mesh founder")?,
                &mesh.name,
                &self.member,
                self.issuer,
            ))?),
            None => Ok(postcard::to_stdvec(&(
                "spirit/mesh/admission/1",
                mesh.id.legacy_node().context("missing legacy mesh ID")?,
                &mesh.name,
                &self.member,
                self.issuer,
            ))?),
        }
    }

    fn signed(mesh: &Mesh, member: Member, key: &SecretKey) -> Result<Self> {
        let mut admission = Self {
            member,
            issuer: key.public(),
            signature: String::new(),
        };
        admission.signature =
            URL_SAFE_NO_PAD.encode(key.sign(&admission.payload(mesh)?).to_bytes());
        Ok(admission)
    }

    fn verify(&self, mesh: &Mesh) -> Result<()> {
        validate_name(&self.member.name)?;
        let bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&self.signature)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid admission signature length"))?;
        self.issuer
            .verify(&self.payload(mesh)?, &Signature::from_bytes(&bytes))
            .context("invalid admission signature")?;
        Ok(())
    }
}

impl Mesh {
    pub fn create(name: &str, member: Member, key: &SecretKey) -> Result<Self> {
        validate_name(name)?;
        ensure!(
            member.id == key.public(),
            "founder identity does not match key"
        );
        let mut mesh = Self {
            id: MeshId::generate()?,
            name: name.to_owned(),
            founder: Some(member.id),
            admissions: Vec::new(),
        };
        mesh.admissions.push(Admission::signed(&mesh, member, key)?);
        mesh.verify()?;
        Ok(mesh)
    }

    pub fn verify(&self) -> Result<()> {
        validate_name(&self.name)?;
        ensure!(
            !self.admissions.is_empty() && self.admissions.len() <= MAX_MEMBERS,
            "invalid mesh size"
        );
        let mut ids = BTreeSet::new();
        for admission in &self.admissions {
            admission.verify(self)?;
            ensure!(ids.insert(admission.member.id), "duplicate admission");
        }
        let founder = match (self.id.legacy_node(), self.founder) {
            (Some(id), None) => id,
            (None, Some(id)) => id,
            _ => bail!("mesh identity and founder format do not match"),
        };
        let root = self
            .admissions
            .iter()
            .find(|a| a.member.id == founder)
            .context("missing mesh founder")?;
        ensure!(root.issuer == founder, "invalid founding admission");
        let mut trusted = BTreeSet::from([founder]);
        loop {
            let before = trusted.len();
            for admission in &self.admissions {
                if trusted.contains(&admission.issuer) {
                    trusted.insert(admission.member.id);
                }
            }
            if trusted.len() == self.admissions.len() {
                return Ok(());
            }
            if trusted.len() == before {
                bail!("admission issuer is not a mesh member");
            }
        }
    }

    pub fn member(&self, id: EndpointId) -> Option<&Member> {
        self.admissions
            .iter()
            .map(|a| &a.member)
            .find(|member| member.id == id)
    }

    pub fn admit(&mut self, member: Member, key: &SecretKey) -> Result<()> {
        ensure!(
            self.member(key.public()).is_some(),
            "only mesh members can admit devices"
        );
        if let Some(existing) = self.member(member.id) {
            ensure!(
                existing.name == member.name,
                "device nickname does not match its membership"
            );
            return Ok(());
        }
        validate_name(&member.name)?;
        ensure!(
            self.admissions.len() < MAX_MEMBERS,
            "mesh member limit reached"
        );
        self.admissions.push(Admission::signed(self, member, key)?);
        Ok(())
    }

    pub fn merge(&mut self, other: &Self) -> Result<()> {
        other.verify()?;
        ensure!(
            self.id == other.id && self.name == other.name && self.founder == other.founder,
            "device belongs to a different mesh"
        );
        let mut merged = self.clone();
        for admission in &other.admissions {
            if let Some(existing) = merged.member(admission.member.id) {
                ensure!(
                    existing.name == admission.member.name,
                    "conflicting device nickname"
                );
            } else {
                merged.admissions.push(admission.clone());
            }
        }
        merged.verify()?;
        *self = merged;
        Ok(())
    }
}

impl Snapshot {
    pub fn verify(&self) -> Result<()> {
        self.mesh.verify()?;
        ensure!(
            self.addresses.len() <= MAX_MEMBERS,
            "too many peer addresses"
        );
        for (id, addr) in &self.addresses {
            ensure!(
                addr.id == *id && self.mesh.member(*id).is_some(),
                "address does not belong to a member"
            );
            ensure!(addr.addrs.len() <= 16, "too many transports for device");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(key: &SecretKey, name: &str) -> Member {
        Member {
            id: key.public(),
            name: name.into(),
        }
    }

    #[test]
    fn tampering_and_untrusted_issuers_are_rejected() {
        let root = SecretKey::generate();
        let outsider = SecretKey::generate();
        let child = SecretKey::generate();
        let mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        let mut forged = mesh.clone();
        forged
            .admissions
            .push(Admission::signed(&mesh, member(&child, "child"), &outsider).unwrap());
        assert!(forged.verify().is_err());
        let mut forged = mesh.clone();
        forged.name = "different".into();
        assert!(forged.verify().is_err());
        let mut forged = mesh.clone();
        forged.id = MeshId::generate().unwrap();
        assert!(forged.verify().is_err());
        let mut forged = mesh.clone();
        forged.founder = Some(child.public());
        assert!(forged.verify().is_err());
        let mut forged = mesh;
        forged.admissions[0].member.name = "impostor".into();
        assert!(forged.verify().is_err());
    }

    #[test]
    fn mesh_ids_are_independent_of_the_founder_and_are_bound_to_admissions() {
        let root = SecretKey::generate();
        let mut first = Mesh::create("private", member(&root, "root"), &root).unwrap();
        let second = Mesh::create("private", member(&root, "root"), &root).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.id.to_string(), root.public().to_string());
        assert!(first.merge(&second).is_err());
        let stored = serde_json::to_vec(&first).unwrap();
        let reopened: Mesh = serde_json::from_slice(&stored).unwrap();
        assert_eq!(reopened.id, first.id);
        reopened.verify().unwrap();
    }

    #[test]
    fn legacy_meshes_retain_their_signed_identity() {
        let directory = tempfile::tempdir().unwrap();
        crate::Node::init(directory.path(), "root").unwrap();
        let (storage, mut state) = crate::storage::Storage::open(directory.path()).unwrap();
        let root = storage.key.clone();
        let child = SecretKey::generate();
        let mut old = Mesh {
            id: MeshId::legacy(root.public()),
            name: "private".into(),
            founder: None,
            admissions: Vec::new(),
        };
        old.admissions
            .push(Admission::signed(&old, member(&root, "root"), &root).unwrap());
        let stored = serde_json::to_value(&old).unwrap();
        assert!(stored.get("founder").is_none());
        assert_eq!(stored["id"], root.public().to_string());
        let mut reopened: Mesh = serde_json::from_value(stored).unwrap();
        reopened.verify().unwrap();
        reopened.admit(member(&child, "child"), &root).unwrap();
        reopened.verify().unwrap();
        assert_eq!(reopened.id.to_string(), root.public().to_string());
        let mut invalid = reopened.clone();
        invalid.founder = Some(root.public());
        assert!(invalid.verify().is_err());
        state.mesh = Some(reopened);
        storage.save(&state).unwrap();
        drop(storage);
        let info = crate::Node::read_info(directory.path()).unwrap();
        assert_eq!(info.mesh_id, Some(MeshId::legacy(root.public())));
        assert_eq!(info.members.len(), 2);
    }

    #[test]
    fn transitive_admission_is_valid_even_when_records_are_reordered() {
        let root = SecretKey::generate();
        let middle = SecretKey::generate();
        let child = SecretKey::generate();
        let mut mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        mesh.admit(member(&middle, "middle"), &root).unwrap();
        mesh.admit(member(&child, "child"), &middle).unwrap();
        mesh.admissions.reverse();
        mesh.verify().unwrap();
    }
}
