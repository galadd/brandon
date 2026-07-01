use std::{collections::BTreeMap, fs};

use anyhow::Context;
use brandon::{EraRandomReader, format::e2store::E2StoreReader};
use serde::Serialize;

use crate::cli::directory::Input;

use super::{count_entry_types, human_size, output};

#[derive(Serialize)]
pub struct InfoResult {
    pub path: String,
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
            writeln!(f, "  {name:<max_name$} {count}")?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub struct DirInfoResult {
    pub path: String,
    pub file_count: usize,
    pub total_size: u64,
    pub total_size_human: String,
    pub total_blocks: usize,
    pub slot_range: [u64; 2],
    pub states_present: usize,
    pub files: Vec<InfoResult>,
}

impl std::fmt::Display for DirInfoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Directory:  {}", self.path)?;
        writeln!(f, "Files:      {}", self.file_count)?;
        writeln!(
            f,
            "Total size: {} ({})",
            self.total_size_human, self.total_size
        )?;
        writeln!(f, "Blocks:     {}", self.total_blocks)?;
        writeln!(
            f,
            "Slot range: {}..{}",
            self.slot_range[0], self.slot_range[1]
        )?;
        writeln!(f, "States:     {}", self.states_present)?;
        Ok(())
    }
}

pub fn run(path: &str, json: bool) -> anyhow::Result<()> {
    let input = Input::resolve(path)?;

    match input {
        Input::File(p) => run_single(p.to_str().unwrap(), json),
        Input::Dir(dir) => run_directory(&dir, json),
    }
}

fn run_single(path: &str, json: bool) -> anyhow::Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("cannot stat {path}"))?;
    let file_size = metadata.len();

    let file = super::open_file(path)?;
    let mut reader = E2StoreReader::new(file);

    let headers = reader
        .scan_all_headers()
        .with_context(|| format!("failed to scan headers in {path}"))?;

    if headers.is_empty() {
        anyhow::bail!("empty e2store file: {path}");
    }

    let data_headers = &headers[1..];
    let total_entries = data_headers.len();
    let entries = count_entry_types(data_headers);

    let rr = EraRandomReader::new(super::open_file(path)?)?;
    let starting_slot = rr.starting_slot().unwrap_or(0);
    let slot_count = rr.slot_count().unwrap_or(0);
    let slot_end = starting_slot + slot_count.saturating_sub(1) as u64;

    let block_count = rr
        .block_index()
        .map(|idx| idx.offsets.iter().filter(|&&o| o != 0).count())
        .unwrap_or(0);

    let state_present = data_headers
        .iter()
        .any(|h| h.typ == brandon::format::types::TYPE_COMPRESSED_BEACON_STATE);

    let result = InfoResult {
        path: path.to_string(),
        format: rr.format().map(|f| f.to_string()).unwrap(),
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

fn run_directory(dir: &super::directory::ArchiveDirectory, json: bool) -> anyhow::Result<()> {
    let mut total_size = 0u64;
    let mut total_blocks = 0usize;
    let mut min_slot = u64::MAX;
    let mut max_slot = 0u64;
    let mut states_present = 0usize;
    let mut file_results = Vec::new();

    for (_, path) in &dir.files {
        let path_str = path.to_str().unwrap();
        let metadata = fs::metadata(path)?;
        total_size += metadata.len();

        let file = match super::open_file(path_str) {
            Ok(f) => f,
            Err(_) => continue, // skip unreadable files
        };

        let mut reader = E2StoreReader::new(file);
        let headers = match reader.scan_all_headers() {
            Ok(h) => h,
            Err(_) => continue,
        };

        if headers.is_empty() {
            continue;
        }

        let rr = match EraRandomReader::new(super::open_file(path_str)?) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let start = rr.starting_slot().unwrap_or(0);
        let count = rr.slot_count().unwrap_or(0);
        let end = start + count.saturating_sub(1) as u64;

        if start < min_slot {
            min_slot = start;
        }
        if end > max_slot {
            max_slot = end;
        }

        let blocks = rr
            .block_index()
            .map(|idx| idx.offsets.iter().filter(|&&o| o != 0).count())
            .unwrap_or(0);
        total_blocks += blocks;

        let has_state = headers[1..]
            .iter()
            .any(|h| h.typ == brandon::format::types::TYPE_COMPRESSED_BEACON_STATE);
        if has_state {
            states_present += 1;
        }

        file_results.push(InfoResult {
            path: path.file_name().unwrap().to_str().unwrap().to_string(),
            format: rr.format().map(|f| f.to_string()).unwrap(),
            file_size: metadata.len(),
            file_size_human: human_size(metadata.len()),
            starting_slot: start,
            slot_range: [start, end],
            block_count: blocks,
            state_present: has_state,
            total_entries: headers.len() - 1,
            entries: count_entry_types(&headers[1..]),
        });
    }

    if min_slot == u64::MAX {
        min_slot = 0;
    }

    let result = DirInfoResult {
        path: dir.path.to_str().unwrap().to_string(),
        file_count: file_results.len(),
        total_size,
        total_size_human: human_size(total_size),
        total_blocks,
        slot_range: [min_slot, max_slot],
        states_present,
        files: file_results,
    };

    output(&result, json);
    Ok(())
}
