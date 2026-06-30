//! ERA1 file format — pre-merge execution layer history.
//!
//! Spec: <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md>
//!
//! An ERA1 file contains one era of execution blocks (up to 8192).

/// Maximum number of blocks per ERA1 file.
pub const MAX_ERA1_BLOCKS: usize = 8192;

/// 32-byte hash.
pub type B256 = [u8; 32];

/// 256-bit unsigned integer (little-endian bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct U256(pub [u8; 32]);

impl U256 {
    pub fn zero() -> Self {
        Self([0u8; 32])
    }
}

/// SSZ `header-record := { block-hash: Bytes32, total-difficulty: Uint256 }`
///
/// The accumulator root is `hash_tree_root(List[HeaderRecord, 8192])`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderRecord {
    pub block_hash: B256,
    pub total_difficulty: U256,
}

impl HeaderRecord {
    /// SSZ-encoded size: 32 + 32 = 64 bytes.
    pub const SSZ_SIZE: usize = 64;

    pub fn decode_ssz(data: &[u8]) -> Result<Self, String> {
        if data.len() < Self::SSZ_SIZE {
            return Err(format!(
                "header record too short: {} < {}",
                data.len(),
                Self::SSZ_SIZE
            ));
        }
        let block_hash: B256 = data[0..32].try_into().unwrap();
        let total_difficulty = U256(data[32..64].try_into().unwrap());
        Ok(HeaderRecord {
            block_hash,
            total_difficulty,
        })
    }

    pub fn encode_ssz(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::SSZ_SIZE);
        buf.extend_from_slice(&self.block_hash);
        buf.extend_from_slice(&self.total_difficulty.0);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_record_roundtrip() {
        let mut hash = [0u8; 32];
        hash[0] = 0xab;
        let mut td = [0u8; 32];
        td[0] = 0x01;
        let record = HeaderRecord {
            block_hash: hash,
            total_difficulty: U256(td),
        };
        let encoded = record.encode_ssz();
        assert_eq!(encoded.len(), 64);
        let decoded = HeaderRecord::decode_ssz(&encoded).unwrap();
        assert_eq!(record, decoded);
    }

    #[test]
    fn header_record_rejects_short() {
        assert!(HeaderRecord::decode_ssz(&[0; 63]).is_err());
    }
}
