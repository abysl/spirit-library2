use crate::membership::{validate_name, Member, Mesh, Snapshot};
use crate::mesh_id::MeshId;
use crate::NodeInfo;
use anyhow::{ensure, Context, Result};
use iroh::{EndpointAddr, EndpointId, SecretKey};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_DEPARTED_MESHES: usize = 64;
pub(crate) const MAX_CURRENT_MESHES: usize = 64;

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
    #[serde(default, rename = "mesh", skip_serializing)]
    legacy_mesh: Option<Mesh>,
    pub addresses: BTreeMap<EndpointId, EndpointAddr>,
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
                departed: BTreeMap::new(),
                departure_order: Vec::new(),
            })?;
        }
        let state = Self::read(root)?;
        ensure!(
            state.member.id == storage.key.public(),
            "identity does not match state"
        );
        if state.legacy_mesh.is_some() {
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
        if state.legacy_mesh.take().is_some() {
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
        for (id, mesh) in &state.meshes {
            ensure!(*id == mesh.id, "mesh ID does not match its key");
            mesh.verify()?;
            ensure!(
                mesh.member(state.member.id) == Some(&state.member),
                "device is missing from its mesh"
            );
        }
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
        let members: BTreeMap<_, _> = self
            .meshes
            .values()
            .flat_map(|mesh| mesh.members())
            .map(|member| (member.id, member.clone()))
            .collect();
        NodeInfo {
            id: self.member.id,
            name: self.member.name.clone(),
            mesh_id: only.map(|mesh| mesh.id),
            mesh_name: only.map(|mesh| mesh.name.clone()),
            members: members.into_values().collect(),
            meshes: self
                .meshes
                .values()
                .map(|mesh| crate::MeshInfo {
                    id: mesh.id,
                    name: mesh.name.clone(),
                    members: mesh
                        .members()
                        .map(|member| crate::MeshMember {
                            id: member.id,
                            name: member.name.clone(),
                            generation: mesh.generation(member.id).unwrap(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    pub fn snapshot(&self, mesh_id: MeshId, addr: EndpointAddr) -> Result<Snapshot> {
        let mesh = self
            .meshes
            .get(&mesh_id)
            .context("device is not a member of this mesh")?
            .clone();
        let mut addresses = self.addresses.clone();
        addresses.insert(addr.id, addr);
        addresses.retain(|id, _| mesh.member(*id).is_some());
        Ok(Snapshot { mesh, addresses })
    }

    pub fn merge(&mut self, snapshot: &Snapshot, source: EndpointId) -> Result<()> {
        ensure!(
            snapshot.mesh.member(self.member.id) == Some(&self.member),
            "membership does not match this device"
        );
        let mut mesh = if let Some(current) = self.meshes.get(&snapshot.mesh.id) {
            let mut mesh = current.clone();
            mesh.merge_verified(&snapshot.mesh)?;
            mesh
        } else {
            ensure!(
                self.meshes.len() < MAX_CURRENT_MESHES,
                "device has reached the 64-group limit"
            );
            snapshot.mesh.clone()
        };
        if let Some(departed) = self.departed.get(&mesh.id) {
            mesh.merge_verified(departed)?;
            ensure!(
                mesh.member(self.member.id).is_some(),
                "this device left that mesh"
            );
            self.departed.remove(&mesh.id);
            self.departure_order.retain(|id| *id != mesh.id);
        }
        self.meshes.insert(mesh.id, mesh);
        for (id, addr) in &snapshot.addresses {
            if *id == source || !self.addresses.contains_key(id) {
                self.addresses.insert(*id, addr.clone());
            }
        }
        self.retain_member_addresses();
        Ok(())
    }

    pub fn merge_departure(&mut self, snapshot: &Snapshot, source: EndpointId) -> Result<()> {
        let mesh = self
            .meshes
            .get_mut(&snapshot.mesh.id)
            .context("device is not a member of this mesh")?;
        ensure!(
            snapshot.mesh.departed(source),
            "peer has not left this mesh"
        );
        mesh.merge_verified(&snapshot.mesh)?;
        self.retain_member_addresses();
        Ok(())
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

    pub fn rejoin_conflict(&self, mesh: &Mesh) -> Result<Option<Snapshot>> {
        let Some(departure) = self.departure(mesh.id) else {
            return Ok(None);
        };
        let mut merged = mesh.clone();
        merged.merge(&departure.mesh)?;
        Ok(merged.departed(self.member.id).then_some(departure))
    }

    fn retain_member_addresses(&mut self) {
        let members: BTreeSet<_> = self
            .meshes
            .values()
            .flat_map(|mesh| mesh.members().map(|member| member.id))
            .collect();
        self.addresses.retain(|id, _| members.contains(id));
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
        let mut mesh = Mesh::create("original", state.member.clone(), &storage.key).unwrap();
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
        assert!(serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(dir.path().join("state.json")).unwrap()
        )
        .unwrap()
        .get("mesh")
        .is_none());
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
        assert!(migrated.get("mesh").is_none());
        assert_eq!(migrated["meshes"][id.to_string()], signed);
        drop(storage);
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
