use crate::membership::{validate_name, Member, Mesh, Snapshot};
use crate::mesh_id::MeshId;
use crate::NodeInfo;
use anyhow::{ensure, Context, Result};
use iroh::{EndpointAddr, EndpointId, SecretKey};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_DEPARTED_MESHES: usize = 64;

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
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct State {
    pub member: Member,
    pub mesh: Option<Mesh>,
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
                mesh: None,
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
        Ok(state.info())
    }

    pub fn open(root: &Path) -> Result<(Self, State)> {
        ensure!(
            root.join("state.json").is_file(),
            "node is not initialized; run spirit node init"
        );
        let guard = lock(root)?;
        let key = read_key(root)?;
        let state = Self::read(root)?;
        ensure!(
            state.member.id == key.public(),
            "identity does not match state"
        );
        Ok((
            Self {
                root: root.into(),
                key,
                _lock: guard,
            },
            state,
        ))
    }

    pub fn read(root: &Path) -> Result<State> {
        let state: State = serde_json::from_slice(
            &std::fs::read(root.join("state.json"))
                .context("node is not initialized; run spirit node init")?,
        )?;
        validate_name(&state.member.name)?;
        if let Some(mesh) = &state.mesh {
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
                state.mesh.as_ref().map(|mesh| mesh.id) != Some(*id),
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
        NodeInfo {
            id: self.member.id,
            name: self.member.name.clone(),
            mesh_id: self.mesh.as_ref().map(|mesh| mesh.id),
            mesh_name: self.mesh.as_ref().map(|mesh| mesh.name.clone()),
            members: self
                .mesh
                .as_ref()
                .map(|mesh| mesh.members().cloned().collect())
                .unwrap_or_default(),
        }
    }

    pub fn snapshot(&self, addr: EndpointAddr) -> Result<Snapshot> {
        let mesh = self
            .mesh
            .clone()
            .context("device is not a mesh member; create a mesh or enroll this device first")?;
        let mut addresses = self.addresses.clone();
        addresses.insert(addr.id, addr);
        Ok(Snapshot { mesh, addresses })
    }

    pub fn merge(&mut self, snapshot: &Snapshot, source: EndpointId) -> Result<()> {
        snapshot.verify()?;
        ensure!(
            snapshot.mesh.member(self.member.id) == Some(&self.member),
            "membership does not match this device"
        );
        let mut mesh = match &self.mesh {
            Some(mesh) => {
                let mut mesh = mesh.clone();
                mesh.merge(&snapshot.mesh)?;
                mesh
            }
            None => snapshot.mesh.clone(),
        };
        if let Some(departed) = self.departed.get(&mesh.id) {
            mesh.merge(departed)?;
            ensure!(
                mesh.member(self.member.id).is_some(),
                "this device left that mesh"
            );
            self.departed.remove(&mesh.id);
            self.departure_order.retain(|id| *id != mesh.id);
        }
        self.mesh = Some(mesh);
        for (id, addr) in &snapshot.addresses {
            if *id == source || !self.addresses.contains_key(id) {
                self.addresses.insert(*id, addr.clone());
            }
        }
        self.retain_member_addresses();
        Ok(())
    }

    pub fn merge_departure(&mut self, snapshot: &Snapshot, source: EndpointId) -> Result<()> {
        let mesh = self.mesh.as_mut().context("device is not a mesh member")?;
        ensure!(mesh.id == snapshot.mesh.id, "different meshes cannot merge");
        ensure!(
            snapshot.mesh.departed(source),
            "peer has not left this mesh"
        );
        mesh.merge(&snapshot.mesh)?;
        self.retain_member_addresses();
        Ok(())
    }

    pub fn leave(&mut self, key: &SecretKey) -> Result<Snapshot> {
        let mut mesh = self.mesh.clone().context("device is not a mesh member")?;
        mesh.depart(key)?;
        self.mesh = None;
        self.addresses.clear();
        if mesh.members().next().is_some() {
            self.departure_order.retain(|id| *id != mesh.id);
            if self.departed.len() == MAX_DEPARTED_MESHES && !self.departed.contains_key(&mesh.id) {
                let oldest = self
                    .departed
                    .keys()
                    .find(|id| !self.departure_order.contains(id))
                    .copied()
                    .or_else(|| self.departure_order.first().copied())
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
            .mesh
            .as_ref()
            .map(|mesh| mesh.members().map(|member| member.id).collect())
            .unwrap_or_default();
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
        state.mesh = Some(mesh);
        id
    }

    #[test]
    fn old_departed_shape_and_main_state_still_open() {
        let (dir, storage, mut state) = setup();
        storage.save(&state).unwrap();
        assert!(Storage::read(dir.path()).unwrap().departed.is_empty());
        let peer = SecretKey::generate();
        let id = joined_mesh(&mut state, &storage.key, &peer);
        state.leave(&storage.key).unwrap();
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
        storage.save(&reopened).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap()
            .departed
            .contains_key(&id));
    }

    #[test]
    fn reading_rejects_invalid_departed_copies() {
        let (dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let id = joined_mesh(&mut state, &storage.key, &peer);
        let active = state.mesh.clone().unwrap();
        state.leave(&storage.key).unwrap();
        let valid = state.clone();
        let mut invalid = valid.clone();
        invalid.departed.insert(id, active.clone());
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("does not record"));
        let mut invalid = valid.clone();
        invalid.mesh = Some(active);
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path())
            .unwrap_err()
            .to_string()
            .contains("cannot be a member"));
        let mut invalid = valid.clone();
        invalid.departed.get_mut(&id).unwrap().name = "tampered".into();
        storage.save(&invalid).unwrap();
        assert!(Storage::read(dir.path()).is_err());
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
    fn departure_retention_evicts_oldest_and_discards_solo_meshes() {
        let (_dir, storage, mut state) = setup();
        let peer = SecretKey::generate();
        let first = joined_mesh(&mut state, &storage.key, &peer);
        state.leave(&storage.key).unwrap();
        for _ in 1..=MAX_DEPARTED_MESHES {
            joined_mesh(&mut state, &storage.key, &peer);
            state.leave(&storage.key).unwrap();
        }
        assert_eq!(state.departed.len(), MAX_DEPARTED_MESHES);
        assert!(!state.departed.contains_key(&first));
        let mut invalid = state.clone();
        let extra = joined_mesh(&mut invalid, &storage.key, &peer);
        invalid.leave(&storage.key).unwrap();
        invalid.departed.insert(
            first,
            Mesh::create("bad", invalid.member.clone(), &storage.key).unwrap(),
        );
        storage.save(&invalid).unwrap();
        assert!(Storage::read(&storage.root)
            .unwrap_err()
            .to_string()
            .contains("too many departed meshes"));
        assert!(state.departure(extra).is_none());
        state.mesh = Some(Mesh::create("solo", state.member.clone(), &storage.key).unwrap());
        state.leave(&storage.key).unwrap();
        assert_eq!(state.departed.len(), MAX_DEPARTED_MESHES);
    }
}
