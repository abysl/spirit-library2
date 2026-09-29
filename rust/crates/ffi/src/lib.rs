#[cfg(target_os = "android")]
mod android;
mod node;
pub use node::*;

use spirit_sdk::{BlobHash, BlobStore, StoreError};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("node blob store is not configured")]
    StoreNotConfigured,
    #[error("blob transfer interrupted")]
    Interrupted,
    #[error("fetch cancelled")]
    Cancelled,
    #[error("transfer timed out")]
    Timeout,
    #[error("store io: {0}")]
    Io(String),
    #[error("source read: {0}")]
    SourceRead(String),
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

impl From<uniffi::UnexpectedUniFFICallbackError> for FfiError {
    fn from(error: uniffi::UnexpectedUniFFICallbackError) -> Self {
        FfiError::Node(error.reason)
    }
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

fn node_error(error: anyhow::Error) -> FfiError {
    node_error_with_hash(error, None)
}

fn node_error_with_hash(error: anyhow::Error, expected_hash: Option<BlobHash>) -> FfiError {
    for cause in error.chain() {
        let callback = cause.downcast_ref::<FfiError>().or_else(|| {
            let store = cause.downcast_ref::<StoreError>()?;
            match store {
                StoreError::Io(io) | StoreError::Destination(io) => {
                    io.get_ref()?.downcast_ref::<FfiError>()
                }
                _ => None,
            }
        });
        if let Some(callback) = callback {
            return match callback {
                FfiError::SourceRead(message) => FfiError::SourceRead(message.clone()),
                FfiError::Destination(message) => FfiError::Destination(message.clone()),
                FfiError::NodeClosed => FfiError::NodeClosed,
                _ => FfiError::Node(callback.to_string()),
            };
        }
    }
    let detail = format!("{error:#}");
    if let Some(error) = error.downcast_ref::<spirit_sdk::FetchError>() {
        return match error {
            spirit_sdk::FetchError::Node(error) => map_node_error(error, detail),
            spirit_sdk::FetchError::Interrupted => FfiError::Interrupted,
            spirit_sdk::FetchError::Cancelled => FfiError::Cancelled,
            spirit_sdk::FetchError::Corrupt => FfiError::Corrupt {
                expected: expected_hash.map_or_else(String::new, |hash| hash.to_string()),
                actual: String::new(),
            },
            spirit_sdk::FetchError::Timeout => FfiError::Timeout,
            spirit_sdk::FetchError::Io(_) => FfiError::Io(detail),
        };
    }
    if let Some(error) = error.downcast_ref::<spirit_sdk::NodeError>() {
        return map_node_error(error, detail);
    }
    if let Some(error) = error.downcast_ref::<StoreError>() {
        return match error {
            StoreError::Io(_) => FfiError::Io(detail),
            StoreError::Destination(_) => FfiError::Destination(detail),
            StoreError::NotFound(hash) => FfiError::Missing(hash.to_string()),
            StoreError::Corrupt { expected, actual } => FfiError::Corrupt {
                expected: expected.to_string(),
                actual: actual.to_string(),
            },
            StoreError::InvalidHash(_) => FfiError::Invalid(detail),
        };
    }
    FfiError::Node(detail)
}

fn map_node_error(error: &spirit_sdk::NodeError, detail: String) -> FfiError {
    match error {
        spirit_sdk::NodeError::MeshLimit => FfiError::MeshLimit,
        spirit_sdk::NodeError::NotMember => FfiError::NotMember(detail),
        spirit_sdk::NodeError::TicketRejected(_) => FfiError::TicketRejected(detail),
        spirit_sdk::NodeError::NodeClosed => FfiError::NodeClosed,
        spirit_sdk::NodeError::StoreNotConfigured => FfiError::StoreNotConfigured,
        spirit_sdk::NodeError::NodeBusy => FfiError::NodeBusy,
        spirit_sdk::NodeError::Invalid(_) => FfiError::Invalid(detail),
        spirit_sdk::NodeError::Unavailable(_) => FfiError::Unavailable(detail),
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
            store: BlobStore::open(store_dir).map_err(|error| match &error {
                StoreError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock => {
                    FfiError::NodeBusy
                }
                _ => error.into(),
            })?,
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

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use std::io;

    #[test]
    fn maps_all_transfer_and_store_failures_without_text_matching() {
        let hash = BlobHash::of(b"expected");
        let actual = BlobHash::of(b"actual");
        assert!(matches!(
            node_error(spirit_sdk::NodeError::StoreNotConfigured.into()),
            FfiError::StoreNotConfigured
        ));
        assert!(matches!(
            node_error(spirit_sdk::FetchError::Interrupted.into()),
            FfiError::Interrupted
        ));
        assert!(matches!(
            node_error(spirit_sdk::FetchError::Timeout.into()),
            FfiError::Timeout
        ));
        assert!(matches!(
            node_error(spirit_sdk::FetchError::Corrupt.into()),
            FfiError::Corrupt { .. }
        ));
        assert!(matches!(
            node_error(spirit_sdk::FetchError::Io(io::Error::other("test")).into()),
            FfiError::Io(_)
        ));
        assert!(matches!(
            node_error(StoreError::NotFound(hash).into()),
            FfiError::Missing(_)
        ));
        assert!(matches!(
            node_error(
                StoreError::Corrupt {
                    expected: hash,
                    actual
                }
                .into()
            ),
            FfiError::Corrupt { .. }
        ));
        assert!(matches!(
            node_error(StoreError::Destination(io::Error::other("test")).into()),
            FfiError::Destination(_)
        ));
        assert!(matches!(
            node_error(StoreError::Io(io::Error::other("test")).into()),
            FfiError::Io(_)
        ));
        assert!(matches!(
            node_error(spirit_sdk::NodeError::NodeBusy.into()),
            FfiError::NodeBusy
        ));
        assert!(matches!(
            node_error(spirit_sdk::FetchError::Node(spirit_sdk::NodeError::NodeBusy).into()),
            FfiError::NodeBusy
        ));
        assert!(matches!(
            node_error(StoreError::InvalidHash("bad".parse::<BlobHash>().unwrap_err()).into()),
            FfiError::Invalid(_)
        ));
        assert!(matches!(
            node_error(anyhow::anyhow!("unknown")),
            FfiError::Node(_)
        ));
    }
}
