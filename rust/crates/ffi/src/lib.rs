#[cfg(target_os = "android")]
mod android;
mod node;
pub use node::*;

use spirit_sdk::{BlobHash, BlobStore, StoreError};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("store io: {0}")]
    Io(String),
    #[error("blob {0} not in store")]
    Missing(String),
    #[error("blob {expected} is corrupt (hashes to {actual})")]
    Corrupt { expected: String, actual: String },
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("node: {0}")]
    Node(String),
}

impl From<StoreError> for FfiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io(e) => FfiError::Io(e.to_string()),
            StoreError::Missing(hash) => FfiError::Missing(hash.to_string()),
            StoreError::Corrupt { expected, actual } => FfiError::Corrupt {
                expected: expected.to_string(),
                actual: actual.to_string(),
            },
            other => FfiError::Io(other.to_string()),
        }
    }
}

fn parse_hash(text: &str) -> Result<BlobHash, FfiError> {
    text.parse()
        .map_err(|e: spirit_sdk::ParseHashError| FfiError::Invalid(e.to_string()))
}

#[derive(uniffi::Object)]
pub struct SpiritStore {
    store: BlobStore,
}

#[uniffi::export]
impl SpiritStore {
    #[uniffi::constructor]
    pub fn open(store_dir: String) -> Result<Self, FfiError> {
        Ok(Self {
            store: BlobStore::open(store_dir)?,
        })
    }

    pub fn put_blob(&self, bytes: Vec<u8>) -> Result<String, FfiError> {
        Ok(self.store.put(&bytes)?.to_string())
    }

    pub fn get_blob(&self, hash: String) -> Result<Vec<u8>, FfiError> {
        Ok(self.store.get(parse_hash(&hash)?)?)
    }

    pub fn has_blob(&self, hash: String) -> Result<bool, FfiError> {
        Ok(self.store.has(parse_hash(&hash)?))
    }
}
