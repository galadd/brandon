//! Structual transformations and extraction utilities for ERA
//!
//! This module provides structural transformations that operate at the
//! e2store container level without modifying or parsing block payloads.
//!
//! # Available Transformations
//!
//! | Command | Description |
//! |---|---|
//! | [`reindex`] | Rebuild indexes from scratch (fast, no decompression) |
//! | [`strip`]   | Remove optional entries (receipts, state) |
//! | [`split`]   | Extract blocks to individual `.snappy` files |
//!
//! # ERA1 to ERA Conversion
//!
//! A true ERA1->ERA conversion requires wrapping execution blocks in SSZ
//! `SignedBeaconBlock` structures. Because SSZ schemas change across
//! forks (Bellatrix, Capella, Deneb, etc.), Brandon delegates this to
//! your consensus library of choice.
//!
//! Use [`era1_to~era`] to handle all e2store indexing, compression, and
//! file layout while you provide a closure that returns the SSZ bytes.
//!
//! ## Example
//!
//! ```ignore
//! use brandon::convert::era1_to_era;
//! use brandon::read::Era1Block;
//!
//! let input = std::fs::File::open("mainnet-00000-5ec1ffb8.era1")?;
//! let output = std::fs::File::create("synthetic.era")?;
//!
//! // The synthesizer receives a reusable buffer to write SSZ bytes into.
//! // This avoids per-block allocation.
//! era1_to_era(input, output, |era1_block: &Era1Block, ssz_buf: &mut Vec<u8>| {
//!     ssz_buf.clear();
//!     // Use your SSZ library to wrap era1_block.header into a SignedBeaconBlock
//!     my_consensus_lib::synthesize_into(era1_block, ssz_buf)?;
//!     Ok(())
//! })?;
//! ```

pub mod reindex;
pub mod split;
pub mod strip;

use std::io::{Read, Seek, Write};

use crate::{Era1Block, EraBlockReader, EraRandomReader, error::Error, write::EraBuilder};

