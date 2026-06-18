//! Command-line interface for the Brandon ERA toolkit.

mod build;
mod info;
mod read;
mod verify;

use std::{collections::BTreeMap, fs::File, io::BufReader};

use anyhow::Context;
use brandon::format::{
    e2store::TYPE_VERSION,
    era::{
        TYPE_BLOCK_INDEX, TYPE_COMPRESSED_BEACON_STATE, TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
        TYPE_STATE_INDEX,
    },
    era1::{
        TYPE_BLOCK_ACCUMULATOR, TYPE_BLOCK_BODY, TYPE_COMPRESSED_HEADER, TYPE_RECEIPTS,
        TYPE_TOTAL_DIFFICULTY,
    },
};
use clap::{Parser, Subcommand};

/// Standalone toolkit for Ethereum ERA/ERA1 archive files.
#[derive(Parser)]
#[command(name = "brandon", version, about)]
#[command(propagate_version = true)]
pub struct Args {
    /// Output results as JSON.
    #[arg(short, long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Show file format, block count, slot range, and entry breakdown.
    Info {
        /// Path to an ERA or ERA1 file.
        file: String,
    },
    /// Verify structural integrity and optional manifest hash.
    Verify {
        /// Path to an ERA or ERA1 file.
        file: String,
        /// Path to a manifest file (SHA256 checksums) for hash verification.
        #[arg(long)]
        manifest: Option<String>,
    },
    /// Read blocks from an ERA/ERA1 file.
    Read {
        /// Path to an ERA or ERA1 file.
        file: String,
        /// Read block at specific slot number.
        #[arg(long, conflicts_with = "all")]
        slot: Option<u64>,
        /// Read all blocks (streams from start to end).
        #[arg(long, conflicts_with = "slot")]
        all: bool,
        /// Limit output to the first N blocks.
        #[arg(long, conflicts_with = "slot")]
        count: Option<usize>,
        /// Also read and output the beacon state (ERA files only).
        #[arg(long)]
        state: bool,
        /// Output format: hex (summary), raw (bytes), json.
        #[arg(long, default_value = "hex", value_parser = ["hex", "raw","json"])]
        format: String,
        /// Write output to a file instead of stdout.
        #[arg(short, long)]
        output: Option<String>,
        /// Write each block to a separate file in this directory.
        /// Files are named `{slot}.raw`. State is written as `state.raw`.
        #[arg(long, conflicts_with = "slot")]
        output_dir: Option<String>,
    },
    /// Build an ERA file from compressed block data.
    Build {
        /// Directory containing `{slot}.snappy` files (compressed block payloads).
        #[arg(long)]
        blocks_dir: String,
        /// Path to a compressed beacon state file.
        #[arg(long)]
        state: Option<String>,
        /// Slot number for the beacon state (required if --state is given)
        #[arg(long, requires = "state")]
        state_slot: Option<u64>,
        /// Output file path.
        #[arg(short, long)]
        output: String,
    },
}

pub fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Info { file } => info::run(&file, args.json),
        Command::Verify { file, manifest } => verify::run(&file, manifest.as_deref(), args.json),
        Command::Read {
            file,
            slot,
            all,
            count,
            state,
            format,
            output,
            output_dir,
        } => read::run(
            &file,
            slot,
            all,
            count,
            state,
            &format,
            output.as_deref(),
            output_dir.as_deref(),
        ),
        Command::Build {
            blocks_dir,
            state,
            state_slot,
            output,
        } => build::run(&blocks_dir, state.as_deref(), state_slot, &output),
    }
}

/// Map a 2-byte entry type to a human-readable name.
pub fn entry_type_name(typ: &[u8; 2]) -> &'static str {
    match *typ {
        TYPE_VERSION => "Version",
        TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => "CompressedSignedBeaconBlock",
        TYPE_COMPRESSED_BEACON_STATE => "CompressedBeaconState",
        TYPE_BLOCK_INDEX => "BlockIndex",
        TYPE_STATE_INDEX => "StateIndex",
        TYPE_COMPRESSED_HEADER => "CompressedHeader",
        TYPE_BLOCK_BODY => "BlockBody",
        TYPE_RECEIPTS => "Receipts",
        TYPE_TOTAL_DIFFICULTY => "TotalDifficulty",
        TYPE_BLOCK_ACCUMULATOR => "BlockAccumulator",
        _ => "Unknown",
    }
}

/// Format a byte count as a human-readable size string.
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes == 0 {
        return "0 B".into();
    }

    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.2} {unit}", unit = UNITS[unit_idx])
    }
}

/// Count occurences of each entry type from a header hist.
pub fn count_entry_types(headers: &[brandon::format::e2store::Header]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for h in headers {
        if h.is_version() {
            continue; // skip version in breakson
        }
        let name = entry_type_name(&h.typ).to_string();
        *counts.entry(name).or_insert(0) += 1;
    }
    counts
}

/// Open a file for buffered reading.
pub fn open_file(path: &str) -> anyhow::Result<BufReader<File>> {
    let file = File::open(path).with_context(|| format!("cannot open {path}"))?;
    Ok(BufReader::new(file))
}

/// Print a value as JSON or human-readable text depending on the flag.
pub fn output<T: serde::Serialize + std::fmt::Display>(value: &T, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else {
        println!("{value}")
    }
}
