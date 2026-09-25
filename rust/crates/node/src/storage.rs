use crate::membership::{bounded_address, validate_name, Member, Mesh, Snapshot, VerifiedSnapshot};
use crate::mesh_id::MeshId;
use crate::NodeInfo;
use anyhow::{ensure, Context, Result};
use iroh::{EndpointAddr, EndpointId, SecretKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) const MAX_CURRENT_MESHES: usize = 64;
const MAX_DEPARTED_MESHES: usize = 64;
const MULTI_MESH_SENTINEL: &str = "spirit/state/multi-mesh";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub(crate) enum AddressEntry {
    Legacy(EndpointAddr),
    Current {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direct: Option<EndpointAddr>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        hints: BTreeMap<MeshId, EndpointAddr>,
    },
}

impl AddressEntry {
    pub(crate) fn preferred(&self, mesh_id: MeshId) -> Option<&EndpointAddr> {
        match self {
            Self::Current { direct, hints } => direct.as_ref().or_else(|| hints.get(&mesh_id)),
            Self::Legacy(_) => None,
        }
    }

    pub fn dial(&self) -> Option<&EndpointAddr> {
        match self {
            Self::Current { direct, hints } => direct.as_ref().or_else(|| hints.values().next()),
            Self::Legacy(_) => None,
        }
    }

    fn set(&mut self, mesh_id: MeshId, address: EndpointAddr, direct_source: bool) -> bool {
        let Self::Current { direct, hints } = self else {
            return false;
        };
        if direct_source {
            if direct.as_ref() == Some(&address) {
                return false;
            }
            *direct = Some(address);
        } else if hints.get(&mesh_id) != Some(&address) {
            hints.insert(mesh_id, address);
        } else {
            return false;
        }
        true
    }
}

fn write_mesh_sentinel<S: Serializer>(
    _: &Option<Mesh>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(MULTI_MESH_SENTINEL)
}

fn legacy_mesh_field<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Mesh>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum LegacyMesh {
        Mesh(Mesh),
        Sentinel(String),
    }
    match Option::<LegacyMesh>::deserialize(deserializer)? {
        Some(LegacyMesh::Mesh(mesh)) => Ok(Some(mesh)),
        Some(LegacyMesh::Sentinel(value)) if value == MULTI_MESH_SENTINEL => Ok(None),
        Some(LegacyMesh::Sentinel(_)) => {
            Err(serde::de::Error::custom("invalid state mesh sentinel"))
        }
        None => Ok(None),
    }
}

