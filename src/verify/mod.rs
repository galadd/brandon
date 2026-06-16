//! ERA file verification.

pub mod hash;

use std::io::Read;

use crate::format::{
    e2store::E2StoreReader,
    era::{
        SlotIndex, TYPE_BLOCK_INDEX, TYPE_COMPRESSED_BEACON_STATE,
        TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, TYPE_STATE_INDEX, decompress_entry,
    },
    era1::{TYPE_BLOCK_BODY, TYPE_COMPRESSED_HEADER, TYPE_RECEIPTS, TYPE_TOTAL_DIFFICULTY},
};

pub struct VerificationResult {
    pub valid: bool,
    pub block_count: usize,
    pub state_present: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn verify_era<R: Read>(reader: R) -> VerificationResult {
    let mut result = VerificationResult {
        valid: true,
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

    let mut block_entries = 0usize;
    let mut block_index = None;
    let mut state_index = None;

    for (i, entry) in entries.iter().enumerate() {
        if i == 0 {
            continue; // version entry
        }

        match entry.header.typ {
            TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => {
                block_entries += 1;

                if let Err(e) = decompress_entry(&entry.data) {
                    result.valid = false;
                    result.errors.push(format!(
                        "block {} failed decompression: {}",
                        block_entries, e
                    ));
                }
            }

            TYPE_COMPRESSED_BEACON_STATE => {
                result.state_present = true;

                if let Err(e) = decompress_entry(&entry.data) {
                    result.valid = false;
                    result
                        .errors
                        .push(format!("state failed decompression: {}", e));
                }
            }

            TYPE_BLOCK_INDEX => match SlotIndex::decode(&entry.data) {
                Ok(idx) => block_index = Some(idx),
                Err(e) => {
                    result.valid = false;
                    result.errors.push(format!("invalid block index: {}", e));
                }
            },

            TYPE_STATE_INDEX => match SlotIndex::decode(&entry.data) {
                Ok(idx) => state_index = Some(idx),
                Err(e) => {
                    result.valid = false;
                    result.errors.push(format!("invalid state index: {}", e));
                }
            },

            // ERA1 entries
            TYPE_COMPRESSED_HEADER => {
                block_entries += 1;
            }
            TYPE_BLOCK_BODY | TYPE_RECEIPTS | TYPE_TOTAL_DIFFICULTY => {}

            _ => {
                result.warnings.push(format!(
                    "unknown entry type {:02x}{:02x}",
                    entry.header.typ[0], entry.header.typ[1]
                ));
            }
        }
    }

    result.block_count = block_entries;

    // Index consistency checks
    if let Some(idx) = block_index {
        let indexed_blocks = idx.offsets.iter().filter(|&&o| o != 0).count();

        if indexed_blocks != block_entries {
            result.valid = false;
            result.errors.push(format!(
                "block index count mismatch: index={}, entries={}",
                indexed_blocks, block_entries
            ));
        }
    } else {
        result.valid = false;
        result.errors.push("missing block index".into());
    }

    if result.state_present && state_index.is_none() {
        result.valid = false;
        result
            .errors
            .push("state present but state index missing".into());
    }

    result
}

// #[test]
// fn verify_mainnet_era1_file() {
//     use crate::verify::verify_era;
//
//     let data = std::fs::read("tests/fixtures/mainnet-00000-5ec1ffb8.era1").expect("fixture missing");
//
//     let result = verify_era(&data[..]);
//
//     if !result.valid {
//         panic!("verification failed:\n{}", result.errors.join("\n"));
//     }
//
//     assert!(result.block_count > 0);
// }
