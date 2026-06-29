use std::{
    fs::{self, File},
    io::{BufWriter, Write},
};

use anyhow::{Context, bail};
use brandon::{Block, EraBlockReader, EraRandomReader};
use serde::Serialize;

use crate::cli::directory::Input;

use super::{human_size, open_file, output};

#[derive(Serialize)]
pub struct BlockSummary {
    pub slot: u64,
    pub format: String,
    pub size: usize,
    pub size_human: String,
    pub hash_prefix: String,
}

impl std::fmt::Display for BlockSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "slot={:<8} {:>4}  {}  {}",
            self.slot, self.format, self.size_human, self.hash_prefix
        )
    }
}

#[derive(Serialize)]
pub struct ReadResult {
    pub blocks: Vec<BlockSummary>,
    pub state: Option<StateSummary>,
}

#[derive(Serialize)]
pub struct StateSummary {
    pub size: usize,
    pub size_human: String,
    pub hash_prefix: String,
}

impl std::fmt::Display for ReadResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in &self.blocks {
            writeln!(f, "{b}")?;
        }
        if let Some(ref s) = self.state {
            writeln!(f, "state          {}  {}", s.size_human, s.hash_prefix)?;
        }
        Ok(())
    }
}

fn sha256_prefix(data: &[u8], len: usize) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hex::encode(&hash[..len])
}

fn make_block_summary(slot: u64, block: &Block) -> BlockSummary {
    let (format_str, data) = match block {
        Block::Era(b) => ("ERA", b.block.as_slice()),
        Block::Era1(b) => ("ERA1", b.header.as_slice()),
    };
    BlockSummary {
        slot,
        format: format_str.to_string(),
        size: data.len(),
        size_human: human_size(data.len() as u64),
        hash_prefix: format!("0x{}", sha256_prefix(data, 8)),
    }
}

fn write_block_raw(w: &mut dyn Write, block: &Block) -> anyhow::Result<()> {
    match block {
        Block::Era(b) => w.write_all(&b.block)?,
        Block::Era1(b) => w.write_all(&b.header)?,
    }
    Ok(())
}

fn write_full_era1_raw(w: &mut dyn Write, block: &brandon::read::Era1Block) -> anyhow::Result<()> {
    w.write_all(&block.header)?;
    w.write_all(&block.body)?;
    w.write_all(&block.receipts)?;
    w.write_all(&block.total_difficulty.0)?;
    Ok(())
}

#[derive(clap::Args)]
pub struct ReadArgs {
    /// Path to an ERA, ERA1 file, or directory.
    pub path: String,

    #[arg(long, conflicts_with = "all")]
    pub slot: Option<u64>,

    #[arg(long, conflicts_with = "slot")]
    pub all: bool,

    #[arg(long, conflicts_with = "slot")]
    pub count: Option<usize>,

    #[arg(long)]
    pub state: bool,

    #[arg(long, default_value = "hex", value_parser = ["hex", "raw", "json"])]
    pub format: String,

    #[arg(short, long)]
    pub output: Option<String>,

    #[arg(long, conflicts_with = "slot")]
    pub output_dir: Option<String>,
}

pub fn run(args: ReadArgs) -> anyhow::Result<()> {
    let input = Input::resolve(args.path.as_str())?;

    match input {
        Input::File(_) => run_single_file(args),
        Input::Dir(dir) => {
            if let Some(slot) = args.slot {
                run_dir_slot(&dir, slot, args.state, &args.format, args.output.as_deref())
            } else if args.all || args.count.is_some() {
                run_dir_stream(
                    &dir,
                    args.count,
                    args.state,
                    &args.format,
                    args.output_dir.as_deref(),
                )
            } else {
                bail!("specify --slot, --all, or --count for directory input")
            }
        }
    }
}

pub fn run_single_file(args: ReadArgs) -> anyhow::Result<()> {
    if let Some(slot) = args.slot {
        return run_slot(
            &args.path,
            slot,
            args.state,
            &args.format,
            args.output.as_deref(),
        );
    }

    let limit = if args.all { None } else { args.count };

    run_stream(
        &args.path,
        limit,
        args.state,
        &args.format,
        args.output.as_deref(),
        args.output_dir.as_deref(),
    )
}