fn read_departed<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<MeshId, Mesh>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Departed {
        Map(BTreeMap<MeshId, Mesh>),
        Previous(Mesh),
    }
    Ok(match Option::<Departed>::deserialize(deserializer)? {
        Some(Departed::Map(meshes)) => meshes,
        Some(Departed::Previous(mesh)) => BTreeMap::from([(mesh.id, mesh)]),
        None => BTreeMap::new(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct State {
    pub member: Member,
    #[serde(default)]
    pub meshes: BTreeMap<MeshId, Mesh>,
    #[serde(
        default,
        rename = "mesh",
        deserialize_with = "legacy_mesh_field",
        serialize_with = "write_mesh_sentinel"
    )]
    legacy_mesh: Option<Mesh>,
    pub addresses: BTreeMap<EndpointId, AddressEntry>,
    #[serde(skip)]
    needs_migration: bool,
    #[serde(
        default,
        deserialize_with = "read_departed",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub departed: BTreeMap<MeshId, Mesh>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub departure_order: Vec<MeshId>,
}

pub(crate) struct Storage {
    pub root: PathBuf,
    pub key: SecretKey,
    _lock: File,
}

pub(crate) fn lock(root: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("node.lock"))?;
    file.try_lock()
        .context("node is already running or being modified")?;
    Ok(file)
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_key(root: &Path) -> Result<SecretKey> {
    let bytes: [u8; 32] = std::fs::read(root.join("secret.key"))?
        .try_into()
        .map_err(|_| {
            anyhow::anyhow!("invalid secret.key: expected 32 bytes; refusing to replace identity")
        })?;
    Ok(SecretKey::from_bytes(&bytes))
}

impl Storage {
    pub fn init(root: &Path, name: &str) -> Result<NodeInfo> {
        validate_name(name)?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root)?;
        let guard = lock(root)?;
        let key = if root.join("secret.key").try_exists()? {
            read_key(root)?
        } else {
            ensure!(
                !root.join("state.json").try_exists()?,
                "identity is missing; refusing to replace it"
            );
            let key = SecretKey::generate();
            write_private(&root.join("secret.key"), &key.to_bytes())?;
            key
        };
        let storage = Self {
            root: root.into(),
            key,
            _lock: guard,
        };
        if !root.join("state.json").try_exists()? {
            storage.save(&State {
                member: Member {
                    id: storage.key.public(),
                    name: name.into(),
                },
                meshes: BTreeMap::new(),
                legacy_mesh: None,
                addresses: BTreeMap::new(),
                needs_migration: false,
                departed: BTreeMap::new(),
                departure_order: Vec::new(),
            })?;
        }
        let state = Self::read(root)?;
        ensure!(
            state.member.id == storage.key.public(),
            "identity does not match state"
        );
        if state.legacy_mesh.is_some() || state.needs_migration {
            storage.save(&state)?;
        }
        Ok(state.info())
    }

    pub fn open(root: &Path) -> Result<(Self, State)> {
        ensure!(
            root.join("state.json").is_file(),
            "node is not initialized; run spirit node init"
        );
        let guard = lock(root)?;
        let key = read_key(root)?;
        let mut state = Self::read(root)?;
        ensure!(
            state.member.id == key.public(),
            "identity does not match state"
        );
        let storage = Self {
            root: root.into(),
            key,
            _lock: guard,
        };
        if state.legacy_mesh.take().is_some() || state.needs_migration {
            storage.save(&state)?;
        }
        Ok((storage, state))
    }

    pub fn read(root: &Path) -> Result<State> {
        let mut state: State = serde_json::from_slice(
            &std::fs::read(root.join("state.json"))
                .context("node is not initialized; run spirit node init")?,
        )?;
        validate_name(&state.member.name)?;
        if let Some(mesh) = &state.legacy_mesh {
            ensure!(
                state.meshes.is_empty(),
                "legacy mesh cannot coexist with meshes"
            );
            state.meshes.insert(mesh.id, mesh.clone());
        }
        ensure!(
            state.meshes.len() <= MAX_CURRENT_MESHES,
            "device has reached the 64-group limit"
        );
        let legacy_mesh_id =
            (state.meshes.len() == 1).then(|| *state.meshes.keys().next().unwrap());
        state.addresses.retain(|_, entry| {
            if let AddressEntry::Legacy(address) = entry {
                state.needs_migration = true;
                let Some(mesh_id) = legacy_mesh_id else {
                    return false;
                };
                *entry = AddressEntry::Current {
                    direct: None,
                    hints: BTreeMap::from([(mesh_id, bounded_address(address.clone()))]),
                };
            }
            if let AddressEntry::Current { direct, hints } = entry {
                if let Some(address) = direct {
                    *address = bounded_address(address.clone());
                }
                for address in hints.values_mut() {
                    *address = bounded_address(address.clone());
                }
            }
            true
        });
        for (id, mesh) in &state.meshes {
            ensure!(*id == mesh.id, "mesh ID does not match its key");
            mesh.verify()?;
            ensure!(
                mesh.member(state.member.id) == Some(&state.member),
                "device is missing from its mesh"
            );
        }
        state.retain_member_addresses();
        ensure!(
            state.departed.len() <= MAX_DEPARTED_MESHES,
            "too many departed meshes"
        );
        let mut seen = BTreeSet::new();
        for id in &state.departure_order {
            ensure!(
                state.departed.contains_key(id) && seen.insert(*id),
                "invalid departure order"
            );
        }
        for id in state.departed.keys() {
            if !seen.contains(id) {
                state.departure_order.push(*id);
            }
        }
        for (id, departed) in &state.departed {
            ensure!(
                *id == departed.id,
                "departed mesh ID does not match its key"
            );
            departed.verify()?;
            ensure!(
                departed.departed(state.member.id),
                "departed mesh does not record this device leaving"
            );
            ensure!(
                !state.meshes.contains_key(id),
                "device cannot be a member of the mesh it left"
            );
        }
        Ok(state)
    }

    pub fn save(&self, state: &State) -> Result<()> {
        write_private(
            &self.root.join("state.json"),
            &serde_json::to_vec_pretty(state)?,
        )
    }
}

