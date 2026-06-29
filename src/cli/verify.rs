use std::path::Path;

use anyhow::{Context, bail};
use brandon::verify::hash::verify_file_against_manifest;
use serde::Serialize;

use crate::cli::{directory::Input, output};

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

#[derive(Serialize)]
pub struct DirVerifyResult {
    pub path: String,
    pub checked: usize,
    pub passed: usize,
    pub failed: usize,
    pub failed_files: Vec<String>,
    pub total_size: u64,
    pub total_size_human: String,
}

impl std::fmt::Display for DirVerifyResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "Checked {} files ({} total)",
            self.checked, self.total_size_human
        )?;
        if self.failed == 0 {
            writeln!(f, "✓ All {} files passed verification", self.passed)?;
        } else {
            writeln!(f, "✗ {} files FAILED", self.failed)?;
            for file in &self.failed_files {
                writeln!(f, "  - {file}")?;
            }
        }
        Ok(())
    }
}

pub fn run(path: &str, manifest_path: Option<&str>, json: bool) -> anyhow::Result<()> {
    let input = Input::resolve(path)?;

    match input {
        Input::File(_) => run_single(path, manifest_path, json),
        Input::Dir(dir) => {
            if manifest_path.is_some() {
                bail!("--manifest cannot be used with a directory");
            }
            run_directory(&dir, json)
        }
    }
}

pub fn run_single(path: &str, manifest_path: Option<&str>, json: bool) -> anyhow::Result<()> {
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
        bail!("verification failed");
    }

    Ok(())
}

fn run_directory(dir: &super::directory::ArchiveDirectory, json: bool) -> anyhow::Result<()> {
    let mut checked = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut failed_files = Vec::new();
    let mut total_size = 0u64;

    for (_, path) in &dir.files {
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => {
                failed += 1;
                failed_files.push(path.file_name().unwrap().to_str().unwrap().to_string());
                continue;
            }
        };
        total_size += metadata.len();
        checked += 1;

        match std::fs::read(path) {
            Ok(data) => {
                let vr = brandon::verify::verify_era(&data[..]);
                if vr.valid {
                    passed += 1;
                } else {
                    failed += 1;
                    failed_files.push(path.file_name().unwrap().to_str().unwrap().to_string());
                }
            }
            Err(_) => {
                failed += 1;
                failed_files.push(path.file_name().unwrap().to_str().unwrap().to_string());
            }
        }
    }

    let result = DirVerifyResult {
        path: dir.path.to_str().unwrap().to_string(),
        checked,
        passed,
        failed,
        failed_files,
        total_size,
        total_size_human: super::human_size(total_size),
    };

    output(&result, json);

    if failed > 0 {
        bail!("directory verification failed");
    }

    Ok(())
}
