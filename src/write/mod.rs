//! ERA file writer.
//!
//! Provides [`E2StoreWriter`] for low-level entry writing and [`EraBuilder`]
//! for constructing valid ERA files with correct index offsets.

use std::collections::HashMap;
use std::io::{Result, Write};

use crate::format::Entry;
use crate::format::era::SlotIndex;
use crate::format::types::*;

/// Writes e2store entries to any `Write` sink.
///
/// Each entry is written as its 8-byte header (via [`Header::encode`])
/// followed by the entry data payload.
pub struct E2StoreWriter<W: Write> {
    writer: W,
}

impl<W: Write> E2StoreWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    /// Write a single entry.
    ///
    /// Uses `Header::encode()` which writes the full 8-byte header
    /// (type[2] + length[4] + reserved[2]), then the data.
    pub fn write_entry(&mut self, entry: &Entry) -> Result<()> {
        self.writer.write_all(&entry.header.encode())?;
        self.writer.write_all(&entry.data)?;
        Ok(())
    }

    /// Write multiple entries in sequence.
    pub fn write_all(&mut self, entries: &[Entry]) -> Result<()> {
        for entry in entries {
            self.write_entry(entry)?;
        }
        Ok(())
    }

    /// Unwrap the inner writer
    pub fn into_inner(self) -> W {
        self.writer
    }
}

/// Builds a valid ERA file with correct block and state index offsets
///
/// Index offsets are relative to the index entry's position in the file,
/// so the builder must calculate absolute byte positions for all entries
/// before it can compute the offsets. It does this in two passes:
///
/// 1. **Position calculation** - walk the planned layout to determine
///    where each entry will land.
/// 2. **Write** - emit entries with correctly-computed index payloads.
///
/// # File layout
///
/// ```text
/// [Version]
/// [CompressedSignedBeaconBlock]...   one per block
/// [CompressedBeaconState]            optional, exactly one
/// [BlockIndex]
/// [StateIndex]                       present only if state is set
/// ```
///
/// # Example
///
/// ```ignore
/// use brandon::write::EraBuilder;
/// use snap::write::FrameEconder;
/// use std::io::Write;
///
/// let compressed = {
///     let mut enc = FrameEncoder::new(Vec::new());
///     enc.write_all(&ssz_bytes).unwrap();
///     enc.into_inner().unwrap();
/// };
///
/// let mut builder = EraBuilder::new();
/// builder.add_block(0, compressed.clone());
/// builder.set_state(8192, compressed); // boundary state slot
///
/// let mut output = Vec::new();
/// builder.build(&mut output).unwrap();
/// ```
pub struct EraBuilder {
    blocks: Vec<(u64, Vec<u8>)>,
    state: Option<(u64, Vec<u8>)>,
}

