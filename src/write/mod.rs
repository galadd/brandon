//! ERA file writer.

use std::io::{Result, Write};

use crate::format::Entry;
use crate::format::era::*;

pub struct E2StoreWriter<W: Write> {
    writer: W,
}

impl<W: Write> E2StoreWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }

    pub fn write_entry(&mut self, entry: &Entry) -> Result<()> {
        self.writer.write_all(&entry.header.typ)?;
        self.writer.write_all(&entry.header.length.to_le_bytes())?;
        self.writer.write_all(&entry.data)?;
        Ok(())
    }

    pub fn write_all(&mut self, entries: &[Entry]) -> Result<()> {
        for entry in entries {
            self.write_entry(entry)?;
        }
        Ok(())
    }
}

pub struct EraBuilder {
    blocks: Vec<(u64, Vec<u8>)>,
    state: Option<Vec<u8>>,
}

impl EraBuilder {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            state: None,
        }
    }

    pub fn add_block(&mut self, slot: u64, compressed_block: Vec<u8>) {
        self.blocks.push((slot, compressed_block));
    }

    pub fn set_state(&mut self, compressed_state: Vec<u8>) {
        self.state = Some(compressed_state);
    }

    pub fn build<W: Write>(&self, output: &mut W) -> std::io::Result<()> {
        let mut writer = E2StoreWriter::new(output);

        let mut version = Entry::version();
        version.header.reserved = 0;
        writer.write_entry(&version)?;

        for (_, block) in &self.blocks {
            let mut entry = Entry::new(TYPE_COMPRESSED_SIGNED_BEACON_BLOCK, block.clone());
            entry.header.reserved = 0;
            writer.write_entry(&entry)?;
        }

        if let Some(state_bytes) = &self.state {
            let mut entry = Entry::new(TYPE_COMPRESSED_BEACON_STATE, state_bytes.clone());
            entry.header.reserved = 0;
            writer.write_entry(&entry)?;
        }

        let mut offsets = Vec::new();
        let mut current_offset = 0i64;
        for (_, block_bytes) in &self.blocks {
            offsets.push(current_offset);
            current_offset += block_bytes.len() as i64;
        }

        let starting_slot = self.blocks.first().map(|(s, _)| *s).unwrap_or(0);
        let block_index = SlotIndex::new(starting_slot, offsets);
        let mut entry = Entry::new(TYPE_BLOCK_INDEX, block_index.encode());
        entry.header.reserved = 0;
        writer.write_entry(&entry)?;

        if self.state.is_some() {
            let mut state_offsets = vec![0];
            if let Some(state_bytes) = &self.state {
                state_offsets.push(state_bytes.len() as i64);
            } else {
                state_offsets.push(0);
            }

            let state_index = SlotIndex::new(0, state_offsets);
            let mut entry = Entry::new(TYPE_STATE_INDEX, state_index.encode());
            entry.header.reserved = 0;
            writer.write_entry(&entry)?;
        }

        Ok(())
    }
}
