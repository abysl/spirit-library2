use anyhow::{ensure, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::EndpointId;
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum MeshIdKind {
    Random([u8; 32]),
    Legacy(EndpointId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MeshId(MeshIdKind);

impl MeshId {
    pub(crate) fn generate() -> Result<Self> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("could not generate mesh ID: {error}"))?;
        Ok(Self(MeshIdKind::Random(bytes)))
    }

    pub(crate) fn legacy(id: EndpointId) -> Self {
        Self(MeshIdKind::Legacy(id))
    }

    pub(crate) fn legacy_node(self) -> Option<EndpointId> {
        match self.0 {
            MeshIdKind::Legacy(id) => Some(id),
            MeshIdKind::Random(_) => None,
        }
    }

    pub(crate) fn random_bytes(self) -> Option<[u8; 32]> {
        match self.0 {
            MeshIdKind::Random(bytes) => Some(bytes),
            MeshIdKind::Legacy(_) => None,
        }
    }
}

impl fmt::Display for MeshId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            MeshIdKind::Random(bytes) => {
                write!(formatter, "mesh1_{}", URL_SAFE_NO_PAD.encode(bytes))
            }
            MeshIdKind::Legacy(id) => id.fmt(formatter),
        }
    }
}

impl FromStr for MeshId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        if let Some(encoded) = value.strip_prefix("mesh1_") {
            let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
            ensure!(
                URL_SAFE_NO_PAD.encode(&bytes) == encoded,
                "noncanonical mesh ID"
            );
            let bytes = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid mesh ID length"))?;
            Ok(Self(MeshIdKind::Random(bytes)))
        } else {
            Ok(Self::legacy(value.parse()?))
        }
    }
}

impl Serialize for MeshId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MeshId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_ids_have_a_separate_canonical_encoding() {
        let id = MeshId::generate().unwrap();
        let encoded = id.to_string();
        assert!(encoded.starts_with("mesh1_"));
        assert_eq!(encoded.parse::<MeshId>().unwrap(), id);
        assert_eq!(
            serde_json::from_str::<MeshId>(&serde_json::to_string(&id).unwrap()).unwrap(),
            id
        );
        assert!("mesh1_".parse::<MeshId>().is_err());
        assert!("mesh1_abc".parse::<MeshId>().is_err());
        assert!(format!("{encoded}=").parse::<MeshId>().is_err());
        assert!("not-a-mesh-id".parse::<MeshId>().is_err());
    }
}
