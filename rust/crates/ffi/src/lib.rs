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
    #[error("destination io: {0}")]
    Destination(String),
    #[error("blob {0} not in store")]
    Missing(String),
    #[error("blob {expected} is corrupt (hashes to {actual})")]
    Corrupt { expected: String, actual: String },
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("node is closed")]
    NodeClosed,
    #[error("node directory is in use")]
    NodeBusy,
    #[error("device has reached the 64-group limit")]
    MeshLimit,
    #[error("not a member: {0}")]
    NotMember(String),
    #[error("ticket rejected: {0}")]
    TicketRejected(String),
    #[error("network unavailable: {0}")]
    Unavailable(String),
    #[error("node: {0}")]
    Node(String),
}

impl From<StoreError> for FfiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Io(e) => FfiError::Io(e.to_string()),
            StoreError::Destination(e) => FfiError::Destination(e.to_string()),
            StoreError::NotFound(hash) => FfiError::Missing(hash.to_string()),
            StoreError::Corrupt { expected, actual } => FfiError::Corrupt {
                expected: expected.to_string(),
                actual: actual.to_string(),
            },
            StoreError::InvalidHash(e) => FfiError::Invalid(e.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn store_errors_keep_their_ffi_categories() {
        let hash = BlobHash::of(b"missing");
        assert!(matches!(
            FfiError::from(StoreError::NotFound(hash)),
            FfiError::Missing(_)
        ));
        assert!(matches!(
            FfiError::from(StoreError::InvalidHash(
                "invalid".parse::<BlobHash>().unwrap_err()
            )),
            FfiError::Invalid(_)
        ));
        assert!(matches!(
            FfiError::from(StoreError::Destination(io::Error::other("write failed"))),
            FfiError::Destination(_)
        ));
    }
}
