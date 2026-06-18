use std::path::Path;

use anyhow::{Context, bail};
use brandon::verify::hash::verify_file_against_manifest;
use serde::Serialize;

use crate::cli::output;

#[derive(Serialize)]
pub struct VerifyResult {
    pub valid: bool,
    pub block_count: usize,
    pub state_present: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub manifest_match: Option<bool>,
}

impl std::fmt::Display for VerifyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.valid {
            writeln!(f, "✓ valid")?;
        } else {
            writeln!(f, "✗ invalid")?;
        }
        writeln!(f, "  blocks:   {}", self.block_count)?;
        writeln!(
            f,
            "  state:    {}",
            if self.state_present {
                "present"
            } else {
                "absent"
            }
        )?;

        if let Some(matched) = self.manifest_match {
            if matched {
                writeln!(f, "  manifest: matches")?;
            } else {
                writeln!(f, "  manifest: MISMATCH")?;
            }
        }

        if !self.errors.is_empty() {
            writeln!(f)?;
            writeln!(f, "Errors:")?;
            for e in &self.errors {
                writeln!(f, "  - {e}")?;
            }
        }

        if !self.warnings.is_empty() {
            writeln!(f)?;
            writeln!(f, "Warnings:")?;
            for w in &self.warnings {
                writeln!(f, "  - {w}")?;
            }
        }

        Ok(())
    }
}

pub fn run(path: &str, manifest_path: Option<&str>, json: bool) -> anyhow::Result<()> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {path}"))?;

    let vr = brandon::verify::verify_era(&data[..]);

    let mut manifest_match = None;

    if let Some(mp) = manifest_path {
        let file = Path::new(path);
        let manifest = Path::new(mp);
        let matched = verify_file_against_manifest(file, manifest)
            .with_context(|| format!("failed to verify against manifest {mp}"))?;
        manifest_match = Some(matched);
    }

    let valid = vr.valid && manifest_match.unwrap_or(true);

    let result = VerifyResult {
        valid,
        block_count: vr.block_count,
        state_present: vr.state_present,
        errors: vr.errors,
        warnings: vr.warnings,
        manifest_match,
    };

    output(&result, json);

    if !valid {
        bail!("verifiction failed");
    }

    Ok(())
}