impl State {
    pub fn info(&self) -> NodeInfo {
        let only = if self.meshes.len() == 1 {
            self.meshes.values().next()
        } else {
            None
        };
        let mut members = BTreeMap::new();
        let meshes = self
            .meshes
            .values()
            .map(|mesh| {
                let current = mesh
                    .current_admissions()
                    .map(|(member, generation)| {
                        members.insert(member.id, member.clone());
                        crate::MeshMember {
                            id: member.id,
                            name: member.name.clone(),
                            generation,
                        }
                    })
                    .collect();
                crate::MeshInfo {
                    id: mesh.id,
                    name: mesh.name.clone(),
                    members: current,
                }
            })
            .collect();
        NodeInfo {
            id: self.member.id,
            name: self.member.name.clone(),
            mesh_id: only.map(|mesh| mesh.id),
            mesh_name: only.map(|mesh| mesh.name.clone()),
            members: members.into_values().collect(),
            meshes,
        }
    }

    pub fn snapshot(&self, mesh_id: MeshId, addr: EndpointAddr) -> Result<Snapshot> {
        let mesh = self
            .meshes
            .get(&mesh_id)
            .context("device is not a member of this mesh")?
            .clone();
        let mut addresses: BTreeMap<_, _> = self
            .addresses
            .iter()
            .filter_map(|(id, entry)| {
                mesh.member(*id).and_then(|_| {
                    entry
                        .preferred(mesh_id)
                        .map(|addr| (*id, bounded_address(addr.clone())))
                })
            })
            .collect();
        addresses.insert(addr.id, bounded_address(addr));
        Ok(Snapshot { mesh, addresses })
    }

    pub fn join(&mut self, verified: &VerifiedSnapshot<'_>, source: EndpointId) -> Result<bool> {
        self.merge_inner(verified, source, false)
    }

    pub fn merge_current(
        &mut self,
        verified: &VerifiedSnapshot<'_>,
        source: EndpointId,
    ) -> Result<bool> {
        self.merge_inner(verified, source, true)
    }

    fn merge_inner(
        &mut self,
        verified: &VerifiedSnapshot<'_>,
        source: EndpointId,
        current_only: bool,
    ) -> Result<bool> {
        let snapshot = verified.snapshot();
        ensure!(
            snapshot.mesh.member(self.member.id) == Some(&self.member),
            "membership does not match this device"
        );
        let (mut mesh, mut changed) = if let Some(current) = self.meshes.get(&snapshot.mesh.id) {
            ensure!(
                !current.records_departure_at(&snapshot.mesh, source),
                "mesh unavailable"
            );
            let mut mesh = current.clone();
            verified.merge_into(&mut mesh)?;
            let changed = mesh.admissions.len() != current.admissions.len()
                || mesh.departures.len() != current.departures.len();
            (mesh, changed)
        } else {
            ensure!(!current_only, "mesh unavailable");
            ensure!(
                self.meshes.len() < MAX_CURRENT_MESHES,
                "device has reached the 64-group limit"
            );
            (snapshot.mesh.clone(), true)
        };
        if let Some(departed) = self.departed.get(&mesh.id) {
            ensure!(!current_only, "mesh unavailable");
            VerifiedSnapshot::caller_vouches_for_local_snapshot(&Snapshot::without_addresses(
                departed.clone(),
            ))
            .merge_into(&mut mesh)?;
            ensure!(
                mesh.member(self.member.id).is_some(),
                "this device left that mesh"
            );
            self.departed.remove(&mesh.id);
            changed = true;
            self.departure_order.retain(|id| *id != mesh.id);
        }
        self.meshes.insert(mesh.id, mesh);
        changed |= self.store_addresses(snapshot, source);
        self.retain_member_addresses();
        Ok(changed)
    }

