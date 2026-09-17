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
    pub id: EndpointId,
    pub name: String,
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
    fn payload(&self, mesh_id: EndpointId, mesh_name: &str) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(&(
            "spirit/mesh/admission/1",
            mesh_id,
            mesh_name,
            &self.member,
            self.issuer,
        ))?)
    }

    fn signed(mesh: &Mesh, member: Member, key: &SecretKey) -> Result<Self> {
        let mut admission = Self {
            member,
            issuer: key.public(),
            signature: String::new(),
        };
        admission.signature = URL_SAFE_NO_PAD.encode(
            key.sign(&admission.payload(mesh.id, &mesh.name)?)
                .to_bytes(),
        );
        Ok(admission)
    }

    fn verify(&self, mesh: &Mesh) -> Result<()> {
        validate_name(&self.member.name)?;
        let bytes: [u8; 64] = URL_SAFE_NO_PAD
            .decode(&self.signature)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid admission signature length"))?;
        self.issuer
            .verify(
                &self.payload(mesh.id, &mesh.name)?,
                &Signature::from_bytes(&bytes),
            )
            .context("invalid admission signature")?;
        Ok(())
    }
}

impl Mesh {
    pub fn create(name: &str, member: Member, key: &SecretKey) -> Result<Self> {
        validate_name(name)?;
        let mut mesh = Self {
            id: key.public(),
            name: name.to_owned(),
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
        let root = self
            .admissions
            .iter()
            .find(|a| a.member.id == self.id)
            .context("missing mesh founder")?;
        ensure!(root.issuer == self.id, "invalid founding admission");
        let mut trusted = BTreeSet::from([self.id]);
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
            self.id == other.id && self.name == other.name,
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
        let mut forged = mesh;
        forged.admissions[0].member.name = "impostor".into();
        assert!(forged.verify().is_err());
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