impl Default for EraBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EraBuilder {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            state: None,
        }
    }

    /// Add a compressed beacon block at the given slot.
    ///
    /// # Panics
    ///
    /// Panics if a block has already been added for this slot.
    pub fn add_block(&mut self, slot: u64, compressed_block: Vec<u8>) {
        assert!(
            !self.blocks.iter().any(|(s, _)| *s == slot),
            "duplicate slot {slot}"
        );
        self.blocks.push((slot, compressed_block));
    }

    /// Set the compressed beacon state and its slot.
    pub fn set_state(&mut self, slot: u64, compressed_state: Vec<u8>) {
        self.state = Some((slot, compressed_state));
    }

    /// Build the ERA file
    pub fn build<W: Write>(&self, output: &mut W) -> std::io::Result<()> {
        let mut writer = E2StoreWriter::new(output);

        // Index offsets are relative to the index entry's file position,
        // so we must know where every entry lands before computing them.

        let mut pos: u64 = 0;

        // Version: 8-byte header, 0 bytes data
        pos += 8;

        // Block entries: 8-byte header + data each
        let mut block_positions: Vec<(u64, u64)> = Vec::with_capacity(self.blocks.len());
        for (slot, block_data) in &self.blocks {
            block_positions.push((*slot, pos));
            pos += 8 + block_data.len() as u64;
        }

        // State entry (optional): 8-byte header + data
        let state_postion: Option<u64> = if let Some((_, state_data)) = &self.state {
            let sp = pos;
            pos += 8 + state_data.len() as u64;
            Some(sp)
        } else {
            None
        };

        let block_index_pos = pos;

        let block_index = if self.blocks.is_empty() {
            SlotIndex::new(0, vec![])
        } else {
            let min_slot = self.blocks.iter().map(|(s, _)| *s).min().unwrap();
            let max_slot = self.blocks.iter().map(|(s, _)| *s).max().unwrap();
            let slot_count = (max_slot - min_slot + 1) as usize;

            let mut slot_to_pos: HashMap<u64, u64> = HashMap::with_capacity(self.blocks.len());
            for (slot, abs_pos) in &block_positions {
                slot_to_pos.insert(*slot, *abs_pos);
            }

            let mut offsets = Vec::with_capacity(slot_count);
            for slot in min_slot..=max_slot {
                if let Some(&abs_pos) = slot_to_pos.get(&slot) {
                    // Negative for entries written before the index
                    offsets.push(abs_pos as i64 - block_index_pos as i64);
                } else {
                    offsets.push(0); // skipped slot
                }
            }

            SlotIndex::new(min_slot, offsets)
        };

        // Advance past the block index entry itself
        pos = block_index_pos + 8 + block_index.encode().len() as u64;

        // Build state index
        let state_index: Option<SlotIndex> = if let Some((state_slot, _)) = &self.state {
            let state_index_pos = pos;
            let state_abs_pos = state_postion.unwrap();
            let offset = state_abs_pos as i64 - state_index_pos as i64;
            Some(SlotIndex::new(*state_slot, vec![offset]))
        } else {
            None
        };

        writer.write_entry(&Entry::version())?;

        for (_, block_data) in &self.blocks {
            writer.write_entry(&Entry::new(
                TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
                block_data.clone(),
            ))?;
        }

        if let Some((_, state_data)) = &self.state {
            writer.write_entry(&Entry::new(
                TYPE_COMPRESSED_BEACON_STATE,
                state_data.clone(),
            ))?;
        }

        writer.write_entry(&Entry::new(TYPE_SLOT_INDEX, block_index.encode()))?;

        if let Some(state_idx) = &state_index {
            writer.write_entry(&Entry::new(TYPE_SLOT_INDEX, state_idx.encode()))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::e2store::{E2StoreReader, Header};
    use crate::read::{EraBlockReader, EraRandomReader, EraReader, TypedEntry};
    use snap::write::FrameEncoder;
    use std::io::{Cursor, Write as IoWrite};

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.into_inner().unwrap()
    }

    #[test]
    fn writer_header_is_exactly_8_bytes() {
        let mut buf = Vec::new();
        let mut writer = E2StoreWriter::new(&mut buf);
        let entry = Entry::new([0x01, 0x00], vec![0xAA, 0xBB]);
        writer.write_entry(&entry).unwrap();

        assert_eq!(buf.len(), 10); // 8 header + 2 data
    }

    #[test]
    fn writer_roundtrip_with_reader() {
        let mut buf = Vec::new();
        let mut writer = E2StoreWriter::new(&mut buf);

        let entries = vec![
            Entry::version(),
            Entry::new([0x01, 0x00], vec![1, 2, 3]),
            Entry::new([0x02, 0x00], vec![4, 5, 6, 7]),
        ];
        writer.write_all(&entries).unwrap();

        let reader = E2StoreReader::new(Cursor::new(&buf));
        let read_back = reader.read_all().unwrap();

        assert_eq!(read_back.len(), 3);
        assert!(read_back[0].header.is_version());
        assert_eq!(read_back[1].data, vec![1, 2, 3]);
        assert_eq!(read_back[2].data, vec![4, 5, 6, 7]);
    }

    #[test]
    fn writer_version_entry_is_valid() {
        let mut buf = Vec::new();
        let mut writer = E2StoreWriter::new(&mut buf);
        writer.write_entry(&Entry::version()).unwrap();

        assert_eq!(buf.len(), 8);
        let header = Header::decode(&buf).unwrap();
        assert!(header.is_version());
        assert_eq!(header.length, 0);
        assert_eq!(header.reserved, 0);
    }

    #[test]
    fn writer_into_inner() {
        let writer = E2StoreWriter::new(Vec::<u8>::new());
        let _recovered: Vec<u8> = writer.into_inner();
    }

    #[test]
    fn builder_empty_file() {
        let mut buf = Vec::new();
        EraBuilder::new().build(&mut buf).unwrap();

        let reader = E2StoreReader::new(Cursor::new(&buf));
        let entries = reader.read_all().unwrap();
        assert_eq!(entries.len(), 2); // version + empty block index
        assert!(entries[0].header.is_version());
        assert_eq!(entries[1].header.typ, TYPE_SLOT_INDEX);
    }

    #[test]
    fn builder_blocks_only() {
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0xAA; 100]));
        builder.add_block(2, compress(&[0xBB; 200]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        let mut reader = EraBlockReader::new(Cursor::new(&buf));
        let b1 = reader.next_block().unwrap().unwrap();
        let b2 = reader.next_block().unwrap().unwrap();
        assert!(reader.next_block().unwrap().is_none());

        assert_eq!(b1.primary_data(), &[0xAA; 100]);
        assert_eq!(b2.primary_data(), &[0xBB; 200]);
    }

    #[test]
    fn builder_with_state() {
        let mut builder = EraBuilder::new();
        builder.add_block(100, compress(&[0x11; 50]));
        builder.set_state(8192, compress(&[0x22; 300]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        let entries = EraReader::new(Cursor::new(&buf)).read_all().unwrap();

        assert!(
            entries
                .iter()
                .any(|e| matches!(e, TypedEntry::BeaconBlock { .. }))
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, TypedEntry::BeaconState { .. }))
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, TypedEntry::BlockIndex { .. }))
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, TypedEntry::StateIndex { .. }))
        );
    }

    #[test]
    #[should_panic(expected = "duplicate slot 0")]
    fn builder_rejects_duplicate_slots() {
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0x01]));
        builder.add_block(0, compress(&[0x02]));
    }

    #[test]
    fn builder_offsets_are_correct() {
        // Blocks at slots 0 and 2 (slot 1 skipped), state at slot 3
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0x00; 64]));
        builder.add_block(2, compress(&[0x02; 64]));
        builder.set_state(3, compress(&[0xFF; 128]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        // Random reader validates offsets by seeking to them
        let mut rr = EraRandomReader::new(Cursor::new(&buf)).unwrap();
        assert_eq!(rr.starting_slot(), Some(0));
        assert_eq!(rr.slot_count(), Some(3)); // slots 0, 1, 2

        let b = rr.read_block_at_slot(0).unwrap().unwrap();
        assert_eq!(b.primary_data(), &[0x00; 64]);

        assert!(rr.read_block_at_slot(1).unwrap().is_none()); // skipped

        let b = rr.read_block_at_slot(2).unwrap().unwrap();
        assert_eq!(b.primary_data(), &[0x02; 64]);

        let state = rr.read_state().unwrap().unwrap();
        assert_eq!(state, &[0xFF; 128]);
    }

    #[test]
    fn builder_block_index_starting_slot_is_minimum() {
        let mut builder = EraBuilder::new();
        builder.add_block(1000, compress(&[0x01]));
        builder.add_block(1005, compress(&[0x02]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        let rr = EraRandomReader::new(Cursor::new(&buf)).unwrap();
        assert_eq!(rr.starting_slot(), Some(1000));
        assert_eq!(rr.slot_count(), Some(6)); // 1000..=1005
    }

    #[test]
    fn builder_state_index_has_correct_slot() {
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0x01]));
        builder.set_state(99999, compress(&[0x02]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        let entries = E2StoreReader::new(Cursor::new(&buf)).read_all().unwrap();

        let last = entries.last().unwrap();
        assert_eq!(last.header.typ, TYPE_SLOT_INDEX);

        let idx = SlotIndex::decode(&last.data).unwrap();
        assert_eq!(idx.starting_slot, 99999);
        assert_eq!(idx.offsets.len(), 1);
        assert_ne!(idx.offsets[0], 0);
    }

    #[test]
    fn builder_state_index_offset_points_to_state_entry() {
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0x01; 32]));
        builder.set_state(1, compress(&[0x02; 32]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        // Find the state index and verify its offset resolves correctly
        let entries = E2StoreReader::new(Cursor::new(&buf)).read_all().unwrap();

        let state_idx_entry = entries
            .iter()
            .find(|e| e.header.typ == TYPE_SLOT_INDEX)
            .unwrap();
        let state_idx = SlotIndex::decode(&state_idx_entry.data).unwrap();

        // Find the state entry's absolute position
        let state_abs: u64 = entries
            .iter()
            .enumerate()
            .find(|(_, e)| e.header.typ == TYPE_COMPRESSED_BEACON_STATE)
            .map(|(i, _)| {
                entries[..i]
                    .iter()
                    .map(|e| 8u64 + e.data.len() as u64)
                    .sum()
            })
            .unwrap();

        // Find the state index entry's absolute position
        let state_idx_abs: u64 = entries
            .iter()
            .enumerate()
            .rfind(|(_, e)| e.header.typ == TYPE_SLOT_INDEX)
            .map(|(i, _)| {
                entries[..i]
                    .iter()
                    .map(|e| 8u64 + e.data.len() as u64)
                    .sum()
            })
            .unwrap();

        let resolved = state_idx_abs as i64 + state_idx.offsets[0];
        assert_eq!(resolved as u64, state_abs);
    }

    #[test]
    fn builder_passes_verify() {
        let mut builder = EraBuilder::new();
        builder.add_block(0, compress(&[0xAA; 50]));
        builder.add_block(1, compress(&[0xBB; 50]));
        builder.set_state(2, compress(&[0xCC; 100]));

        let mut buf = Vec::new();
        builder.build(&mut buf).unwrap();

        let result = crate::verify::verify_era(&buf[..]);
        if !result.valid {
            panic!("verification failed:\n{}", result.errors.join("\n"));
        }
        assert_eq!(result.block_count, 2);
        assert!(result.state_present);
    }

    #[test]
    fn builder_default_equals_new() {
        let mut buf_a = Vec::new();
        let mut buf_b = Vec::new();
        EraBuilder::new().build(&mut buf_a).unwrap();
        EraBuilder::default().build(&mut buf_b).unwrap();
        assert_eq!(buf_a, buf_b);
    }
}
