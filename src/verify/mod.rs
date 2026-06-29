//! ERA/ERA1 file verification.
//!
//! Performs comprehensive structural validation of e2store-based archive files:
//!
//! | Check | Description |
//! |---|---|
//! | Structural | Valid e2store headers, version entry first |
//! | Decompression | All compressed entries decode without error |
//! | Index integrity | Block/state indexes decode, counts match, offsets valid |
//! | Offset resolution | Index offsets point to correct entry types within file bounds |
//! | Block groups | ERA1 blocks have complete header+body+td before next block |
//! | Cross-reference | State present ↔ state index present, state index points to state |
//!
//! SHA256 manifest verification is in the [`hash`](self::hash) submodule.

pub mod hash;

use std::{collections::HashMap, io::Read};

use crate::format::{
    e2store::E2StoreReader,
    era::{
        SlotIndex, TYPE_COMPRESSED_BEACON_STATE, TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
        TYPE_SLOT_INDEX, decompress_entry,
    },
    era1::{
        TYPE_BLOCK_ACCUMULATOR, TYPE_BLOCK_BODY, TYPE_BLOCK_INDEX, TYPE_COMPRESSED_HEADER,
        TYPE_RECEIPTS, TYPE_TOTAL_DIFFICULTY,
    },
};

pub struct VerificationResult {
    pub valid: bool,
    /// Detected file form: `"ERA"` or `"ERA1`.
    pub format: Option<String>,
    pub block_count: usize,
    pub state_present: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Tracks an ERA1 block group being assembled during verification.
///
/// ERA1 blocks span consecutive entries: CompressedHeader, BlockBody,
/// Receipts (optional), TotalDifficulty. A new CompressedHeader signals
/// the previous group is complete.
struct Era1Group {
    /// Index of the header entry (for error messages).
    header_idx: usize,
    has_body: bool,
    has_receipts: bool,
    has_td: bool,
}

impl Era1Group {
    fn new(header_idx: usize) -> Self {
        Self {
            header_idx,
            has_body: false,
            has_receipts: false,
            has_td: false,
        }
    }

    /// Required parts are header (implicit) + body + td.
    fn is_complete(&self) -> bool {
        self.has_body && self.has_td
    }