/// Converts a stream of ERA1 blocks into an ERA file using a custom synthesizer.
///
/// This function handles all e2store container logic: reading ERA1 groups,
/// calling your synthesizer, Snappy-compressing the result, calculating
/// exact byte offsets, and writing a valid ERA file with correct indexes.
///
/// The `synthesizer` closure receives the block and a reusable buffer.
/// It should write the raw SSZ bytes for a `SignedBeaconBlock` into `ssz_buf`
/// and clear the buffer before writing.
pub fn era1_to_era<R, W, F, E>(reader: R, writer: W, mut synthesizer: F) -> Result<(), E>
where
    R: Read + Seek,
    W: Write,
    F: FnMut(&Era1Block, &mut Vec<u8>) -> Result<(), E>,
    E: From<Error>,
{
    let rr = EraRandomReader::new(reader).map_err(E::from)?;
    let block_slots: Vec<u64> = rr
        .block_index()
        .map(|idx| {
            idx.offsets
                .iter()
                .enumerate()
                .filter_map(|(i, &off)| {
                    if off != 0 {
                        Some(idx.starting_slot + i as u64)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Recover the reader and rewind for the streaming pass
    let mut reader = rr.into_inner();
    reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|e| E::from(Error::Io(e)))?;

    let mut ssz_buf = Vec::with_capacity(256 * 1024);
    let mut compressed_buf = Vec::with_capacity(256 * 1024);
    let mut builder = EraBuilder::new();
    let mut slot_idx = 0usize;

    let mut block_reader = EraBlockReader::new(&mut reader);

    while let Some(block) = block_reader.next_block().map_err(E::from)? {
        if let crate::Block::Era1(era1) = block {
            let slot = block_slots
                .get(slot_idx)
                .copied()
                .unwrap_or(slot_idx as u64);

            ssz_buf.clear();
            synthesizer(&era1, &mut ssz_buf)?;

            compressed_buf.clear();
            compress_into(&ssz_buf, &mut compressed_buf).map_err(E::from)?;
            builder.add_block(slot, std::mem::take(&mut compressed_buf));

            slot_idx += 1;
        } else {
            return Err(E::from(Error::E2Store(
                crate::error::E2StoreError::InvalidEra("expected ERA1 blocks in input".into()),
            )));
        }
    }

    builder
        .build(&mut { writer })
        .map_err(|e| E::from(Error::Io(e)))?;

    Ok(())
}

/// Snappy-compress data into a provided buffer.
fn compress_into(data: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    out.clear();
    let mut enc = snap::write::FrameEncoder::new(out);
    enc.write_all(data)
        .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    enc.flush()
        .map_err(|e| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::era::{SlotIndex, TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, TYPE_SLOT_INDEX};
    use crate::format::era1::*;
    use crate::verify::verify_era;
    use crate::{format::Entry, write::E2StoreWriter};
    use snap::write::FrameEncoder;
    use std::io::{Cursor, Write};

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.into_inner().unwrap()
    }

    /// Build a minimal valid ERA1 file covering slots 100..102 (slot 101 skipped).
    fn build_test_era1() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();

        // Slot 100
        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x64])))
            .unwrap();
        w.write_entry(&Entry::new(TYPE_BLOCK_BODY, vec![0x01]))
            .unwrap();
        let mut td = [0u8; 32];
        td[31] = 100;
        w.write_entry(&Entry::new(TYPE_TOTAL_DIFFICULTY, td.to_vec()))
            .unwrap();

        // Slot 102 (101 skipped)
        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x66])))
            .unwrap();
        w.write_entry(&Entry::new(TYPE_BLOCK_BODY, vec![0x02]))
            .unwrap();
        let mut td2 = [0u8; 32];
        td2[31] = 102;
        w.write_entry(&Entry::new(TYPE_TOTAL_DIFFICULTY, td2.to_vec()))
            .unwrap();

        // Block index: slots 100, 101, 102
        let idx = SlotIndex::with_count(100, vec![-8i64, 0i64, -8i64], 2);
        w.write_entry(&Entry::new(TYPE_SLOT_INDEX, idx.encode()))
            .unwrap();

        buf
    }

    #[test]
    fn era1_to_era_preserves_slot_range() {
        let input = build_test_era1();
        let mut output = Vec::new();

        era1_to_era(
            Cursor::new(input),
            &mut output,
            |_era1_block: &Era1Block, ssz_buf: &mut Vec<u8>| {
                // Dummy synthesizer: just write a fixed SSZ-like payload
                ssz_buf.extend_from_slice(&[0x00, 0x01, 0x02, 0x03]);
                Ok::<(), Error>(())
            },
        )
        .unwrap();

        // Verify the output is a valid ERA file
        let result = verify_era(&output[..]);
        if !result.valid {
            panic!("output failed verification:\n{}", result.errors.join("\n"));
        }
        assert_eq!(result.format.as_deref(), Some("ERA"));
        assert_eq!(result.block_count, 2);

        // Verify slot range is preserved (100..102, not 0..2)
        let mut rr = EraRandomReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(rr.starting_slot(), Some(100));
        assert_eq!(rr.slot_count(), Some(3)); // slots 100, 101, 102

        // Slot 100 has a block
        assert!(rr.read_block_at_slot(100).unwrap().is_some());
        // Slot 101 is skipped
        assert!(rr.read_block_at_slot(101).unwrap().is_none());
        // Slot 102 has a block
        assert!(rr.read_block_at_slot(102).unwrap().is_some());
    }

    #[test]
    fn era1_to_era_synthesizer_receives_correct_blocks() {
        let input = build_test_era1();
        let mut output = Vec::new();
        let mut seen_td: Vec<u8> = Vec::new();

        era1_to_era(
            Cursor::new(input),
            &mut output,
            |era1_block: &Era1Block, ssz_buf: &mut Vec<u8>| -> Result<(), Error> {
                // Capture the TD value from each block to verify correct ordering
                seen_td.push(era1_block.total_difficulty.0[31]);
                ssz_buf.extend_from_slice(b"dummy");
                Ok(())
            },
        )
        .unwrap();

        // Should have seen TD values 100 and 102 (in order)
        assert_eq!(seen_td, vec![100, 102]);
    }

    #[test]
    fn era1_to_era_empty_file() {
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();
        let idx = SlotIndex::new(0, vec![]);
        w.write_entry(&Entry::new(TYPE_SLOT_INDEX, idx.encode()))
            .unwrap();

        let mut output = Vec::new();
        era1_to_era(Cursor::new(buf), &mut output, |_, buf| {
            buf.extend_from_slice(b"x");
            Ok::<(), Error>(())
        })
        .unwrap();

        let result = verify_era(&output[..]);
        assert!(result.valid);
        assert_eq!(result.block_count, 0);
    }

    #[test]
    fn era1_to_era_rejects_non_era1() {
        // Build an ERA file (not ERA1) and try to convert it
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();
        w.write_entry(&Entry::new(
            TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
            compress(&[0x01]),
        ))
        .unwrap();
        let idx = SlotIndex::with_count(0, vec![-8i64], 1);
        w.write_entry(&Entry::new(TYPE_SLOT_INDEX, idx.encode()))
            .unwrap();

        let mut output = Vec::new();
        let result = era1_to_era(Cursor::new(buf), &mut output, |_, buf| {
            buf.extend_from_slice(b"x");
            Ok::<(), Error>(())
        });

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected ERA1"));
    }
}
