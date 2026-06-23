//! Fast re-indexing of ERA/ERA1 files.
//!
//! Reads entries sequentially and writes them back with perfectly calculated
//! indexes. **Does not decompress block data** and **does not load the entire
//! file into memory** - entries are streamed one at a time.

use std::io::{Read, Seek, Write};

use crate::{
    error::{E2StoreError, Error},
    format::{
        Entry,
        e2store::E2StoreReader,
        era::{
            SlotIndex, TYPE_BLOCK_INDEX, TYPE_COMPRESSED_BEACON_STATE,
            TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, TYPE_STATE_INDEX,
        },
        era1::TYPE_COMPRESSED_HEADER,
    },
    write::E2StoreWriter,
};

/// Rebuilds an ERA or ERA1 file with correct, validated indexes.
///
/// Preserves entry order and slot numering exactly. Requires a seekable
/// source to read the original index for slot numbers.
pub fn reindex<R, W>(reader: R, writer: W) -> Result<(), Error>
where
    R: Read + Seek,
    W: Write,
{
    reindex_filtered(reader, writer, |_| true)
}

/// Rebuilds indexes while skipping entries that don't match the filter.
///
/// Used by [`strip`](super::strip::strip) to remove entries and rebuild
/// indexes in a single streaming pass.
pub(crate) fn reindex_filtered<R, W, F>(mut reader: R, writer: W, filter: F) -> Result<(), Error>
where
    R: Read + Seek,
    W: Write,
    F: Fn(&[u8; 2]) -> bool,
{
    let (block_slots, old_state_slot) = extract_index_metadata(&mut reader)?;

    reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(Error::Io)?;

    let mut e2s = E2StoreReader::new(&mut reader);
    let mut w = E2StoreWriter::new(writer);

    let version = e2s
        .next_entry()
        .map_err(Error::Io)?
        .ok_or(E2StoreError::MissingVersion)?;
    if !version.header.is_version() {
        return Err(E2StoreError::MissingVersion.into());
    }
    w.write_entry(&version)?;
    let mut pos: u64 = 8;

    let mut block_i = 0usize;
    let mut out_block_slots: Vec<u64> = Vec::new();
    let mut out_block_positions: Vec<u64> = Vec::new();
    let mut state_position: Option<u64> = None;

    while let Some(entry) = e2s.next_entry().map_err(Error::Io)? {
        // Always skip old indexes - we write new ones at the end
        match entry.header.typ {
            TYPE_BLOCK_INDEX | TYPE_STATE_INDEX => continue,
            _ => {}
        }

        if !filter(&entry.header.typ) {
            continue;
        }

        match entry.header.typ {
            TYPE_COMPRESSED_SIGNED_BEACON_BLOCK | TYPE_COMPRESSED_HEADER => {
                let slot = block_slots.get(block_i).copied().unwrap_or(block_i as u64);
                out_block_slots.push(slot);
                out_block_positions.push(pos);
                block_i += 1;
            }
            TYPE_COMPRESSED_BEACON_STATE => {
                state_position = Some(pos);
            }
            _ => {}
        }

        w.write_entry(&entry)?;
        pos += 8 + entry.data.len() as u64;
    }

    let block_index_pos = pos;
    let block_index = build_block_index(out_block_slots, out_block_positions, block_index_pos);
    w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, block_index.encode()))?;
    pos += 8 + block_index.encode().len() as u64;

    if let Some(abs_pos) = state_position {
        let state_slot = old_state_slot.unwrap_or(0);
        let state_index_pos = pos;
        let offset = abs_pos as i64 - state_index_pos as i64;
        let state_index = SlotIndex::new(state_slot, vec![offset]);
        w.write_entry(&Entry::new(TYPE_STATE_INDEX, state_index.encode()))?;
    }

    Ok(())
}