    pub fn merge_departure(
        &mut self,
        verified: &VerifiedSnapshot<'_>,
        source: EndpointId,
    ) -> Result<bool> {
        let snapshot = verified.snapshot();
        let mesh = self
            .meshes
            .get_mut(&snapshot.mesh.id)
            .context("device is not a member of this mesh")?;
        ensure!(
            snapshot.mesh.departed(source),
            "peer has not left this mesh"
        );
        let before = (mesh.admissions.len(), mesh.departures.len());
        verified.merge_into(mesh)?;
        let changed = before != (mesh.admissions.len(), mesh.departures.len());
        self.retain_member_addresses();
        Ok(changed)
    }

    pub fn leave(&mut self, mesh_id: MeshId, key: &SecretKey) -> Result<Snapshot> {
        let mut mesh = self
            .meshes
            .get(&mesh_id)
            .context("device is not a member of this mesh")?
            .clone();
        mesh.depart(key)?;
        self.meshes.remove(&mesh_id);
        self.retain_member_addresses();
        if mesh.members().next().is_some() {
            self.departure_order.retain(|id| *id != mesh.id);
            if self.departed.len() == MAX_DEPARTED_MESHES && !self.departed.contains_key(&mesh.id) {
                let oldest = *self
                    .departure_order
                    .first()
                    .context("missing oldest departure")?;
                self.departed.remove(&oldest);
                self.departure_order.retain(|id| *id != oldest);
            }
            self.departed.insert(mesh.id, mesh.clone());
            self.departure_order.push(mesh.id);
        }
        Ok(Snapshot::without_addresses(mesh))
    }

    pub fn departure(&self, mesh_id: MeshId) -> Option<Snapshot> {
        self.departed
            .get(&mesh_id)
            .map(|departed| Snapshot::without_addresses(departed.clone()))
    }

    pub fn rejoin_conflict(&self, verified: &VerifiedSnapshot<'_>) -> Result<Option<Snapshot>> {
        let Some(departure) = self.departure(verified.snapshot().mesh.id) else {
            return Ok(None);
        };
        let mut merged = verified.snapshot().mesh.clone();
        VerifiedSnapshot::caller_vouches_for_local_snapshot(&departure).merge_into(&mut merged)?;
        Ok(merged.departed(self.member.id).then_some(departure))
    }

    fn store_addresses(&mut self, snapshot: &Snapshot, source: EndpointId) -> bool {
        let mut changed = false;
        for (id, addr) in &snapshot.addresses {
            if *id == self.member.id {
                continue;
            }
            let entry = self
                .addresses
                .entry(*id)
                .or_insert_with(|| AddressEntry::Current {
                    direct: None,
                    hints: BTreeMap::new(),
                });
            changed |= entry.set(
                snapshot.mesh.id,
                bounded_address(addr.clone()),
                *id == source,
            );
        }
        changed
    }

