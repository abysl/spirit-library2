use crate::mesh_id::MeshId;
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{EndpointAddr, EndpointId, SecretKey, Signature, TransportAddr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_MEMBERS: usize = 256;
pub(crate) const MAX_ADMISSIONS: usize = 256;
pub(crate) const MAX_ADDRESSES: usize = 16;
pub(crate) const MAX_TRANSPORT_ADDRESS_BYTES: usize = 160;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub id: EndpointId,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Admission {
    pub member: Member,
    pub issuer: EndpointId,
    #[serde(default, skip_serializing_if = "is_first_generation")]
    pub generation: u32,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Departure {
    pub member: EndpointId,
    pub generation: u32,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Mesh {
    pub id: MeshId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founder: Option<EndpointId>,
    pub admissions: Vec<Admission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub departures: Vec<Departure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub mesh: Mesh,
    pub addresses: BTreeMap<EndpointId, EndpointAddr>,
}

pub(crate) struct VerifiedSnapshot<'a>(&'a Snapshot);

impl<'a> VerifiedSnapshot<'a> {
    pub fn new(snapshot: &'a Snapshot) -> Result<Self> {
        snapshot.verify()?;
        Ok(Self(snapshot))
    }

    pub fn caller_vouches_for_local_snapshot(snapshot: &'a Snapshot) -> Self {
        Self(snapshot)
    }

    pub fn snapshot(&self) -> &'a Snapshot {
        self.0
    }

    pub fn merge_into(&self, mesh: &mut Mesh) -> Result<()> {
        mesh.merge_trusted(&self.0.mesh)
    }
}

pub(crate) fn bounded_address(mut addr: EndpointAddr) -> EndpointAddr {
    let mut addrs: Vec<_> = std::mem::take(&mut addr.addrs).into_iter().collect();
    addrs.sort_by_key(|transport| match transport {
        TransportAddr::Relay(_) => 0,
        TransportAddr::Ip(ip) if ip.is_ipv4() => 1,
        TransportAddr::Ip(_) => 2,
        _ => 3,
    });
    addr.addrs = addrs
        .into_iter()
        .filter(|transport| {
            serde_json::to_vec(transport)
                .is_ok_and(|bytes| bytes.len() <= MAX_TRANSPORT_ADDRESS_BYTES)
        })
        .take(MAX_ADDRESSES)
        .collect();
    addr
}

fn is_first_generation(generation: &u32) -> bool {
    *generation == 0
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control),
        "names must contain 1–128 bytes and no control characters"
    );
    Ok(())
}

fn decode_signature(signature: &str) -> Result<Signature> {
    let bytes: [u8; 64] = URL_SAFE_NO_PAD
        .decode(signature)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid signature length"))?;
    Ok(Signature::from_bytes(&bytes))
}

impl Admission {
    fn payload(&self, mesh: &Mesh) -> Result<Vec<u8>> {
        if self.generation > 0 {
            return Ok(postcard::to_stdvec(&(
                "spirit/mesh/readmission/1",
                mesh.id,
                mesh.founder,
                &mesh.name,
                &self.member,
                self.issuer,
                self.generation,
            ))?);
        }
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

    fn signed(mesh: &Mesh, member: Member, generation: u32, key: &SecretKey) -> Result<Self> {
        let mut admission = Self {
            member,
            issuer: key.public(),
            generation,
            signature: String::new(),
        };
        admission.signature =
            URL_SAFE_NO_PAD.encode(key.sign(&admission.payload(mesh)?).to_bytes());
        Ok(admission)
    }

    fn verify(&self, mesh: &Mesh) -> Result<()> {
        validate_name(&self.member.name)?;
        self.issuer
            .verify(&self.payload(mesh)?, &decode_signature(&self.signature)?)
            .context("invalid admission signature")?;
        Ok(())
    }

    fn key(&self) -> (EndpointId, u32) {
        (self.member.id, self.generation)
    }
}

impl Departure {
    fn payload(&self, mesh: &Mesh) -> Result<Vec<u8>> {
        Ok(postcard::to_stdvec(&(
            "spirit/mesh/departure/1",
            mesh.id,
            mesh.founder,
            &mesh.name,
            self.member,
            self.generation,
        ))?)
    }

