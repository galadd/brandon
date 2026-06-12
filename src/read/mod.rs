//! Streaming reader for e2store-based file formats.

use std::io::{self, Read};

use crate::format::e2store::E2StoreReader;
use crate::format::era::*;
use crate::format::era1::{
    TYPE_BLOCK_BODY, TYPE_COMPRESSED_HEADER, TYPE_RECEIPTS, TYPE_TOTAL_DIFFICULTY,
};

#[derive(Debug)]
pub struct EraFile {
    pub blocks: Vec<Vec<u8>>,
    pub state: Vec<u8>,
    pub block_index: Option<SlotIndex>,
    pub state_index: Option<SlotIndex>,
    pub is_era1: bool,
}

impl EraFile {
    pub fn read<R: Read>(reader: R) -> Result<Self, io::Error> {
        let reader = E2StoreReader::new(reader);
        let entries = reader.read_all()?;

        if entries.len() < 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "empty e2store file",
            ));
        }

        // Skip version entry
        let entries = &entries[1..];

        let mut blocks = Vec::new();
        let mut state = Vec::new();
        let mut block_index = None;
        let mut state_index = None;
        let mut is_era1 = false;

        for entry in entries {
            match entry.header.typ {
                TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => {
                    // ERA post merge
                    let block = decompress_entry(&entry.data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    blocks.push(block);
                }
                TYPE_COMPRESSED_BEACON_STATE => {
                    state = decompress_entry(&entry.data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                }
                TYPE_COMPRESSED_HEADER => {
                    is_era1 = true;
                    let block = decompress_entry(&entry.data)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    blocks.push(block);
                }

                TYPE_BLOCK_BODY | TYPE_RECEIPTS | TYPE_TOTAL_DIFFICULTY => {
                    is_era1 = true;
                    blocks.push(entry.data.clone());
                }

                TYPE_BLOCK_INDEX => {
                    block_index = Some(
                        SlotIndex::decode(&entry.data)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                    );
                }
                TYPE_STATE_INDEX => {
                    state_index = Some(
                        SlotIndex::decode(&entry.data)
                            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                    );
                }
                _ => {
                    // unknown entry type — skip for forward compatibility
                    println!(
                        "Warning: unknown entry type {:02x}{:02x}, length {}",
                        entry.header.typ[0], entry.header.typ[1], entry.header.length
                    );
                }
            }
        }

        Ok(EraFile {
            blocks,
            state,
            block_index,
            state_index,
            is_era1,
        })
    }
}

#[test]
fn parse_mainnet_era_0() {
    use crate::read::EraFile;

    let data = std::fs::read("mainnet-00000-5ec1ffb8.era1").unwrap();

    let era = EraFile::read(&data[..]).unwrap();

    assert!(!era.blocks.is_empty());
    if era.is_era1 {
        assert!(era.state.is_empty());
    } else {
        assert!(!era.state.is_empty());
        assert!(era.state_index.is_some());
    }

    assert!(era.block_index.is_some());

    let block_index = era.block_index.unwrap();

    assert!(!block_index.offsets.is_empty());
}