    fn retain_member_addresses(&mut self) {
        let meshes = &self.meshes;
        self.addresses.retain(|id, entry| {
            if *id == self.member.id {
                return false;
            }
            let AddressEntry::Current { direct, hints } = entry else {
                return false;
            };
            hints.retain(|mesh_id, _| {
                meshes
                    .get(mesh_id)
                    .is_some_and(|mesh| mesh.member(*id).is_some())
            });
            if !meshes.values().any(|mesh| mesh.member(*id).is_some()) {
                *direct = None;
            }
            direct.is_some() || !hints.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;

    fn setup() -> (tempfile::TempDir, Storage, State) {
        let dir = tempfile::tempdir().unwrap();
        Node::init(dir.path(), "device").unwrap();
        let (storage, state) = Storage::open(dir.path()).unwrap();
        (dir, storage, state)
    }

    fn joined_mesh(state: &mut State, key: &SecretKey, peer: &SecretKey) -> MeshId {
        let mut mesh = Mesh::create("mesh", state.member.clone(), key).unwrap();
        mesh.admit(
            Member {
                id: peer.public(),
                name: "peer".into(),
            },
            key,
        )
        .unwrap();
        let id = mesh.id;
        state.meshes.insert(mesh.id, mesh);
        id
    }

    #[test]
    fn direct_addresses_override_hints_and_self_addresses_are_discarded() {
        let (_dir, storage, mut state) = setup();
        let introducer = SecretKey::generate();
        let peer = SecretKey::generate();
        let mesh_id = joined_mesh(&mut state, &storage.key, &introducer);
        state
            .meshes
            .get_mut(&mesh_id)
            .unwrap()
            .admit(
                Member {
                    id: peer.public(),
                    name: "other".into(),
                },
                &storage.key,
            )
            .unwrap();
        let hint =
            EndpointAddr::new(peer.public()).with_ip_addr("127.0.0.1:54321".parse().unwrap());
        let direct =
            EndpointAddr::new(peer.public()).with_ip_addr("127.0.0.1:54322".parse().unwrap());
        let mut incoming = Snapshot::without_addresses(state.meshes[&mesh_id].clone());
        incoming.addresses.insert(peer.public(), hint.clone());
        incoming
            .addresses
            .insert(state.member.id, EndpointAddr::new(state.member.id));
        state
            .merge_current(
                &VerifiedSnapshot::new(&incoming).unwrap(),
                introducer.public(),
            )
            .unwrap();
        assert_eq!(
            state.addresses[&peer.public()].preferred(mesh_id),
            Some(&hint)
        );
        assert!(!state.addresses.contains_key(&state.member.id));
        incoming.addresses.insert(peer.public(), direct.clone());
        state
            .merge_current(&VerifiedSnapshot::new(&incoming).unwrap(), peer.public())
            .unwrap();
        assert_eq!(
            state.addresses[&peer.public()].preferred(mesh_id),
            Some(&direct)
        );
        state.addresses.insert(
            state.member.id,
            AddressEntry::Current {
                direct: Some(EndpointAddr::new(state.member.id)),
                hints: BTreeMap::new(),
            },
        );
        state.retain_member_addresses();
        assert!(!state.addresses.contains_key(&state.member.id));
        incoming.addresses.insert(peer.public(), hint);
        state
            .merge_current(
                &VerifiedSnapshot::new(&incoming).unwrap(),
                introducer.public(),
            )
            .unwrap();
        assert_eq!(
            state.addresses[&peer.public()].preferred(mesh_id),
            Some(&direct)
        );
    }

    #[test]
    fn old_departed_shape_and_main_state_still_open() {
        let (dir, storage, mut state) = setup();
        storage.save(&state).unwrap();
        assert!(Storage::read(dir.path()).unwrap().departed.is_empty());
        let peer = SecretKey::generate();
        let id = joined_mesh(&mut state, &storage.key, &peer);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        let mut old = serde_json::to_value(&state).unwrap();
        old["departed"] = serde_json::to_value(&state.departed[&id]).unwrap();
        old.as_object_mut().unwrap().remove("departure_order");
        write_private(
            &dir.path().join("state.json"),
            &serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        let reopened = Storage::read(dir.path()).unwrap();
        assert!(reopened.departed.contains_key(&id));
        assert_eq!(reopened.departure_order, [id]);
        storage.save(&reopened).unwrap();
        assert_eq!(Storage::read(dir.path()).unwrap().departure_order, [id]);
    }

    #[test]
    fn origin_main_state_shape_migrates_without_changing_signatures() {
        let (dir, storage, state) = setup();
        let mut mesh = Mesh::create_legacy("original", state.member.clone(), &storage.key).unwrap();
        let peer = SecretKey::generate();
        mesh.admit(
            Member {
                id: peer.public(),
                name: "peer".into(),
            },
            &storage.key,
        )
        .unwrap();
        let signed = serde_json::to_value(&mesh).unwrap();
        assert!(signed.get("departures").is_none());
        assert!(signed["admissions"][0].get("generation").is_none());
        let legacy = serde_json::json!({"member": state.member, "mesh": signed, "addresses": {}});
        write_private(
            &dir.path().join("state.json"),
            &serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        drop(storage);
        let (storage, reopened) = Storage::open(dir.path()).unwrap();
        assert_eq!(reopened.meshes.len(), 1);
        assert_eq!(reopened.info().meshes[0].members.len(), 2);
        assert_eq!(
            serde_json::to_value(&reopened.meshes[&mesh.id]).unwrap(),
            signed
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(dir.path().join("state.json")).unwrap()
            )
            .unwrap()["mesh"],
            MULTI_MESH_SENTINEL
        );
        drop(storage);
    }

    #[test]
    fn legacy_single_mesh_with_departed_copies_migrates_without_resigning() {
        let (dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let departed_id = joined_mesh(&mut state, &storage.key, &peer);
        state.leave(departed_id, &storage.key).unwrap();
        let current = Mesh::create("old group", state.member.clone(), &storage.key).unwrap();
        let id = current.id;
        let signed = serde_json::to_value(&current).unwrap();
        let mut legacy = serde_json::to_value(&state).unwrap();
        legacy.as_object_mut().unwrap().remove("meshes");
        legacy["mesh"] = signed.clone();
        write_private(
            &dir.path().join("state.json"),
            &serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        drop(storage);
        let (storage, reopened) = Storage::open(dir.path()).unwrap();
        assert_eq!(reopened.meshes.len(), 1);
        assert_eq!(reopened.info().mesh_id, Some(id));
        assert_eq!(serde_json::to_value(&reopened.meshes[&id]).unwrap(), signed);
        assert!(reopened.departed.contains_key(&departed_id));
        let migrated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("state.json")).unwrap()).unwrap();
        assert_eq!(migrated["mesh"], MULTI_MESH_SENTINEL);
        assert_eq!(migrated["meshes"][id.to_string()], signed);
        drop(storage);
    }

    #[test]
    fn hints_from_one_mesh_are_not_forwarded_to_another() {
        let (_dir, storage, mut state) = setup();
        let introducer = SecretKey::generate();
        let shared = SecretKey::generate();
        let first = joined_mesh(&mut state, &storage.key, &introducer);
        let second = joined_mesh(&mut state, &storage.key, &shared);
        state
            .meshes
            .get_mut(&first)
            .unwrap()
            .admit(
                Member {
                    id: shared.public(),
                    name: "shared".into(),
                },
                &storage.key,
            )
            .unwrap();
        let mut incoming = Snapshot::without_addresses(state.meshes[&first].clone());
        let hint =
            EndpointAddr::new(shared.public()).with_ip_addr("127.0.0.1:54321".parse().unwrap());
        incoming.addresses.insert(shared.public(), hint.clone());
        assert!(state
            .join(
                &VerifiedSnapshot::new(&incoming).unwrap(),
                introducer.public()
            )
            .unwrap());
        assert_eq!(
            state.addresses[&shared.public()].preferred(first),
            Some(&hint)
        );
        let newer =
            EndpointAddr::new(shared.public()).with_ip_addr("127.0.0.1:54322".parse().unwrap());
        incoming.addresses.insert(shared.public(), newer.clone());
        assert!(state
            .merge_current(
                &VerifiedSnapshot::new(&incoming).unwrap(),
                introducer.public()
            )
            .unwrap());
        assert_eq!(
            state.addresses[&shared.public()].preferred(first),
            Some(&newer)
        );
        assert!(!state
            .snapshot(second, EndpointAddr::new(state.member.id))
            .unwrap()
            .addresses
            .contains_key(&shared.public()));
        assert!(state
            .snapshot(first, EndpointAddr::new(state.member.id))
            .unwrap()
            .addresses
            .contains_key(&shared.public()));
        let mut second_hint = Snapshot::without_addresses(state.meshes[&second].clone());
        second_hint.addresses.insert(shared.public(), hint.clone());
        state
            .merge_current(
                &VerifiedSnapshot::new(&second_hint).unwrap(),
                state.member.id,
            )
            .unwrap();
        assert_eq!(
            state.addresses[&shared.public()].preferred(first),
            Some(&newer)
        );
        assert_eq!(
            state.addresses[&shared.public()].preferred(second),
            Some(&hint)
        );
        let mut direct = Snapshot::without_addresses(state.meshes[&second].clone());
        direct.addresses.insert(shared.public(), hint.clone());
        state
            .merge_current(&VerifiedSnapshot::new(&direct).unwrap(), shared.public())
            .unwrap();
        assert_eq!(
            state.addresses[&shared.public()].preferred(first),
            Some(&hint)
        );
        incoming.addresses.insert(shared.public(), newer);
        state
            .merge_current(
                &VerifiedSnapshot::new(&incoming).unwrap(),
                introducer.public(),
            )
            .unwrap();
        assert_eq!(
            state.addresses[&shared.public()].preferred(first),
            Some(&hint)
        );
        assert!(state
            .snapshot(first, EndpointAddr::new(state.member.id))
            .unwrap()
            .addresses
            .contains_key(&shared.public()));
        assert!(state
            .snapshot(second, EndpointAddr::new(state.member.id))
            .unwrap()
            .addresses
            .contains_key(&shared.public()));
        state.leave(first, &storage.key).unwrap();
        if let AddressEntry::Current { hints, .. } = &state.addresses[&shared.public()] {
            assert!(!hints.contains_key(&first));
        } else {
            panic!("address was not migrated");
        }
    }

    #[test]
    fn sync_after_concurrent_leave_cannot_rejoin_without_a_retained_copy() {
        let (_dir, storage, mut state) = setup();
        let solo = Mesh::create("solo", state.member.clone(), &storage.key).unwrap();
        let snapshot = Snapshot::without_addresses(solo.clone());
        state.meshes.insert(solo.id, solo);
        state.leave(snapshot.mesh.id, &storage.key).unwrap();
        assert!(state.departed.is_empty());
        assert!(state
            .merge_current(&VerifiedSnapshot::new(&snapshot).unwrap(), state.member.id)
            .is_err());
        assert!(state.meshes.is_empty());
    }

    #[test]
    fn historical_state_fixtures_migrate_and_block_old_readers() {
        #[derive(Deserialize)]
        struct MainState {
            member: Member,
            mesh: Option<Mesh>,
            addresses: BTreeMap<EndpointId, EndpointAddr>,
        }

        #[derive(Deserialize)]
        struct LeaveState {
            member: Member,
            mesh: Option<Mesh>,
            addresses: BTreeMap<EndpointId, EndpointAddr>,
            #[serde(default, deserialize_with = "read_departed")]
            departed: BTreeMap<MeshId, Mesh>,
            #[serde(default)]
            departure_order: Vec<MeshId>,
        }

        let main = include_bytes!("../tests/fixtures/main-state.json");
        let leave = include_bytes!("../tests/fixtures/leave-state.json");
        let old_main: MainState = serde_json::from_slice(main).unwrap();
        let old_leave: LeaveState = serde_json::from_slice(leave).unwrap();
        assert!(old_main.mesh.is_some());
        assert_eq!(old_main.addresses.len(), 1);
        assert!(old_leave.mesh.is_some());
        assert_eq!(old_leave.departed.len(), 1);
        assert_eq!(old_leave.departure_order.len(), 1);
        assert_eq!(old_leave.addresses.len(), 1);
        for (bytes, id) in [
            (main.as_slice(), old_main.member.id),
            (leave.as_slice(), old_leave.member.id),
        ] {
            let (dir, storage, _) = setup();
            write_private(&dir.path().join("state.json"), bytes).unwrap();
            let migrated = Storage::read(dir.path()).unwrap();
            assert_eq!(migrated.member.id, id);
            let mesh_id = *migrated.meshes.keys().next().unwrap();
            let legacy_address = if id == old_main.member.id {
                &old_main.addresses
            } else {
                &old_leave.addresses
            };
            for (peer_id, address) in legacy_address {
                if *peer_id == id {
                    assert!(!migrated.addresses.contains_key(peer_id));
                    continue;
                }
                match &migrated.addresses[peer_id] {
                    AddressEntry::Current { direct, hints } => {
                        assert!(direct.is_none());
                        assert_eq!(hints.get(&mesh_id), Some(address));
                    }
                    AddressEntry::Legacy(_) => panic!("unmigrated address"),
                }
            }
            let signed = serde_json::to_value(migrated.meshes.values().next().unwrap()).unwrap();
            storage.save(&migrated).unwrap();
            let bytes = std::fs::read(dir.path().join("state.json")).unwrap();
            assert!(serde_json::from_slice::<MainState>(&bytes).is_err());
            assert!(serde_json::from_slice::<LeaveState>(&bytes).is_err());
            let reopened = Storage::read(dir.path()).unwrap();
            assert_eq!(
                serde_json::to_value(reopened.meshes.values().next().unwrap()).unwrap(),
                signed
            );
        }
    }

    #[test]
    fn reading_rejects_invalid_departed_copies() {
        let (dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let id = joined_mesh(&mut state, &storage.key, &peer);
        let active = state.meshes.values().next().cloned().unwrap();
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        let valid = state.clone();
        let mut invalid = valid.clone();
        invalid.departed.insert(id, active.clone());
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("does not record"));
        let mut invalid = valid.clone();
        invalid.meshes.insert(active.id, active);
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("cannot be a member"));
        let mut invalid = valid.clone();
        invalid.departed.get_mut(&id).unwrap().name = "tampered".into();
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("invalid admission signature"));
        let mut invalid = valid.clone();
        let wrong = MeshId::generate().unwrap();
        let copy = invalid.departed.remove(&id).unwrap();
        invalid.departed.insert(wrong, copy);
        invalid.departure_order[0] = wrong;
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("does not match its key"));
    }

    #[test]
    fn reading_rejects_duplicate_and_dangling_departure_order_ids() {
        let (dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let id = joined_mesh(&mut state, &storage.key, &peer);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        let mut duplicate = state.clone();
        duplicate.departure_order.push(id);
        storage.save(&duplicate).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("invalid departure order"));
        state.departure_order.push(MeshId::generate().unwrap());
        storage.save(&state).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("invalid departure order"));
    }