    fn signed(mesh: &Mesh, generation: u32, key: &SecretKey) -> Result<Self> {
        let mut departure = Self {
            member: key.public(),
            generation,
            signature: String::new(),
        };
        departure.signature =
            URL_SAFE_NO_PAD.encode(key.sign(&departure.payload(mesh)?).to_bytes());
        Ok(departure)
    }

    fn verify(&self, mesh: &Mesh) -> Result<()> {
        self.member
            .verify(&self.payload(mesh)?, &decode_signature(&self.signature)?)
            .context("invalid departure signature")?;
        Ok(())
    }

    fn key(&self) -> (EndpointId, u32) {
        (self.member, self.generation)
    }
}

impl Mesh {
    pub fn create(name: &str, member: Member, key: &SecretKey) -> Result<Self> {
        Self::with_id(MeshId::generate()?, name, member, key)
    }

    #[cfg(test)]
    pub(crate) fn create_with_id(
        id: MeshId,
        name: &str,
        member: Member,
        key: &SecretKey,
    ) -> Result<Self> {
        Self::with_id(id, name, member, key)
    }

    fn with_id(id: MeshId, name: &str, member: Member, key: &SecretKey) -> Result<Self> {
        validate_name(name)?;
        ensure!(
            member.id == key.public(),
            "founder identity does not match key"
        );
        let mut mesh = Self {
            id,
            name: name.to_owned(),
            founder: Some(member.id),
            admissions: Vec::new(),
            departures: Vec::new(),
        };
        mesh.admissions
            .push(Admission::signed(&mesh, member, 0, key)?);
        mesh.verify()?;
        Ok(mesh)
    }

    #[cfg(test)]
    pub(crate) fn create_legacy(name: &str, member: Member, key: &SecretKey) -> Result<Self> {
        let mut mesh = Self {
            id: MeshId::legacy(key.public()),
            name: name.into(),
            founder: None,
            admissions: Vec::new(),
            departures: Vec::new(),
        };
        mesh.admissions
            .push(Admission::signed(&mesh, member, 0, key)?);
        mesh.verify()?;
        Ok(mesh)
    }

    pub fn verify(&self) -> Result<()> {
        self.verify_structure()?;
        for admission in &self.admissions {
            admission.verify(self)?;
        }
        for departure in &self.departures {
            departure.verify(self)?;
        }
        Ok(())
    }

    fn verify_structure(&self) -> Result<()> {
        validate_name(&self.name)?;
        ensure!(
            !self.admissions.is_empty() && self.admissions.len() <= MAX_ADMISSIONS,
            "invalid mesh size"
        );
        let mut admissions = BTreeSet::new();
        for admission in &self.admissions {
            ensure!(admissions.insert(admission.key()), "duplicate admission");
        }
        let mut departures = BTreeSet::new();
        for departure in &self.departures {
            ensure!(
                admissions.contains(&departure.key()),
                "departure has no matching admission"
            );
            ensure!(departures.insert(departure.key()), "duplicate departure");
        }
        for admission in &self.admissions {
            ensure!(
                admission.generation == 0
                    || departures.contains(&(admission.member.id, admission.generation - 1)),
                "readmission does not follow a departure"
            );
        }
        for admission in &self.admissions {
            validate_name(&admission.member.name)?;
        }
        let founder = match (self.id.legacy_node(), self.founder) {
            (Some(id), None) => id,
            (None, Some(id)) => id,
            _ => bail!("mesh identity and founder format do not match"),
        };
        let root = self
            .admissions
            .iter()
            .find(|a| a.member.id == founder && a.generation == 0)
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
            if self
                .admissions
                .iter()
                .all(|admission| trusted.contains(&admission.issuer))
            {
                return Ok(());
            }
            if trusted.len() == before {
                bail!("admission issuer is not a mesh member");
            }
        }
    }

    fn latest_admission(&self, id: EndpointId) -> Option<&Admission> {
        self.admissions
            .iter()
            .filter(|a| a.member.id == id)
            .max_by_key(|a| a.generation)
    }

    fn departed_generation(&self, id: EndpointId, generation: u32) -> bool {
        self.departures
            .iter()
            .any(|d| d.member == id && d.generation == generation)
    }

    pub fn member(&self, id: EndpointId) -> Option<&Member> {
        self.latest_admission(id)
            .filter(|admission| !self.departed_generation(id, admission.generation))
            .map(|admission| &admission.member)
    }

