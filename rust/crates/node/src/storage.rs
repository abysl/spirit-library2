use crate::membership::{validate_name, Member, Mesh, Snapshot};
use crate::NodeInfo;
use anyhow::{ensure, Context, Result};
use iroh::{EndpointAddr, EndpointId, SecretKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct State {
    pub member: Member,
    pub mesh: Option<Mesh>,
    pub addresses: BTreeMap<EndpointId, EndpointAddr>,
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
                .map(|mesh| mesh.admissions.iter().map(|a| a.member.clone()).collect())
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
        match &mut self.mesh {
            Some(mesh) => mesh.merge(&snapshot.mesh)?,
            None => self.mesh = Some(snapshot.mesh.clone()),
        }
        for (id, addr) in &snapshot.addresses {
            if *id == source || !self.addresses.contains_key(id) {
                self.addresses.insert(*id, addr.clone());
            }
        }
        Ok(())
    }
}
