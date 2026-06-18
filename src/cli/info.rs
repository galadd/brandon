use std::{collections::BTreeMap, fs};

use anyhow::{Context, bail};
use brandon::{
    EraRandomReader,
    format::{e2store::E2StoreReader, era::TYPE_STATE_INDEX},
};
use serde::Serialize;

use super::{count_entry_types, human_size, open_file, output};

#[derive(Serialize)]
pub struct InfoResult {
    pub format: String,
    pub file_size: u64,
    pub file_size_human: String,
    pub starting_slot: u64,
    pub slot_range: [u64; 2],
    pub block_count: usize,
    pub state_present: bool,
    pub total_entries: usize,
    pub entries: BTreeMap<String, usize>,
}

impl std::fmt::Display for InfoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Format:            {}", self.format)?;
        writeln!(
            f,
            "File size:         {} ({})",
            self.file_size_human, self.file_size
        )?;
        writeln!(f, "Starting slot:     {}", self.starting_slot)?;
        writeln!(
            f,
            "Slot range:        {}..{}",
            self.slot_range[0], self.slot_range[1]
        )?;
        writeln!(f, "Block count:       {}", self.block_count)?;
        writeln!(
            f,
            "State present:     {}",
            if self.state_present { "yes" } else { "no" }
        )?;
        writeln!(f, "Total entries:     {}", self.total_entries)?;
        writeln!(f)?;
        writeln!(f, "Entry breakdown:")?;
        // Find the longest name for allignment
        let max_name = self.entries.keys().map(|k| k.len()).max().unwrap_or(0);
        for (name, count) in &self.entries {
            writeln!(f, "  {:<width$} {}", name, count, width = max_name)?;
        }
        Ok(())
    }
}

pub fn run(path: &str, json: bool) -> anyhow::Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("cannnot stat {path}"))?;
    let file_size = metadata.len();

    let file = open_file(path)?;
    let mut reader = E2StoreReader::new(file);

    // Fast header-only scan - no payload data is read
    let headers = reader
        .scan_all_headers()
        .with_context(|| format!("failed to scan headers in {path}"))?;

    if headers.is_empty() {
        bail!("empty file: {path}");
    }

    let first = &headers[0];
    if !first.is_version() {
        bail!("not an e2store file: first entry is not a version entry");
    }

    let data_headers = &headers[1..];
    let total_entries = data_headers.len();

    let entries = count_entry_types(data_headers);

    let format_str = if data_headers.is_empty() {
        "Unknown".to_string()
    } else {
        let rr = EraRandomReader::new(open_file(path)?)
            .with_context(|| format!("failed to open {path} for random access"))?;
        match rr.format() {
            Some(f) => f.to_string(),
            None => "Unknown".to_string(),
        }
    };

    // Use random reader for accurate slot range and block count
    let rr = EraRandomReader::new(open_file(path)?)
        .with_context(|| format!("failed to open {path} for random access"))?;

    let starting_slot = rr.starting_slot().unwrap_or(0);
    let slot_count = rr.slot_count().unwrap_or(0);
    let slot_end = starting_slot + slot_count.saturating_sub(1) as u64;

    // Block count = non-zero offsets in the index
    let block_count = rr
        .block_index()
        .map(|idx| idx.offsets.iter().filter(|&&o| o != 0).count())
        .unwrap_or(0);

    // State present = state index entry exists
    let state_present = data_headers.iter().any(|h| h.typ == TYPE_STATE_INDEX);

    let result = InfoResult {
        format: format_str,
        file_size,
        file_size_human: human_size(file_size),
        starting_slot,
        slot_range: [starting_slot, slot_end],
        block_count,
        state_present,
        total_entries,
        entries,
    };

    output(&result, json);
    Ok(())
}
