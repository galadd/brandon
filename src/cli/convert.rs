use std::fs::File;

use anyhow::{Context, bail};
use clap::Subcommand;
use serde::Serialize;

use brandon::convert::{self, strip::StripConfig};

use super::human_size;

#[derive(Subcommand)]
pub enum ConvertCommand {
    /// Rebuild file indexes without modifying block data (fast).
    Reindex {
        file: String,
        #[arg(short, long)]
        output: String,
    },
    /// Remove optional data to reduce file size.
    Strip {
        file: String,
        #[arg(short, long)]
        output: String,
        #[arg(long)]
        strip_receipts: bool,
        #[arg(long)]
        strip_bodies: bool,
        #[arg(long)]
        strip_td: bool,
        #[arg(long)]
        strip_state: bool,
        #[arg(long)]
        strip_accumulator: bool,
    },
    /// Extract blocks to individual {slot}.snappy files.
    Split {
        file: String,
        #[arg(short, long)]
        output_dir: String,
    },
}

#[derive(Serialize)]
struct ReindexResult {
    input_size: u64,
    output_size: u64,
    output_size_human: String,
}

impl std::fmt::Display for ReindexResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Reindexed successfully")?;
        writeln!(
            f,
            "  output: {} ({})",
            self.output_size_human, self.output_size
        )?;
        Ok(())
    }
}

#[derive(Serialize)]
struct StripResult {
    input_size: u64,
    output_size: u64,
    saved: u64,
    saved_percent: f64,
    output_size_human: String,
}

impl std::fmt::Display for StripResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Stripped successfully")?;
        writeln!(
            f,
            "  output: {} ({})",
            self.output_size_human, self.output_size
        )?;
        writeln!(
            f,
            "  saved:  {} ({:.1}%)",
            human_size(self.saved),
            self.saved_percent
        )?;
        Ok(())
    }
}

#[derive(Serialize)]
struct SplitResult {
    blocks_extracted: u64,
    output_dir: String,
}

impl std::fmt::Display for SplitResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Split {} blocks to {}",
            self.blocks_extracted, self.output_dir
        )?;
        Ok(())
    }
}

pub fn run(cmd: ConvertCommand) -> anyhow::Result<()> {
    match cmd {
        ConvertCommand::Reindex { file, output } => run_reindex(&file, &output),
        ConvertCommand::Strip {
            file,
            output,
            strip_receipts,
            strip_bodies,
            strip_td,
            strip_state,
            strip_accumulator,
        } => run_strip(
            &file,
            &output,
            strip_receipts,
            strip_bodies,
            strip_td,
            strip_state,
            strip_accumulator,
        ),
        ConvertCommand::Split { file, output_dir } => run_split(&file, &output_dir),
    }
}

fn run_reindex(path: &str, out_path: &str) -> anyhow::Result<()> {
    let input_size = std::fs::metadata(path)?.len();

    let reader = super::open_file(path)?;
    let writer = File::create(out_path).with_context(|| format!("cannot create {out_path}"))?;

    convert::reindex::reindex(reader, writer)
        .with_context(|| format!("failed to reindex {path}"))?;

    let output_size = std::fs::metadata(out_path)?.len();
    let result = ReindexResult {
        input_size,
        output_size,
        output_size_human: human_size(output_size),
    };

    super::output(&result, false);
    Ok(())
}

fn run_strip(
    path: &str,
    out_path: &str,
    strip_receipts: bool,
    strip_bodies: bool,
    strip_td: bool,
    strip_state: bool,
    strip_accumulator: bool,
) -> anyhow::Result<()> {
    if !strip_receipts && !strip_bodies && !strip_td && !strip_state && !strip_accumulator {
        bail!("no strip options selected (use --strip-receipts, etc.)");
    }

    let input_size = std::fs::metadata(path)?.len();

    let config = StripConfig {
        receipts: strip_receipts,
        bodies: strip_bodies,
        total_difficulty: strip_td,
        state: strip_state,
        accumulator: strip_accumulator,
    };

    let reader = super::open_file(path)?;
    let writer = File::create(out_path).with_context(|| format!("cannot create {out_path}"))?;

    convert::strip::strip(reader, writer, &config)
        .with_context(|| format!("failed to strip {path}"))?;

    let output_size = std::fs::metadata(out_path)?.len();
    let saved = input_size.saturating_sub(output_size);
    let saved_percent = if input_size > 0 {
        (saved as f64 / input_size as f64) * 100.0
    } else {
        0.0
    };

    let result = StripResult {
        input_size,
        output_size,
        saved,
        saved_percent,
        output_size_human: human_size(output_size),
    };

    super::output(&result, false);
    Ok(())
}

fn run_split(path: &str, out_dir: &str) -> anyhow::Result<()> {
    let reader = super::open_file(path)?;

    let count = convert::split::split_blocks(reader, out_dir)
        .with_context(|| format!("failed to split {path}"))?;

    let result = SplitResult {
        blocks_extracted: count,
        output_dir: out_dir.to_string(),
    };

    super::output(&result, false);
    Ok(())
}
