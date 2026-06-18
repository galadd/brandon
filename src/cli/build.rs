use std::{
    fs::{self, File},
    io::BufWriter,
};

use anyhow::{Context, bail};
use brandon::write::EraBuilder;

use super::output;

#[derive(serde::Serialize)]
pub struct BuildResult {
    pub block_count: usize,
    pub state_included: bool,
    pub output_size: u64,
    pub output_size_human: String,
}

impl std::fmt::Display for BuildResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Built {} blocks", self.block_count)?;
        writeln!(
            f,
            "State: {}",
            if self.state_included {
                "included"
            } else {
                "not included"
            }
        )?;
        writeln!(
            f,
            "Output: {} ({})",
            self.output_size_human, self.output_size
        )?;
        Ok(())
    }
}

pub fn run(
    blocks_dir: &str,
    state_path: Option<&str>,
    state_slot: Option<u64>,
    output_path: &str,
) -> anyhow::Result<()> {
    let dir =
        fs::read_dir(blocks_dir).with_context(|| format!("cannot read directory {blocks_dir}"))?;

    // Parse slot numbers from filenames: expected format is `{slot}.snappy`
    let mut slot_files: Vec<(u64, String)> = Vec::new();
    for entry in dir {
        let entry = entry.with_context(|| format!("error reading {blocks_dir}"))?;
        let name = entry.file_name().to_string_lossy().to_string();

        if let Some(rest) = name.strip_suffix(".snappy") {
            let slot: u64 = rest
                .parse()
                .with_context(|| format!("cannot parse slot number from filename: {name}"))?;
            slot_files.push((slot, entry.path().to_string_lossy().to_string()));
        }
    }

    if slot_files.is_empty() {
        bail!("no .snappy files found in {blocks_dir}");
    }

    slot_files.sort_by_key(|(slot, _)| *slot);

    // Validate no duplicate slots
    for window in slot_files.windows(2) {
        if window[0].0 == window[1].0 {
            bail!("duplicate slot {} in {blocks_dir}", window[0].0);
        }
    }

    let mut builder = EraBuilder::new();
    for (slot, path) in &slot_files {
        let data = fs::read(path).with_context(|| format!("cannot read {path}"))?;
        builder.add_block(*slot, data);
    }

    if let (Some(sp), Some(ss)) = (state_path, state_slot) {
        let state_data = fs::read(sp).with_context(|| format!("cannot read state file {sp}"))?;
        builder.set_state(ss, state_data);
    } else if state_path.is_some() || state_slot.is_some() {
        bail!("both --state and --state-slot are required together");
    }

    // Build the ERA file
    let out_file =
        File::create(output_path).with_context(|| format!("cannot create {output_path}"))?;
    let mut out_writer = BufWriter::new(out_file);

    builder.build(&mut out_writer)?;
    out_writer
        .into_inner()
        .with_context(|| format!("failed to flush {output_path}"))?;

    let output_size = fs::metadata(output_path)
        .with_context(|| format!("cannot stat {output_path}"))?
        .len();

    let result = BuildResult {
        block_count: slot_files.len(),
        state_included: state_path.is_some(),
        output_size,
        output_size_human: super::human_size(output_size),
    };

    output(&result, false);
    Ok(())
}
