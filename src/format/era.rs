//! ERA file format — post-merge beacon chain history.
//!
//! Spec: <https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md>
//!
//! An ERA file contains one era of `SLOTS_PER_HISTORICAL_ROOT` (8192) slots:
//! - Version entry
//! - Multiple CompressedSignedBeaconBlock entries
//! - One CompressedBeaconState entry (boundary state at era end)
//! - Slot index entries for random access

use crate::error::Error;

/// CompressedSignedBeaconBlock entry type.
pub const TYPE_COMPRESSED_SIGNED_BEACON_BLOCK: [u8; 2] = [0x01, 0x00];

/// CompressedBeaconState entry type.
pub const TYPE_COMPRESSED_BEACON_STATE: [u8; 2] = [0x02, 0x00];

/// Block slot index.
pub const TYPE_BLOCK_INDEX: [u8; 2] = [0x66, 0x32];

/// State slot index.
pub const TYPE_STATE_INDEX: [u8; 2] = [0x33, 0x00];

/// Number of slots per era (= SLOTS_PER_HISTORICAL_ROOT).
pub const SLOTS_PER_ERA: u64 = 8192;

/// Slot index — maps slot numbers to byte offsets within the ERA file.
///
/// Layout:
///   starting_slot: u64 LE
///   offsets: [i64 LE; N]  (0 = no block at that slot, relative to index start)
#[derive(Debug, Clone)]
pub struct SlotIndex {
    pub starting_slot: u64,
    pub offsets: Vec<i64>,
}

impl SlotIndex {
    pub fn new(starting_slot: u64, offsets: Vec<i64>) -> Self {
        Self {
            starting_slot,
            offsets,
        }
    }

    /// Decode a SlotIndex from its e2store entry data.
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() < 8 || (data.len() - 8) % 8 != 0 {
            return Err(format!("invalid slot index data length: {}", data.len()));
        }
        let starting_slot = u64::from_le_bytes(
            data[0..8]
                .try_into()
                .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
        );
        let offsets = data[8..]
            .chunks_exact(8)
            .map(|chunk| i64::from_le_bytes(chunk.try_into().unwrap()))
            .collect();
        Ok(SlotIndex {
            starting_slot,
            offsets,
        })
    }

    /// Encode to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.offsets.len() * 8);
        buf.extend_from_slice(&self.starting_slot.to_le_bytes());
        for off in &self.offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        buf
    }
}

// #[cfg(feature = "read")]
pub fn decompress_entry(data: &[u8]) -> Result<Vec<u8>, crate::error::Error> {
    use snap::read::FrameDecoder;
    use std::io::Read;

    let mut decoder = FrameDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_index_roundtrip() {
        let idx = SlotIndex::new(0, vec![0, 100, 200, 0, 300]);
        let encoded = idx.encode();
        let decoded = SlotIndex::decode(&encoded).unwrap();
        assert_eq!(decoded.starting_slot, 0);
        assert_eq!(decoded.offsets, vec![0, 100, 200, 0, 300]);
    }

    #[test]
    fn slot_index_rejects_bad_length() {
        assert!(SlotIndex::decode(&[0; 7]).is_err()); // too short
        assert!(SlotIndex::decode(&[0; 13]).is_err()); // not aligned
    }
}
