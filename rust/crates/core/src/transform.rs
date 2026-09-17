use std::collections::BTreeMap;
use std::fmt;

use crate::BlobHash;

const FORMAT_VERSION: u16 = 1;
const NIX_EXECUTOR_TAG: u8 = 1;
const ASSET_MANIFEST_OUTPUT_TAG: u8 = 1;
const TRANSFORM_HEADER: &[u8] = b"spirit2-transform";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformExecutor {
    Nix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformOutput {
    AssetManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputName(String);

impl InputName {
    pub fn new(name: impl Into<String>) -> Result<Self, TransformDocumentError> {
        let name = name.into();
        if !is_input_name(&name) {
            return Err(TransformDocumentError::InvalidInputName(name));
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InputName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransformHash(BlobHash);

impl TransformHash {
    pub fn as_blob_hash(&self) -> BlobHash {
        self.0
    }
}

impl fmt::Display for TransformHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformDocument {
    flake_source: BlobHash,
    attribute: String,
    inputs: BTreeMap<InputName, BlobHash>,
}

impl TransformDocument {
    pub fn nix(
        flake_source: BlobHash,
        attribute: impl Into<String>,
        inputs: BTreeMap<InputName, BlobHash>,
    ) -> Result<Self, TransformDocumentError> {
        let attribute = attribute.into();
        if attribute.trim().is_empty() || attribute.contains('\0') {
            return Err(TransformDocumentError::InvalidAttribute(attribute));
        }
        Ok(Self {
            flake_source,
            attribute,
            inputs,
        })
    }

    pub fn version(&self) -> u16 {
        FORMAT_VERSION
    }

    pub fn executor(&self) -> TransformExecutor {
        TransformExecutor::Nix
    }

    pub fn flake_source(&self) -> BlobHash {
        self.flake_source
    }

    pub fn attribute(&self) -> &str {
        &self.attribute
    }

    pub fn inputs(&self) -> &BTreeMap<InputName, BlobHash> {
        &self.inputs
    }

    pub fn output(&self) -> TransformOutput {
        TransformOutput::AssetManifest
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(TRANSFORM_HEADER);
        write_u16(&mut bytes, self.version());
        bytes.push(NIX_EXECUTOR_TAG);
        write_hash(&mut bytes, self.flake_source);
        write_text(&mut bytes, &self.attribute);
        write_u32(&mut bytes, self.inputs.len() as u32);
        for (name, hash) in &self.inputs {
            write_text(&mut bytes, name.as_str());
            write_hash(&mut bytes, *hash);
        }
        bytes.push(ASSET_MANIFEST_OUTPUT_TAG);
        bytes
    }

    pub fn hash(&self) -> TransformHash {
        TransformHash(BlobHash::of(&self.canonical_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformDocumentError {
    InvalidAttribute(String),
    InvalidInputName(String),
}

impl fmt::Display for TransformDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAttribute(attribute) => {
                write!(f, "{attribute:?} is not a valid Nix output attribute")
            }
            Self::InvalidInputName(name) => {
                write!(f, "{name:?} is not a valid transform input name")
            }
        }
    }
}

impl std::error::Error for TransformDocumentError {}

fn is_input_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

fn write_hash(bytes: &mut Vec<u8>, hash: BlobHash) {
    bytes.extend_from_slice(hash.as_bytes());
}

fn write_text(bytes: &mut Vec<u8>, text: &str) {
    write_u32(bytes, text.len() as u32);
    bytes.extend_from_slice(text.as_bytes());
}

fn write_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn write_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(bytes: &[u8]) -> BlobHash {
        BlobHash::of(bytes)
    }

    fn inputs(entries: &[(&str, &[u8])]) -> BTreeMap<InputName, BlobHash> {
        entries
            .iter()
            .map(|(name, bytes)| (InputName::new(*name).unwrap(), hash(bytes)))
            .collect()
    }

    #[test]
    fn nix_document_hashes_its_pinned_fields() {
        let document = TransformDocument::nix(
            hash(b"flake source"),
            "packages.x86_64-linux.unpack-zip",
            inputs(&[("archive", b"cards.zip")]),
        )
        .unwrap();

        assert_eq!(document.version(), FORMAT_VERSION);
        assert_eq!(document.executor(), TransformExecutor::Nix);
        assert_eq!(document.output(), TransformOutput::AssetManifest);
        assert_eq!(document.hash(), document.hash());
        assert_ne!(
            document.hash(),
            TransformDocument::nix(
                hash(b"other flake source"),
                "packages.x86_64-linux.unpack-zip",
                inputs(&[("archive", b"cards.zip")]),
            )
            .unwrap()
            .hash()
        );
        assert_ne!(
            document.hash(),
            TransformDocument::nix(
                hash(b"flake source"),
                "packages.x86_64-linux.another-output",
                inputs(&[("archive", b"cards.zip")]),
            )
            .unwrap()
            .hash()
        );
        assert_ne!(
            document.hash(),
            TransformDocument::nix(
                hash(b"flake source"),
                "packages.x86_64-linux.unpack-zip",
                inputs(&[("archive", b"other.zip")]),
            )
            .unwrap()
            .hash()
        );
    }

    #[test]
    fn input_order_does_not_change_the_document_hash() {
        let source = hash(b"flake source");
        let first = TransformDocument::nix(
            source,
            "packages.x86_64-linux.unpack-zip",
            inputs(&[("archive", b"cards.zip"), ("settings", b"settings.json")]),
        )
        .unwrap();
        let second = TransformDocument::nix(
            source,
            "packages.x86_64-linux.unpack-zip",
            inputs(&[("settings", b"settings.json"), ("archive", b"cards.zip")]),
        )
        .unwrap();

        assert_eq!(first.canonical_bytes(), second.canonical_bytes());
        assert_eq!(first.hash(), second.hash());
    }

    #[test]
    fn document_rejects_invalid_input_names_and_attributes() {
        for name in ["", "Archive", "archive/path", "archive space", "1archive"] {
            assert!(InputName::new(name).is_err(), "{name:?}");
        }
        assert!(InputName::new("archive-1_name").is_ok());
        assert!(TransformDocument::nix(hash(b"flake"), "", BTreeMap::new()).is_err());
        assert!(TransformDocument::nix(hash(b"flake"), "  ", BTreeMap::new()).is_err());
        assert!(TransformDocument::nix(hash(b"flake"), "a\0b", BTreeMap::new()).is_err());
    }
}