/// Extract slot list and state slot from the old indexes via header-only scan.
///
/// This reads only entry headers (8 bytes each), then seeks to read just
/// the index entries themselves.
fn extract_index_metadata<R: Read + Seek>(
    reader: &mut R,
) -> Result<(Vec<u64>, Option<u64>), Error> {
    reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(Error::Io)?;

    let mut block_idx_pos = None;
    let mut state_idx_pos = None;
    let mut pos = 0u64;

    {
        let mut scanner = E2StoreReader::new(&mut *reader);
        while let Some(header) = scanner.next_header_skip().map_err(Error::Io)? {
            match header.typ {
                TYPE_BLOCK_INDEX => block_idx_pos = Some(pos),
                TYPE_STATE_INDEX => state_idx_pos = Some(pos),
                _ => {}
            }
            pos += 8 + header.length as u64;
        }
    }

    let block_slots = match block_idx_pos {
        Some(p) => {
            reader
                .seek(std::io::SeekFrom::Start(p))
                .map_err(Error::Io)?;
            let mut r = E2StoreReader::new(&mut *reader);
            match r.next_entry().map_err(Error::Io)? {
                Some(e) => SlotIndex::decode(&e.data)
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
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
        None => Vec::new(),
    };

    let state_slot = match state_idx_pos {
        Some(p) => {
            reader
                .seek(std::io::SeekFrom::Start(p))
                .map_err(Error::Io)?;
            let mut r = E2StoreReader::new(reader);
            match r.next_entry().map_err(Error::Io)? {
                Some(e) => SlotIndex::decode(&e.data).ok().map(|idx| idx.starting_slot),
                None => None,
            }
        }
        None => None,
    };

    Ok((block_slots, state_slot))
}

/// Build a SlotIndex from collected slot numbers and their absolute file positions.
fn build_block_index(slots: Vec<u64>, positions: Vec<u64>, index_pos: u64) -> SlotIndex {
    if slots.is_empty() {
        return SlotIndex::new(0, vec![]);
    }

    let min_slot = *slots.first().unwrap();
    let max_slot = *slots.last().unwrap();
    let slot_range = (max_slot - min_slot + 1) as usize;

    let mut offsets = vec![0i64; slot_range];
    for (slot, abs_pos) in slots.iter().zip(positions.iter()) {
        let idx = (*slot - min_slot) as usize;
        offsets[idx] = *abs_pos as i64 - index_pos as i64;
    }

    SlotIndex::new(min_slot, offsets)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use snap::write::FrameEncoder;

    use super::*;
    use crate::{EraRandomReader, format::era1::*, verify::verify_era};

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.into_inner().unwrap()
    }

    fn build_era1_bad_index() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();

        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x32])))
            .unwrap();
        w.write_entry(&Entry::new(TYPE_BLOCK_BODY, vec![0x01]))
            .unwrap();
        let mut td = [0u8; 32];
        td[31] = 50;
        w.write_entry(&Entry::new(TYPE_TOTAL_DIFFICULTY, td.to_vec()))
            .unwrap();

        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x32])))
            .unwrap();
        w.write_entry(&Entry::new(TYPE_BLOCK_BODY, vec![0x02]))
            .unwrap();
        let mut td2 = [0u8; 32];
        td2[31] = 52;
        w.write_entry(&Entry::new(TYPE_TOTAL_DIFFICULTY, td2.to_vec()))
            .unwrap();

        let idx = SlotIndex::with_count(50, vec![-999i64, 0i64, -888i64], 2);
        w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, idx.encode()))
            .unwrap();

        buf
    }

    #[test]
    fn reindex_fixes_bad_offsets() {
        let input = build_era1_bad_index();
        let mut output = Vec::new();

        reindex(Cursor::new(input), &mut output).unwrap();

        let result = verify_era(&output[..]);
        if !result.valid {
            panic!("reindexed file failed:\n{}", result.errors.join("\n"));
        }

        let mut rr = EraRandomReader::new(Cursor::new(&output)).unwrap();
        assert_eq!(rr.starting_slot(), Some(50));
        assert_eq!(rr.slot_count(), Some(3));
        assert!(rr.read_block_at_slot(50).unwrap().is_some());
        assert!(rr.read_block_at_slot(51).unwrap().is_none());
        assert!(rr.read_block_at_slot(52).unwrap().is_some());
    }

    #[test]
    fn reindex_preserves_data_integrity() {
        let input = build_era1_bad_index();
        let mut output = Vec::new();
        reindex(Cursor::new(input), &mut output).unwrap();

        let mut rr = EraRandomReader::new(Cursor::new(&output)).unwrap();
        let full = rr.read_full_era1_block_at_slot(50).unwrap().unwrap();
        assert_eq!(full.header, vec![0x32]);
    }

    #[test]
    fn reindex_empty_file() {
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();
        let idx = SlotIndex::new(0, vec![]);
        w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, idx.encode()))
            .unwrap();

        let mut output = Vec::new();
        reindex(Cursor::new(buf), &mut output).unwrap();

        let result = verify_era(&output[..]);
        assert!(result.valid);
    }
}
