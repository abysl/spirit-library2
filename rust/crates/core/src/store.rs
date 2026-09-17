use std::fmt;
use std::path::{Path, PathBuf};

use crate::BlobHash;

#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError {
    Io(std::io::Error),
    Missing(BlobHash),
    Corrupt {
        expected: BlobHash,
        actual: BlobHash,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "store io: {e}"),
            StoreError::Missing(hash) => write!(f, "blob {hash} not in store"),
            StoreError::Corrupt { expected, actual } => {
                write!(f, "blob {expected} is corrupt (hashes to {actual})")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_of(&self, hash: BlobHash) -> PathBuf {
        self.root.join(hash.to_string())
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobHash, StoreError> {
        let hash = BlobHash::of(bytes);
        let path = self.path_of(hash);
        if path.exists() {
            return Ok(hash);
        }
        let temp = self.root.join(format!("tmp-{hash}-{}", std::process::id()));
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, &path)?;
        Ok(hash)
    }

    pub fn get(&self, hash: BlobHash) -> Result<Vec<u8>, StoreError> {
        let bytes = match std::fs::read(self.path_of(hash)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::Missing(hash))
            }
            Err(e) => return Err(e.into()),
        };
        let actual = BlobHash::of(&bytes);
        if actual != hash {
            return Err(StoreError::Corrupt {
                expected: hash,
                actual,
            });
        }
        Ok(bytes)
    }

    pub fn has(&self, hash: BlobHash) -> bool {
        self.path_of(hash).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_store(tag: &str) -> BlobStore {
        let dir = std::env::temp_dir().join(format!("spirit-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        BlobStore::open(dir).unwrap()
    }

    #[test]
    fn roundtrip() {
        let store = scratch_store("roundtrip");
        let hash = store.put(b"there and back again").unwrap();
        assert_eq!(store.get(hash).unwrap(), b"there and back again");
        assert!(store.has(hash));
    }

    #[test]
    fn put_is_idempotent_and_content_addressed() {
        let store = scratch_store("idempotent");
        let first = store.put(b"riddles in the dark").unwrap();
        let second = store.put(b"riddles in the dark").unwrap();
        assert_eq!(first, second);
        let other = store.put(b"an unexpected party").unwrap();
        assert_ne!(first, other);
        assert_eq!(std::fs::read_dir(store.root()).unwrap().count(), 2);
    }

    #[test]
    fn missing_blob_is_an_error() {
        let store = scratch_store("missing");
        let hash = BlobHash::of(b"never stored");
        assert!(!store.has(hash));
        assert!(matches!(store.get(hash), Err(StoreError::Missing(_))));
    }

    #[test]
    fn corruption_is_detected_on_read() {
        let store = scratch_store("corrupt");
        let hash = store.put(b"the real contents").unwrap();
        std::fs::write(store.path_of(hash), b"tampered").unwrap();
        assert!(matches!(store.get(hash), Err(StoreError::Corrupt { .. })));
    }
}
