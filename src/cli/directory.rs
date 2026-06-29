//! Directory-aware archive operations.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use brandon::fs::naming::ParsedArchiveName;

/// Represents a resolved input, either a single file or a directory of archives.
pub enum Input {
    File(PathBuf),
    Dir(ArchiveDirectory),
}

impl Input {
    /// Resolve a path string into either a File or Directory input.
    pub fn resolve(path: &str) -> anyhow::Result<Self> {
        let p = Path::new(path);
        if p.is_dir() {
            Ok(Input::Dir(ArchiveDirectory::load(p)?))
        } else if p.is_file() {
            Ok(Input::File(p.to_path_buf()))
        } else {
            bail!("path does not exist: {path}");
        }
    }
}

/// A scanned directory containing sorted archive files.
pub struct ArchiveDirectory {
    pub path: PathBuf,
    /// Sorted list of (era_number, full_path)
    pub files: Vec<(u64, PathBuf)>,
}

impl ArchiveDirectory {
    /// Scan a directory for archive files and sort them by era number.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut files = Vec::new();

        for entry in
            fs::read_dir(path).with_context(|| format!("cannot read directory {path:?}"))?
        {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            if let Some(parsed) = ParsedArchiveName::parse(&name) {
                files.push((parsed.era_number, entry.path()));
            }
        }

        files.sort_by_key(|(era, _)| *era);

        if files.is_empty() {
            bail!("no .era or .era1 files found in {path:?}");
        }

        Ok(Self {
            path: path.to_path_buf(),
            files,
        })
    }

    /// Find which file contains a given slot, assuming standard 8192 slots per era.
    /// Returns the path to the file.
    pub fn find_file_for_slot(&self, slot: u64) -> Option<&Path> {
        let era_num = slot / 8192;
        self.files
            .binary_search_by_key(&era_num, |(num, _)| *num)
            .ok()
            .map(|idx| self.files[idx].1.as_path())
    }
}