    pub fn current_admissions(&self) -> impl Iterator<Item = (&Member, u32)> {
        let mut latest = BTreeMap::new();
        for admission in &self.admissions {
            latest
                .entry(admission.member.id)
                .and_modify(|generation: &mut u32| {
                    *generation = (*generation).max(admission.generation);
                })
                .or_insert(admission.generation);
        }
        let departures: BTreeSet<_> = self.departures.iter().map(Departure::key).collect();
        self.admissions
            .iter()
            .filter(move |admission| {
                latest.get(&admission.member.id) == Some(&admission.generation)
                    && !departures.contains(&admission.key())
            })
            .map(|admission| (&admission.member, admission.generation))
    }

    pub fn members(&self) -> impl Iterator<Item = &Member> {
        self.current_admissions().map(|(member, _)| member)
    }

    pub fn departed(&self, id: EndpointId) -> bool {
        self.latest_admission(id)
            .is_some_and(|admission| self.departed_generation(id, admission.generation))
    }

    pub fn records_departure_at(&self, other: &Self, id: EndpointId) -> bool {
        other
            .latest_admission(id)
            .is_some_and(|admission| self.departed_generation(id, admission.generation))
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
            self.admissions.len() < MAX_ADMISSIONS,
            "group membership history is full"
        );
        let generation = match self.latest_admission(member.id) {
            Some(departed) => departed
                .generation
                .checked_add(1)
                .context("device readmission limit reached")?,
            None => 0,
        };
        self.admissions
            .push(Admission::signed(self, member, generation, key)?);
        Ok(())
    }

    pub fn depart(&mut self, key: &SecretKey) -> Result<()> {
        let admission = self
            .latest_admission(key.public())
            .filter(|admission| !self.departed_generation(key.public(), admission.generation))
            .context("device is not a mesh member")?;
        let departure = Departure::signed(self, admission.generation, key)?;
        self.departures.push(departure);
        Ok(())
    }

    #[cfg(test)]
    pub fn merge(&mut self, other: &Self) -> Result<()> {
        other.verify()?;
        self.merge_trusted(other)
    }

    pub fn same_identity(&self, other: &Self) -> bool {
        self.id == other.id && self.name == other.name && self.founder == other.founder
    }

    fn merge_trusted(&mut self, other: &Self) -> Result<()> {
        ensure!(
            self.same_identity(other),
            "device belongs to a different mesh"
        );
        let mut merged = self.clone();
        for admission in &other.admissions {
            match merged
                .admissions
                .iter()
                .find(|a| a.key() == admission.key())
            {
                Some(existing) => ensure!(
                    existing.member.name == admission.member.name,
                    "conflicting device nickname"
                ),
                None => merged.admissions.push(admission.clone()),
            }
        }
        for departure in &other.departures {
            if !merged.departures.iter().any(|d| d.key() == departure.key()) {
                merged.departures.push(departure.clone());
            }
        }
        merged.verify_structure()?;
        *self = merged;
        Ok(())
    }
}