    #[test]
    fn migrated_departures_are_appended_after_recorded_order() {
        let (dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let first = joined_mesh(&mut state, &storage.key, &peer);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        let second = joined_mesh(&mut state, &storage.key, &peer);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        state.departure_order.remove(0);
        storage.save(&state).unwrap();
        let reopened = Storage::read(dir.path()).unwrap();
        assert_eq!(reopened.departure_order, [second, first]);
        let mut reopened = reopened;
        for _ in 0..MAX_DEPARTED_MESHES - 1 {
            joined_mesh(&mut reopened, &storage.key, &peer);
            reopened
                .leave(
                    reopened.meshes.keys().next().copied().unwrap(),
                    &storage.key,
                )
                .unwrap();
        }
        assert!(!reopened.departed.contains_key(&second));
        assert!(reopened.departed.contains_key(&first));
    }

    #[test]
    fn departure_retention_evicts_oldest_and_discards_solo_meshes() {
        let (_dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let first = joined_mesh(&mut state, &storage.key, &peer);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        for _ in 1..=MAX_DEPARTED_MESHES {
            joined_mesh(&mut state, &storage.key, &peer);
            state
                .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
                .unwrap();
        }
        assert_eq!(state.departed.len(), MAX_DEPARTED_MESHES);
        assert!(!state.departed.contains_key(&first));
        let mut invalid = state.clone();
        joined_mesh(&mut invalid, &storage.key, &peer);
        invalid
            .leave(invalid.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        invalid.departed.insert(
            first,
            Mesh::create("bad", invalid.member.clone(), &storage.key).unwrap(),
        );
        storage.save(&invalid).unwrap();
        assert!(Storage::read(&storage.root)
            .unwrap_err()
            .to_string()
            .contains("too many departed meshes"));
        let solo = Mesh::create("solo", state.member.clone(), &storage.key).unwrap();
        let solo_id = solo.id;
        state.meshes.insert(solo.id, solo);
        state
            .leave(state.meshes.keys().next().copied().unwrap(), &storage.key)
            .unwrap();
        assert_eq!(state.departed.len(), MAX_DEPARTED_MESHES);
        assert!(!state.departed.contains_key(&solo_id));
    }
}
