//! ERA file naming conventions.
//!
//! Format: `{network}-{era_number:05}-{root_prefix}.{ext}`
//!
//! Where `root_prefix` is the first 4 bytes (8 hex chars) of the era's
//! historical root, lower-case hex. Extension is `.era` or `.era1`.

/// Supported archive extensions.
pub const ARCHIVE_EXTENSIONS: &[&str] = &["era", "era1"];

/// Construct an ERA filename.
pub fn era_filename(network: &str, era_number: u64, root_prefix: &[u8; 4]) -> String {
    format!(
        "{}-{:05}-{:02x}{:02x}{:02x}{:02x}.era",
        network, era_number, root_prefix[0], root_prefix[1], root_prefix[2], root_prefix[3]
    )
}

/// Parsed archive filename components.
#[derive(Debug, Clone)]
pub struct ParsedArchiveName {
    pub network: String,
    pub era_number: u64,
    pub root_hex: String,
    pub extension: String,
}

impl ParsedArchiveName {
    /// Parse any recognized archive filename (`.era`, `.era1`).
    pub fn parse(name: &str) -> Option<Self> {
        for ext in ARCHIVE_EXTENSIONS {
            if let Some(rest) = name.strip_suffix(&format!(".{ext}")) {
                let mut parts = rest.rsplitn(3, '-');
                let root_hex = parts.next()?.to_string();
                let era_str = parts.next()?;
                let network = parts.next()?.to_string();
                let era_number = era_str.parse().ok()?;

                return Some(Self {
                    network,
                    era_number,
                    root_hex,
                    extension: ext.to_string(),
                });
            }
        }
        None
    }
}

/// Sort a list of filenames by their parsed era number.
/// Filenames that fail to parse are placed at the end.
pub fn sort_archive_filenames(files: &mut [String]) {
    files.sort_by_key(|f| {
        ParsedArchiveName::parse(f)
            .map(|p| p.era_number)
            .unwrap_or(u64::MAX)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_era() {
        let p = ParsedArchiveName::parse("mainnet-00090-56a5eb95.era").unwrap();
        assert_eq!(p.network, "mainnet");
        assert_eq!(p.era_number, 90);
        assert_eq!(p.extension, "era");
    }
}