impl Snapshot {
    pub fn without_addresses(mesh: Mesh) -> Self {
        Self {
            mesh,
            addresses: BTreeMap::new(),
        }
    }

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
            ensure!(
                addr.addrs.len() <= MAX_ADDRESSES,
                "too many transports for device"
            );
            for transport in &addr.addrs {
                ensure!(
                    serde_json::to_vec(transport)?.len() <= MAX_TRANSPORT_ADDRESS_BYTES,
                    "transport address is too long"
                );
            }
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
            .push(Admission::signed(&mesh, member(&child, "child"), 0, &outsider).unwrap());
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
            departures: Vec::new(),
        };
        let root_member = member(&root, "root");
        let legacy_payload = postcard::to_stdvec(&(
            "spirit/mesh/admission/1",
            root.public(),
            "private",
            &root_member,
            root.public(),
        ))
        .unwrap();
        old.admissions.push(Admission {
            member: root_member,
            issuer: root.public(),
            generation: 0,
            signature: URL_SAFE_NO_PAD.encode(root.sign(&legacy_payload).to_bytes()),
        });
        let stored = serde_json::to_value(&old).unwrap();
        assert!(stored.get("founder").is_none());
        assert_eq!(stored["id"], root.public().to_string());
        let mut reopened: Mesh = serde_json::from_value(stored).unwrap();
        reopened.verify().unwrap();
        reopened.admit(member(&child, "child"), &root).unwrap();
        reopened.verify().unwrap();
        let child_member = member(&child, "child");
        let legacy_child_payload = postcard::to_stdvec(&(
            "spirit/mesh/admission/1",
            root.public(),
            "private",
            &child_member,
            root.public(),
        ))
        .unwrap();
        assert_eq!(
            reopened.admissions[1].signature,
            URL_SAFE_NO_PAD.encode(root.sign(&legacy_child_payload).to_bytes()),
        );
        assert_eq!(reopened.id.to_string(), root.public().to_string());
        let mut invalid = reopened.clone();
        invalid.founder = Some(root.public());
        assert!(invalid.verify().is_err());
        state.meshes.insert(reopened.id, reopened);
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

    #[test]
    fn departures_hide_members_converge_and_permit_readmission() {
        let root = SecretKey::generate();
        let child = SecretKey::generate();
        let mut before = Mesh::create("private", member(&root, "root"), &root).unwrap();
        before.admit(member(&child, "child"), &root).unwrap();
        let mut left = before.clone();
        left.depart(&child).unwrap();
        left.verify().unwrap();
        assert!(left.member(child.public()).is_none());
        assert!(left.departed(child.public()));
        assert_eq!(left.members().count(), 1);
        assert!(left.depart(&child).is_err());
        assert!(left.admit(member(&root, "root"), &child).is_err());

        let mut stale = before.clone();
        stale.merge(&left).unwrap();
        assert!(stale.departed(child.public()));
        left.merge(&before).unwrap();
        assert!(left.departed(child.public()));

        left.admit(member(&child, "child"), &root).unwrap();
        left.verify().unwrap();
        assert_eq!(left.member(child.public()), Some(&member(&child, "child")));
        assert_eq!(left.members().count(), 2);
        assert_eq!(left.admissions.last().unwrap().generation, 1);
        let reopened: Mesh = serde_json::from_slice(&serde_json::to_vec(&left).unwrap()).unwrap();
        reopened.verify().unwrap();
        assert!(reopened.member(child.public()).is_some());
        stale.merge(&reopened).unwrap();
        assert!(stale.member(child.public()).is_some());
        let first = serde_json::to_value(&before).unwrap();
        assert!(first.get("departures").is_none());
        assert!(first["admissions"][1].get("generation").is_none());
    }

    #[test]
    fn departures_and_readmissions_must_be_signed_and_ordered() {
        let root = SecretKey::generate();
        let child = SecretKey::generate();
        let mut mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        mesh.admit(member(&child, "child"), &root).unwrap();

        let mut forged = mesh.clone();
        forged
            .departures
            .push(Departure::signed(&forged, 0, &root).unwrap());
        forged.departures[0].member = child.public();
        assert!(forged.verify().is_err());

        let mut unmatched = mesh.clone();
        unmatched
            .departures
            .push(Departure::signed(&unmatched, 1, &child).unwrap());
        assert!(unmatched.verify().is_err());

        let mut early = mesh.clone();
        early
            .admissions
            .push(Admission::signed(&early, member(&child, "child"), 1, &root).unwrap());
        assert!(early.verify().is_err());

        let mut other = Mesh::create("private", member(&root, "root"), &root).unwrap();
        other.admit(member(&child, "child"), &root).unwrap();
        let mut replayed = mesh.clone();
        replayed
            .departures
            .push(Departure::signed(&other, 0, &child).unwrap());
        assert!(replayed.verify().is_err());
    }

    #[test]
    fn shuffled_departures_and_readmissions_verify() {
        let root = SecretKey::generate();
        let child = SecretKey::generate();
        let mut mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        mesh.admit(member(&child, "child"), &root).unwrap();
        for _ in 0..3 {
            mesh.depart(&child).unwrap();
            mesh.admit(member(&child, "child"), &root).unwrap();
        }
        mesh.admissions.reverse();
        mesh.departures.reverse();
        mesh.verify().unwrap();
        assert_eq!(mesh.members().count(), 2);
    }

    #[test]
    fn concurrent_readmissions_converge_and_repeat_merges_are_noops() {
        let root = SecretKey::generate();
        let introducer = SecretKey::generate();
        let child = SecretKey::generate();
        let mut mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        mesh.admit(member(&introducer, "introducer"), &root)
            .unwrap();
        mesh.admit(member(&child, "child"), &root).unwrap();
        mesh.depart(&child).unwrap();
        let mut left = mesh.clone();
        let mut right = mesh;
        left.admit(member(&child, "child"), &root).unwrap();
        right.admit(member(&child, "child"), &introducer).unwrap();
        left.merge(&right).unwrap();
        right.merge(&left).unwrap();
        assert_eq!(left.admissions.len(), right.admissions.len());
        assert_eq!(left.members().count(), 3);
        let before = serde_json::to_vec(&left).unwrap();
        left.merge(&right).unwrap();
        assert_eq!(before, serde_json::to_vec(&left).unwrap());
        let before = serde_json::to_vec(&right).unwrap();
        right.merge(&left).unwrap();
        assert_eq!(before, serde_json::to_vec(&right).unwrap());
    }

    #[test]
    fn independently_valid_admission_caps_cannot_merge() {
        let root = SecretKey::generate();
        let mut base = Mesh::create("private", member(&root, "root"), &root).unwrap();
        for index in 0..MAX_ADMISSIONS - 2 {
            let key = SecretKey::generate();
            base.admit(member(&key, &index.to_string()), &root).unwrap();
        }
        let mut left = base.clone();
        let mut right = base;
        left.admit(member(&SecretKey::generate(), "left"), &root)
            .unwrap();
        right
            .admit(member(&SecretKey::generate(), "right"), &root)
            .unwrap();
        left.verify().unwrap();
        right.verify().unwrap();
        assert!(left
            .merge(&right)
            .unwrap_err()
            .to_string()
            .contains("invalid mesh size"));
        assert!(right
            .merge(&left)
            .unwrap_err()
            .to_string()
            .contains("invalid mesh size"));
    }

    #[test]
    fn membership_bounds_and_maximum_snapshot_fit_wire_limit() {
        use iroh::{RelayUrl, TransportAddr};
        use std::time::Instant;

        let root = SecretKey::generate();
        let mut mesh =
            Mesh::create(&"n".repeat(128), member(&root, &"r".repeat(128)), &root).unwrap();
        let mut keys = vec![root.clone()];
        for _ in 1..MAX_MEMBERS {
            let key = SecretKey::generate();
            mesh.admit(member(&key, &"\"".repeat(128)), &root).unwrap();
            keys.push(key);
        }
        assert!(mesh
            .admit(member(&SecretKey::generate(), "extra"), &root)
            .unwrap_err()
            .to_string()
            .contains("group membership history is full"));
        let readmission = SecretKey::generate();
        let mut past = Mesh::create("past", member(&root, "root"), &root).unwrap();
        past.admit(member(&readmission, "returning"), &root)
            .unwrap();
        past.depart(&readmission).unwrap();
        for _ in 0..MAX_ADMISSIONS - 2 {
            past.admit(member(&SecretKey::generate(), "other"), &root)
                .unwrap();
        }
        assert!(past
            .admit(member(&readmission, "returning"), &root)
            .unwrap_err()
            .to_string()
            .contains("group membership history is full"));
        let mesh_current = mesh.clone();
        for key in &keys {
            mesh.depart(key).unwrap();
            assert!(mesh.admissions.len() + mesh.departures.len() <= 2 * MAX_ADMISSIONS);
        }
        assert_eq!(
            mesh.admissions.len() + mesh.departures.len(),
            2 * MAX_ADMISSIONS
        );
        mesh.verify().unwrap();
        let mut addresses = BTreeMap::new();
        for key in &keys {
            let mut addr: EndpointAddr = key.public().into();
            for index in 0..MAX_ADDRESSES {
                let prefix = format!("https://relay.example/{index:02}/");
                let overhead =
                    serde_json::to_vec(&TransportAddr::Relay(prefix.parse::<RelayUrl>().unwrap()))
                        .unwrap()
                        .len();
                let url = format!(
                    "{prefix}{}",
                    "x".repeat(MAX_TRANSPORT_ADDRESS_BYTES - overhead)
                );
                let transport = TransportAddr::Relay(url.parse::<RelayUrl>().unwrap());
                assert_eq!(
                    serde_json::to_vec(&transport).unwrap().len(),
                    MAX_TRANSPORT_ADDRESS_BYTES
                );
                addr.addrs.insert(transport);
            }
            addresses.insert(key.public(), addr);
        }
        let snapshot = Snapshot {
            mesh: mesh_current.clone(),
            addresses,
        };
        let departed_snapshot = Snapshot::without_addresses(mesh);
        departed_snapshot.verify().unwrap();
        let departed_length = serde_json::to_vec(&departed_snapshot).unwrap().len();
        snapshot.verify().unwrap();
        let length = serde_json::to_vec(&snapshot).unwrap().len();
        assert!(
            length.max(departed_length) < 1024 * 1024,
            "maximum snapshot is {} bytes",
            length.max(departed_length)
        );
        let start = Instant::now();
        departed_snapshot.mesh.verify().unwrap();
        let verify = start.elapsed();
        let mut merging = departed_snapshot.mesh.clone();
        let start = Instant::now();
        merging.merge_trusted(&departed_snapshot.mesh).unwrap();
        let merge = start.elapsed();
        println!("256 current members: snapshot={length} bytes; 512 records, no current members: snapshot={departed_length} bytes verify={verify:?} trusted_merge={merge:?}");
        let mut too_long = snapshot;
        let first = too_long.addresses.values_mut().next().unwrap();
        first.addrs.clear();
        first.addrs.insert(TransportAddr::Relay(
            format!("https://relay.example/{}", "z".repeat(160))
                .parse()
                .unwrap(),
        ));
        assert!(too_long
            .verify()
            .unwrap_err()
            .to_string()
            .contains("transport address is too long"));
    }

    #[test]
    fn addresses_are_trimmed_before_sending() {
        use iroh::RelayUrl;
        let id = SecretKey::generate().public();
        let mut address: EndpointAddr = id.into();
        let relay = TransportAddr::Relay("https://relay.example/".parse::<RelayUrl>().unwrap());
        address.addrs.insert(relay.clone());
        address.addrs.insert(TransportAddr::Relay(
            format!("https://relay.example/{}", "x".repeat(200))
                .parse()
                .unwrap(),
        ));
        for index in 0..20 {
            address.addrs.insert(TransportAddr::Ip(
                format!("127.0.0.1:{}", 1000 + index).parse().unwrap(),
            ));
        }
        address
            .addrs
            .insert(TransportAddr::Ip("[::1]:1111".parse().unwrap()));
        let bounded = bounded_address(address);
        assert_eq!(bounded.addrs.len(), MAX_ADDRESSES);
        assert!(bounded.addrs.contains(&relay));
        assert_eq!(
            bounded
                .addrs
                .iter()
                .filter(|addr| matches!(addr, TransportAddr::Ip(ip) if ip.is_ipv4()))
                .count(),
            MAX_ADDRESSES - 1
        );
        assert!(bounded
            .addrs
            .iter()
            .all(|addr| serde_json::to_vec(addr).unwrap().len() <= MAX_TRANSPORT_ADDRESS_BYTES));
    }

    #[test]
    fn signed_conflicting_readmission_names_fail_permanently() {
        let root = SecretKey::generate();
        let child = SecretKey::generate();
        let mut base = Mesh::create("private", member(&root, "root"), &root).unwrap();
        base.admit(member(&child, "child"), &root).unwrap();
        base.depart(&child).unwrap();
        let mut left = base.clone();
        let mut right = base;
        left.admit(member(&child, "child"), &root).unwrap();
        right.admit(member(&child, "dishonest"), &root).unwrap();
        left.verify().unwrap();
        right.verify().unwrap();
        for _ in 0..2 {
            assert!(left
                .merge(&right)
                .unwrap_err()
                .to_string()
                .contains("conflicting device nickname"));
            assert!(right
                .merge(&left)
                .unwrap_err()
                .to_string()
                .contains("conflicting device nickname"));
        }
    }

    #[test]
    fn remaining_members_keep_admitting_after_the_founder_leaves() {
        let root = SecretKey::generate();
        let middle = SecretKey::generate();
        let child = SecretKey::generate();
        let mut mesh = Mesh::create("private", member(&root, "root"), &root).unwrap();
        mesh.admit(member(&middle, "middle"), &root).unwrap();
        mesh.depart(&root).unwrap();
        mesh.admit(member(&child, "child"), &middle).unwrap();
        mesh.verify().unwrap();
        assert_eq!(
            mesh.members().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["middle", "child"]
        );
    }
}