    fn missing_parts(&self) -> String {
        let mut parts = Vec::new();
        if !self.has_body {
            parts.push("BlockBody");
        }
        if !self.has_td {
            parts.push("TotalDifficulty");
        }
        parts.join(", ")
    }
}

/// Comprehensive ERA/ERA1 file verification.
///
/// Loads the entire file into memory. For large files, use
/// [`crate::format::e2store::E2StoreReader::scan_all_headers`] for a
/// lightweight structural pre-check instead.
pub fn verify_era<R: Read>(reader: R) -> VerificationResult {
    let mut result = VerificationResult {
        valid: true,
        format: None,
        block_count: 0,
        state_present: false,
        errors: Vec::new(),
        warnings: Vec::new(),
    };

    let reader = E2StoreReader::new(reader);

    let entries = match reader.read_all() {
        Ok(v) => v,
        Err(e) => {
            result.valid = false;
            result
                .errors
                .push(format!("failed to read e2store reader: {e}"));
            return result;
        }
    };

    if entries.is_empty() {
        result.valid = false;
        result.errors.push("empty e2store file".into());
        return result;
    }

    if !entries[0].header.is_version() {
        result.valid = false;
        result
            .errors
            .push("first entry is not a version entry".into());
        return result;
    }

    let mut pos: u64 = 0;
    let mut pos_to_entry: HashMap<u64, usize> = HashMap::new();

    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            pos_to_entry.insert(pos, i);
        }
        pos += 8 + entry.data.len() as u64;
    }

    let file_size = pos;

    let mut block_count = 0usize;
    let mut state_present = false;
    let mut slot_index_seen = false;
    let mut block_index: Option<SlotIndex> = None;
    let mut block_index_pos: Option<u64> = None;
    let mut state_index: Option<SlotIndex> = None;
    let mut state_index_pos: Option<u64> = None;
    let mut state_entry_pos: Option<u64> = None;
    let mut detected_format: Option<&str> = None;
    let mut era1_group: Option<Era1Group> = None;

    let mut pos: u64 = 0;
    for (i, entry) in entries.iter().enumerate() {
        let entry_pos = pos;
        pos += 8 + entry.data.len() as u64;

        if i == 0 {
            continue; // version entry
        }

        match entry.header.typ {
            TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => {
                if detected_format.is_none() {
                    detected_format = Some("ERA");
                }
                block_count += 1;

                if let Err(e) = decompress_entry(&entry.data) {
                    result.valid = false;
                    result
                        .errors
                        .push(format!("entry {i}: beacon block decompression failed: {e}"));
                }
            }

            TYPE_COMPRESSED_BEACON_STATE => {
                state_present = true;
                state_entry_pos = Some(entry_pos);
                if let Err(e) = decompress_entry(&entry.data) {
                    result.valid = false;
                    result
                        .errors
                        .push(format!("entry {i}: beacon state decompression failed: {e}"));
                }
            }

            TYPE_SLOT_INDEX => {
                if !slot_index_seen {
                    slot_index_seen = true;

                    block_index_pos = Some(entry_pos);
                    match SlotIndex::decode(&entry.data) {
                        Ok(idx) => block_index = Some(idx),
                        Err(e) => {
                            result.valid = false;
                            result
                                .errors
                                .push(format!("entry {i}: invalid block index: {e}"));
                        }
                    }
                } else {
                    state_index_pos = Some(entry_pos);
                    match SlotIndex::decode(&entry.data) {
                        Ok(idx) => state_index = Some(idx),
                        Err(e) => {
                            result.valid = false;
                            result
                                .errors
                                .push(format!("entry {i}: invalid state index: {e}"));
                        }
                    }
                }
            }

            // ERA1 entries
            TYPE_COMPRESSED_HEADER => {
                if detected_format.is_none() {
                    detected_format = Some("ERA1");
                }

                // Flush previous group
                if let Some(group) = era1_group.take()
                    && !group.is_complete()
                {
                    result.valid = false;
                    result.errors.push(format!(
                        "entry {i}: incomplete ERA1 block starting at entry {} (missing: {})",
                        group.header_idx,
                        group.missing_parts()
                    ));
                }

                block_count += 1;
                era1_group = Some(Era1Group::new(i));

                if let Err(e) = decompress_entry(&entry.data) {
                    result.valid = false;
                    result
                        .errors
                        .push(format!("entry {i}: header decompression failed: {e}"));
                }
            }

            TYPE_BLOCK_BODY => {
                if detected_format.is_none() {
                    detected_format = Some("ERA1");
                }
                if let Some(ref mut g) = era1_group {
                    g.has_body = true;
                } else {
                    result
                        .warnings
                        .push(format!("entry {i}: BlockBody without preceding header"));
                }
            }

            TYPE_RECEIPTS => {
                if detected_format.is_none() {
                    detected_format = Some("ERA1");
                }
                if let Some(ref mut g) = era1_group {
                    g.has_receipts = true;
                } else {
                    result
                        .warnings
                        .push(format!("entry {i}: Receipts without preceding header"));
                }
            }

            TYPE_TOTAL_DIFFICULTY => {
                if detected_format.is_none() {
                    detected_format = Some("ERA1");
                }
                if entry.data.len() != 32 {
                    result.valid = false;
                    result.errors.push(format!(
                        "entry {i}: TotalDifficulty must be 32 bytes, got {}",
                        entry.data.len()
                    ));
                }
                if let Some(ref mut g) = era1_group {
                    g.has_td = true;
                } else {
                    result.warnings.push(format!(
                        "entry {i}: TotalDifficulty without preceding header"
                    ));
                }
            }

            TYPE_BLOCK_ACCUMULATOR => {
                if entry.data.len() != 32 {
                    result.valid = false;
                    result.errors.push(format!(
                        "entry {i}: BlockAccumulator must be 32 bytes, got {}",
                        entry.data.len()
                    ));
                }
            }

            TYPE_BLOCK_INDEX => {
                block_index_pos = Some(entry_pos);

                match SlotIndex::decode(&entry.data) {
                    Ok(idx) => block_index = Some(idx),
                    Err(e) => {
                        result.valid = false;
                        result
                            .errors
                            .push(format!("entry {i}: invalid block index: {e}"));
                    }
                }
            }

            _ => {
                result.warnings.push(format!(
                    "entry {i}: unknown entry type {:02x}{:02x}",
                    entry.header.typ[0], entry.header.typ[1]
                ));
            }
        }
    }

    // Flush final ERA1 group
    if let Some(group) = era1_group
        && !group.is_complete()
    {
        result.valid = false;
        result.errors.push(format!(
            "incomplete final ERA1 block starting at entry {} (missing: {})",
            group.header_idx,
            group.missing_parts()
        ));
    }

    result.format = detected_format.map(String::from);
    result.block_count = block_count;
    result.state_present = state_present;

    match (&block_index, block_index_pos) {
        (None, _) => {
            result.valid = false;
            result.errors.push("missing block index".into());
        }
        (Some(idx), Some(idx_pos)) => {
            let indexed_blocks = idx
                .offsets
                .iter()
                .filter(|&&o| {
                    if o == 0 {
                        return false;
                    }
                    let abs = (idx_pos as i64 + o) as u64;
                    abs != 0
                })
                .count();

            if indexed_blocks != block_count {
                result.valid = false;
                result.errors.push(format!(
                    "block index has {indexed_blocks} non-zero offsets, file has {block_count} block entries"
                ));
            }

            if idx.count as usize != idx.offsets.len() {
                result.warnings.push(format!(
                    "block index count field ({}) does not match offset array length ({})",
                    idx.count,
                    idx.offsets.len()
                ));
            }

            let expected_type = match detected_format {
                Some("ERA") => TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
                Some("ERA1") => TYPE_COMPRESSED_HEADER,
                _ => [0, 0],
            };

            for (slot_i, &offset) in idx.offsets.iter().enumerate() {
                if offset == 0 {
                    continue;
                }

                let abs = (idx_pos as i64 + offset) as u64;
                let slot = idx.starting_slot + slot_i as u64;

                if abs == 0 {
                    continue;
                }

                if abs >= file_size {
                    result.valid = false;
                    result.errors.push(format!(
                        "block index slot {slot}: offset resolves to byte {abs}, past file size {file_size}"
                    ));
                    continue;
                }

                if expected_type != [0, 0] {
                    match pos_to_entry.get(&abs) {
                        Some(&entry_idx) => {
                            let actual = entries[entry_idx].header.typ;
                            if actual != expected_type {
                                result.valid = false;
                                result.errors.push(format!(
                                    "block index slot {slot}: offset points to entry {entry_idx} (type {:02x}{:02x}), expected {:02x}{:02x}",
                                    actual[0], actual[1],
                                    expected_type[0], expected_type[1]
                                ));
                            }
                        }
                        None => {
                            result.valid = false;
                            result.errors.push(format!(
                                "block index slot {slot}: offset resolves to byte {abs}, which does not align to any entry boundary"
                            ));
                        }
                    }
                }
            }
        }
        (Some(_), None) => {
            result.valid = false;
            result
                .errors
                .push("block index found but position not tracked".into());
        }
    }

    if state_present {
        match (&state_index, state_index_pos) {
            (None, _) => {
                result.valid = false;
                result
                    .errors
                    .push("beacon state present but state index missing".into());
            }
            (Some(idx), Some(idx_pos)) => {
                if idx.offsets.is_empty() {
                    result.valid = false;
                    result.errors.push("state index contains no offsets".into());
                } else if let Some(expected_pos) = state_entry_pos {
                    let offset = idx.offsets[0];
                    if offset == 0 {
                        result.valid = false;
                        result
                            .errors
                            .push("state index offset is 0 (does not reference state)".into());
                    } else {
                        let abs = (idx_pos as i64 + offset) as u64;
                        if abs != expected_pos {
                            result.valid = false;
                            result.errors.push(format!(
                                "state index offset resolves to byte {abs}, expected {expected_pos}"
                            ));
                        }
                    }
                }
            }
            (Some(_), None) => {
                result.valid = false;
                result
                    .errors
                    .push("state index found but position not tracked".into());
            }
        }
    } else if state_index.is_some() {
        result
            .warnings
            .push("state index present but no beacon state entry found".into());
    }

    result
}

