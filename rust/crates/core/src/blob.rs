use std::fmt;
use std::str::FromStr;

const BLOB_HASH_BYTE_LENGTH: usize = blake3::OUT_LEN;
const HEX_DIGITS_PER_BYTE: usize = 2;
const BLOB_HASH_HEX_LENGTH: usize = BLOB_HASH_BYTE_LENGTH * HEX_DIGITS_PER_BYTE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobHash([u8; BLOB_HASH_BYTE_LENGTH]);

impl BlobHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; BLOB_HASH_BYTE_LENGTH] {
        &self.0
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHashError(String);

impl fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} is not a blob hash ({BLOB_HASH_HEX_LENGTH} hex characters)",
            self.0
        )
    }
}

impl std::error::Error for ParseHashError {}

impl FromStr for BlobHash {
    type Err = ParseHashError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex = text.trim();
        let invalid_hash = || ParseHashError(text.to_string());
        if hex.len() != BLOB_HASH_HEX_LENGTH || !hex.is_ascii() {
            return Err(invalid_hash());
        }

        let mut bytes = [0; BLOB_HASH_BYTE_LENGTH];
        hex.as_bytes()
            .chunks_exact(HEX_DIGITS_PER_BYTE)
            .zip(&mut bytes)
            .try_for_each(|(digits, byte)| {
                let digits = std::str::from_utf8(digits).map_err(|_| invalid_hash())?;
                *byte = u8::from_str_radix(digits, 16).map_err(|_| invalid_hash())?;
                Ok(())
            })?;

        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_displays_and_parses_as_hex() {
        let hash = BlobHash::of(b"smaug");
        let hex = hash.to_string();
        assert_eq!(hex.len(), BLOB_HASH_HEX_LENGTH);
        assert_eq!(hex.parse::<BlobHash>(), Ok(hash));
        assert!("zz".parse::<BlobHash>().is_err());
        assert!(hex[..10].parse::<BlobHash>().is_err());
        assert!(format!("{}zz", &hex[..62]).parse::<BlobHash>().is_err());
    }
}