/// Read a single block by slot using random access.
fn run_slot(
    path: &str,
    slot: u64,
    read_state: bool,
    format: &str,
    output_file: Option<&str>,
) -> anyhow::Result<()> {
    let file = open_file(path)?;
    let mut rr = EraRandomReader::new(file).with_context(|| format!("failed to open {path}"))?;

    let block = match rr.read_block_at_slot(slot)? {
        Some(b) => b,
        None => bail!("slot {slot} is empty (skipped slot)"),
    };

    // For ERA1, try to get the full block
    let file = open_file(path)?;
    let mut rr = EraRandomReader::new(file)?;
    let full_block = if matches!(block, Block::Era1(_)) {
        rr.read_full_era1_block_at_slot(slot)?.map(Block::Era1)
    } else {
        Some(block.clone())
    };

    let effective_block = full_block.as_ref().unwrap_or(&block);

    match format {
        "hex" => {
            let summary = make_block_summary(slot, effective_block);
            output(&summary, false);
        }
        "json" => {
            let summary = make_block_summary(slot, effective_block);
            output(&summary, true);
        }
        "raw" => {
            let mut w: Box<dyn Write> = match output_file {
                Some(p) => Box::new(BufWriter::new(
                    File::create(p).with_context(|| format!("cannot create {p}"))?,
                )),
                None => Box::new(std::io::stdout()),
            };
            if let Some(Block::Era1(b)) = &full_block {
                write_full_era1_raw(&mut w, b)?;
            } else {
                write_block_raw(&mut w, effective_block)?;
            }
        }
        _ => bail!("unknown format: {format}"),
    }

    if read_state {
        let file = open_file(path)?;
        let mut rr = EraRandomReader::new(file)?;
        match rr.read_state()? {
            Some(data) => match format {
                "raw" => {
                    let mut w: Box<dyn Write> = match output_file {
                        Some(p) => Box::new(BufWriter::new(
                            File::create(p).with_context(|| format!("cannot create {p}"))?,
                        )),
                        None => Box::new(std::io::stdout()),
                    };
                    w.write_all(&data)?;
                }
                _ => {
                    if !matches!(format, "json") {
                        eprintln!(
                            "state: {}  0x{}",
                            human_size(data.len() as u64),
                            sha256_prefix(&data, 8)
                        );
                    }
                }
            },
            None => eprintln!("no beacon state found in file"),
        }
    }

    Ok(())
}

/// Stream blocks sequentially.
fn run_stream(
    path: &str,
    limit: Option<usize>,
    read_state: bool,
    format: &str,
    output_file: Option<&str>,
    output_dir: Option<&str>,
) -> anyhow::Result<()> {
    let file = open_file(path)?;

    // Detect starting slot for slot numbering
    let slot_start = {
        let f = open_file(path)?;
        let rr = EraRandomReader::new(f)?;
        rr.starting_slot().unwrap_or(0)
    };

    let mut reader = EraBlockReader::new(file);
    let mut blocks: Vec<BlockSummary> = Vec::new();
    let mut slot = slot_start;
    let mut state_data: Option<Vec<u8>> = None;

    if let Some(dir) = output_dir {
        fs::create_dir_all(dir).with_context(|| format!("cannot create directory {dir}"))?;
    }

    while let Some(block) = reader.next_block()? {
        if let Some(limit) = limit
            && blocks.len() >= limit
        {
            break;
        }

        match format {
            "hex" | "json" => {
                blocks.push(make_block_summary(slot, &block));
            }
            "raw" => {
                if let Some(dir) = output_dir {
                    let p = format!("{dir}/{slot}.raw");
                    let mut f = BufWriter::new(
                        File::create(&p).with_context(|| format!("cannot create {p}"))?,
                    );
                    write_block_raw(&mut f, &block)?;
                } else if let Some(p) = output_file {
                    // Append mode for single output file
                    let mut f = BufWriter::new(
                        fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(p)
                            .with_context(|| format!("cannot open {p}"))?,
                    );
                    write_block_raw(&mut f, &block)?;
                } else {
                    write_block_raw(&mut std::io::stdout(), &block)?;
                }
            }
            _ => bail!("unknown format: {format}"),
        }

        slot += 1;
    }

    if read_state {
        let file = open_file(path)?;
        let mut rr = EraRandomReader::new(file)?;
        state_data = rr.read_state()?;
    }

    let result = ReadResult {
        blocks,
        state: state_data.clone().map(|d| StateSummary {
            size: d.len(),
            size_human: human_size(d.len() as u64),
            hash_prefix: format!("0x{}", sha256_prefix(&d, 8)),
        }),
    };

    match format {
        "hex" => output(&result, false),
        "json" => output(&result, true),

        "raw" => {
            if let (Some(dir), Some(ref data)) = (output_dir, state_data) {
                let p = format!("{dir}/state.raw");
                let mut f =
                    BufWriter::new(File::create(&p).with_context(|| format!("cannot create {p}"))?);
                f.write_all(data)?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn run_dir_slot(
    dir: &super::directory::ArchiveDirectory,
    slot: u64,
    state: bool,
    format: &str,
    output_file: Option<&str>,
) -> anyhow::Result<()> {
    let file_path = dir
        .find_file_for_slot(slot)
        .ok_or_else(|| anyhow::anyhow!("slot {slot} not found in directory"))?;

    // Delegate to the single-file slot logic
    run_single_file(ReadArgs {
        path: file_path.to_string_lossy().into_owned(),
        slot: Some(slot),
        all: false,
        count: None,
        state,
        format: format.to_owned(),
        output: output_file.map(str::to_owned),
        output_dir: None,
    })
}

fn run_dir_stream(
    dir: &super::directory::ArchiveDirectory,
    count: Option<usize>,
    state: bool,
    format: &str,
    output_dir: Option<&str>,
) -> anyhow::Result<()> {
    match count {
        Some(limit) => {
            for (_, path) in &dir.files {
                run_single_file(ReadArgs {
                    path: path.to_string_lossy().into_owned(),
                    slot: None,
                    all: false,
                    count: Some(limit),
                    state,
                    format: format.to_owned(),
                    output: None,
                    output_dir: output_dir.map(str::to_owned),
                })?;
                break;
            }
        }
        None => {
            for (_, path) in &dir.files {
                run_single_file(ReadArgs {
                    path: path.to_string_lossy().into_owned(),
                    slot: None,
                    all: true,
                    count: None,
                    state,
                    format: format.to_owned(),
                    output: None,
                    output_dir: output_dir.map(str::to_owned),
                })?;
            }
        }
    }
    Ok(())
}