#[cfg(test)]
mod tests {
    use std::io::Write as IoWrite;

    use snap::write::FrameEncoder;

    use super::*;
    use crate::{format::Entry, write::EraBuilder};

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.into_inner().unwrap()
    }

    fn make_era1_header_entry(data: &[u8]) -> Vec<u8> {
        let e = Entry::new(TYPE_COMPRESSED_HEADER, compress(data));
        let mut buf = Vec::new();
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);
        buf
    }

    fn make_body_entry() -> Vec<u8> {
        let e = Entry::new(TYPE_BLOCK_BODY, vec![0x01]);
        let mut buf = Vec::new();
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);
        buf
    }

    fn make_td_entry(slot: u8) -> Vec<u8> {
        let mut td = [0u8; 32];
        td[31] = slot;
        let e = Entry::new(TYPE_TOTAL_DIFFICULTY, td.to_vec());
        let mut buf = Vec::new();
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);
        buf
    }

    fn make_receipts_entry() -> Vec<u8> {
        let e = Entry::new(TYPE_RECEIPTS, vec![0x04, 0x05]);
        let mut buf = Vec::new();
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);
        buf
    }

    fn make_slot_index(offsets: Vec<i64>) -> Vec<u8> {
        let idx = SlotIndex::new(0, offsets);
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        let mut buf = Vec::new();
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);
        buf
    }

    #[test]
    fn empty_file() {
        let r = verify_era(&[][..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("empty")));
    }

    #[test]
    fn missing_version() {
        let buf = [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("version")));
    }

    #[test]
    fn version_only() {
        let buf = [0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("block index")));
    }

    #[test]
    fn builder_two_blocks_and_state() {
        let mut b = EraBuilder::new();
        b.add_block(0, compress(&[0xAA; 50]));
        b.add_block(1, compress(&[0xBB; 50]));
        b.set_state(2, compress(&[0xCC; 100]));

        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        let r = verify_era(&buf[..]);
        if !r.valid {
            panic!("verification failed:\n{}", r.errors.join("\n"));
        }
        assert_eq!(r.block_count, 2);
        assert!(r.state_present);
        assert_eq!(r.format.as_deref(), Some("ERA"));
        assert!(r.errors.is_empty());
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn builder_empty() {
        let mut buf = Vec::new();
        EraBuilder::new().build(&mut buf).unwrap();

        let r = verify_era(&buf[..]);
        assert!(r.valid);
        assert_eq!(r.block_count, 0);
        assert!(!r.state_present);
    }

    #[test]
    fn builder_with_skipped_slots() {
        let mut b = EraBuilder::new();
        b.add_block(0, compress(&[0x01; 30]));
        b.add_block(5, compress(&[0x02; 30]));
        b.add_block(7, compress(&[0x03; 30]));

        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        let r = verify_era(&buf[..]);
        if !r.valid {
            panic!("verification failed:\n{}", r.errors.join("\n"));
        }
        assert_eq!(r.block_count, 3);
    }

    #[test]
    fn builder_single_block() {
        let mut b = EraBuilder::new();
        b.add_block(42, compress(&[0xFF; 10]));

        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        let r = verify_era(&buf[..]);
        if !r.valid {
            panic!("verification failed:\n{}", r.errors.join("\n"));
        }
        assert_eq!(r.block_count, 1);
    }

    #[test]
    fn corrupted_block_data() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let block_pos = buf.len() as u64;

        let e = Entry::new(TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, vec![0xFF, 0xFF, 0xFF]);

        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let block_index_pos = buf.len() as u64;

        let block_offset = block_pos as i64 - block_index_pos as i64;

        let idx = SlotIndex::with_count(0, vec![block_offset], 1);

        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);

        assert!(!r.valid);

        assert!(r.errors.iter().any(|e| e.contains("decompression")));
    }

    #[test]
    fn era1_complete_block_passes() {
        let version = Entry::version();
        let header = make_era1_header_entry(&[0x01]);
        let body = make_body_entry();
        let td = make_td_entry(0);

        let header_pos = 8;
        let block_index_pos = 8 + header.len() as u64 + body.len() as u64 + td.len() as u64;
        let offset = header_pos as i64 - block_index_pos as i64;

        let block_index = make_slot_index(vec![offset]);

        let mut buf = Vec::new();
        buf.extend_from_slice(&version.header.encode());
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&body);
        buf.extend_from_slice(&td);
        buf.extend_from_slice(&block_index);

        let r = verify_era(&buf[..]);
        if !r.valid {
            panic!("verification failed:\n{}", r.errors.join("\n"));
        }
        assert_eq!(r.block_count, 1);
        assert_eq!(r.format.as_deref(), Some("ERA1"));
    }

    #[test]
    fn era1_complete_block_with_receipts_passes() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let header_pos = buf.len() as u64;
        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_body_entry());
        buf.extend_from_slice(&make_receipts_entry());
        buf.extend_from_slice(&make_td_entry(0));

        let index_pos = buf.len() as u64;
        let offset = header_pos as i64 - index_pos as i64;

        buf.extend_from_slice(&make_slot_index(vec![offset]));

        let r = verify_era(&buf[..]);
        if !r.valid {
            panic!("verification failed:\n{}", r.errors.join("\n"));
        }
        assert_eq!(r.block_count, 1);
    }

    #[test]
    fn era1_missing_body_fails() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let header_pos = buf.len() as u64;
        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_td_entry(0));

        let index_pos = buf.len() as u64;
        let offset = header_pos as i64 - index_pos as i64;

        buf.extend_from_slice(&make_slot_index(vec![offset]));

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("incomplete") && e.contains("BlockBody"))
        );
    }

    #[test]
    fn era1_missing_td_fails() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let header_pos = buf.len() as u64;
        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_body_entry());

        let index_pos = buf.len() as u64;
        let offset = header_pos as i64 - index_pos as i64;

        buf.extend_from_slice(&make_slot_index(vec![offset]));

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("incomplete") && e.contains("TotalDifficulty"))
        );
    }

    #[test]
    fn era1_incomplete_final_block_fails() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let header0_pos = buf.len() as u64;
        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_body_entry());
        buf.extend_from_slice(&make_td_entry(0));

        let header1_pos = buf.len() as u64;
        buf.extend_from_slice(&make_era1_header_entry(&[0x02]));

        let index_pos = buf.len() as u64;

        let offset0 = header0_pos as i64 - index_pos as i64;
        let offset1 = header1_pos as i64 - index_pos as i64;

        buf.extend_from_slice(&make_slot_index(vec![offset0, offset1]));

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("incomplete final")));
    }

    #[test]
    fn bad_td_size_fails() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let header_pos = buf.len() as u64;

        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_body_entry());

        // TD with 16 bytes instead of 32
        let e = Entry::new(TYPE_TOTAL_DIFFICULTY, vec![0u8; 16]);
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let index_pos = buf.len() as u64;

        let offset = header_pos as i64 - index_pos as i64;

        buf.extend_from_slice(&make_slot_index(vec![offset]));

        let r = verify_era(&buf[..]);

        assert!(!r.valid);

        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("TotalDifficulty") && e.contains("32"))
        );
    }

    #[test]
    fn bad_accumulator_size_fails() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let e = Entry::new(TYPE_BLOCK_ACCUMULATOR, vec![0u8; 16]);
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        // Empty index (no blocks)
        buf.extend_from_slice(&make_slot_index(vec![]));

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("BlockAccumulator") && e.contains("32"))
        );
    }

    #[test]
    fn block_accumulator_is_known() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let e = Entry::new(TYPE_BLOCK_ACCUMULATOR, [0u8; 32].to_vec());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        buf.extend_from_slice(&make_slot_index(vec![]));

        let r = verify_era(&buf[..]);
        // Fails because no blocks, but must NOT warn about unknown type
        assert!(
            !r.warnings
                .iter()
                .any(|w| w.contains("unknown") || w.contains("0700"))
        );
    }

    #[test]
    fn truly_unknown_type_warns() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let e = Entry::new([0xFE, 0xEE], vec![0x01, 0x02]);
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        buf.extend_from_slice(&make_slot_index(vec![]));

        let r = verify_era(&buf[..]);
        assert!(r.warnings.iter().any(|w| w.contains("feee")));
    }

    #[test]
    fn orphan_body_warns() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());
        buf.extend_from_slice(&make_body_entry());
        buf.extend_from_slice(&make_slot_index(vec![]));

        let r = verify_era(&buf[..]);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("BlockBody") && w.contains("header"))
        );
    }

    #[test]
    fn state_without_index_fails() {
        let mut b = EraBuilder::new();
        b.add_block(0, compress(&[0x01]));
        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        // Manually append a state entry without a state index
        let e = Entry::new(TYPE_COMPRESSED_BEACON_STATE, compress(&[0xFF; 20]));
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("state") && e.contains("index"))
        );
    }

    #[test]
    fn index_without_state_warns() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let block_pos = buf.len() as u64;

        let e = Entry::new(TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, compress(&[0x01]));
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let block_index_pos = buf.len() as u64;

        let block_offset = block_pos as i64 - block_index_pos as i64;
        let idx = SlotIndex::with_count(0, vec![block_offset], 1);

        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        // State index with no actual state entry
        let idx = SlotIndex::with_count(8192, vec![-100i64], 1);

        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);

        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("state index") && w.contains("no beacon state"))
        );
    }

    #[test]
    fn offset_points_to_wrong_type_fails() {
        let mut buf = Vec::new();

        buf.extend_from_slice(&Entry::version().header.encode());

        let body_pos = buf.len() as u64;

        let e = Entry::new(TYPE_BLOCK_BODY, vec![0x01]);
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let index_pos = buf.len() as u64;
        let offset = body_pos as i64 - index_pos as i64;

        let idx = SlotIndex::with_count(0, vec![offset], 1);
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);

        assert!(!r.valid);

        println!("{:#?}", r.errors);

        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("expected") && e.contains("type"))
        );
    }

    #[test]
    fn offset_past_end_of_file_fails() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let idx = SlotIndex::with_count(0, vec![999999i64], 1);
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("past file size")));
    }

    #[test]
    fn offset_not_on_entry_boundary_fails() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let e = Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x01]));
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        // Offset points to byte 12 (mid-entry, not aligned)
        let idx = SlotIndex::with_count(0, vec![4i64], 1); // 8 + 4 = 12
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("does not align")));
    }

    #[test]
    fn non_zero_offset_count_mismatch_fails() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let block_pos = buf.len() as u64;
        let e = Entry::new(TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, compress(&[0x01]));
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let index_pos = buf.len() as u64;
        let offset1 = block_pos as i64 - index_pos as i64;
        let offset2 = block_pos as i64 - index_pos as i64;

        // Index has 2 offsets pointing to real blocks, but file only has 1 block
        let idx = SlotIndex::with_count(0, vec![offset1, offset2], 2);
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);
        assert!(!r.valid);
        assert!(
            r.errors
                .iter()
                .any(|e| e.contains("non-zero offsets") && e.contains("2") && e.contains("1"))
        );
    }

    #[test]
    fn count_field_mismatch_warns() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());

        let block_pos = buf.len() as u64;
        let e = Entry::new(TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, compress(&[0x01]));
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let index_pos = buf.len() as u64;
        let offset = block_pos as i64 - index_pos as i64;

        // count field says 5, but offset array has 1 element
        let idx = SlotIndex::with_count(0, vec![offset], 5);
        let e = Entry::new(TYPE_SLOT_INDEX, idx.encode());
        buf.extend_from_slice(&e.header.encode());
        buf.extend_from_slice(&e.data);

        let r = verify_era(&buf[..]);
        assert!(r.warnings.iter().any(|w| w.contains("count field")));
    }

    #[test]
    fn count_matches_after_builder_roundtrip() {
        let mut b = EraBuilder::new();
        b.add_block(0, compress(&[0x01]));
        b.add_block(1, compress(&[0x02]));
        b.add_block(3, compress(&[0x03])); // slot 2 skipped

        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        // Parse the index and verify count directly
        let entries = crate::format::e2store::E2StoreReader::new(std::io::Cursor::new(&buf))
            .read_all()
            .unwrap();
        let idx_entry = entries
            .iter()
            .find(|e| e.header.typ == TYPE_SLOT_INDEX)
            .unwrap();
        let idx = SlotIndex::decode(&idx_entry.data).unwrap();

        assert_eq!(idx.offsets.len(), 4); // slots 0,1,2,3
        assert_eq!(idx.count, 4); // 3 non-zero (slot 2 is 0)
    }

    #[test]
    fn format_detected_as_era() {
        let mut b = EraBuilder::new();
        b.add_block(0, compress(&[0x01]));
        let mut buf = Vec::new();
        b.build(&mut buf).unwrap();

        assert_eq!(verify_era(&buf[..]).format.as_deref(), Some("ERA"));
    }

    #[test]
    fn format_detected_as_era1() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Entry::version().header.encode());
        buf.extend_from_slice(&make_era1_header_entry(&[0x01]));
        buf.extend_from_slice(&make_body_entry());
        buf.extend_from_slice(&make_td_entry(0));
        buf.extend_from_slice(&make_slot_index(vec![-8i64]));

        assert_eq!(verify_era(&buf[..]).format.as_deref(), Some("ERA1"));
    }
}
