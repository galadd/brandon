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

/// Slot index — maps block numbers to byte offsets within the ERA1 file.
///
/// Layout:
///   starting_slot: u64 LE
///   offsets:       [i64 LE; count]
///   count:         u64 LE
///
/// `offsets` contains one entry per slot in the archive range.
/// A value of 0 means no block exists for that slot.
/// Each offset is relative to the block index entry location.
#[derive(Debug, Clone)]
pub struct SlotIndex {
    pub starting_slot: u64,
    pub offsets: Vec<i64>,
    pub count: u64,
}

impl SlotIndex {
    pub fn new(starting_slot: u64, offsets: Vec<i64>) -> Self {
        let count = offsets.iter().filter(|&&o| o != 0).count() as u64;

        Self {
            starting_slot,
            offsets,
            count,
        }
    }

    pub fn with_count(starting_slot: u64, offsets: Vec<i64>, count: u64) -> Self {
        Self {
            starting_slot,
            offsets,
            count,
        }
    }

    /// Decode a SlotIndex from its e2store entry data.
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        // Need:
        // 8 bytes starting_slot
        // N * 8 bytes offsets
        // 8 bytes count
        if data.len() < 16 || !(data.len() - 16).is_multiple_of(8) {
            return Err(format!("invalid slot index data length: {}", data.len()));
        }

        let starting_slot = u64::from_le_bytes(
            data[0..8]
                .try_into()
                .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
        );

        let count_pos = data.len() - 8;

        let offsets = data[8..count_pos]
            .chunks_exact(8)
            .map(|chunk| i64::from_le_bytes(chunk.try_into().unwrap()))
            .collect();

        let count = u64::from_le_bytes(
            data[count_pos..]
                .try_into()
                .map_err(|e: std::array::TryFromSliceError| e.to_string())?,
        );

        Ok(Self {
            starting_slot,
            offsets,
            count,
        })
    }

    /// Encode to bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + (self.offsets.len() * 8) + 8);
        buf.extend_from_slice(&self.starting_slot.to_le_bytes());
        for off in &self.offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        buf.extend_from_slice(&self.count.to_le_bytes());
        buf
    }
}

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
        assert_eq!(decoded.count, 3);
    }

    #[test]
    fn slot_index_rejects_bad_length() {
        assert!(SlotIndex::decode(&[0; 7]).is_err()); // too short
        assert!(SlotIndex::decode(&[0; 13]).is_err()); // not aligned
    }
}
