//! ERA file naming convention.
//!
//! Format: `{network}-{era_number:05}-{root_prefix}.era`
//!
//! Where `root_prefix` is the first 4 bytes (8 hex chars) of the era's
//! historical root (or genesis_validators_root for era 0), lower-case hex.

/// Construct an ERA filename.
pub fn era_filename(network: &str, era_number: u64, root_prefix: &[u8; 4]) -> String {
    format!(
        "{}-{:05}-{:02x}{:02x}{:02x}{:02x}.era",
        network, era_number, root_prefix[0], root_prefix[1], root_prefix[2], root_prefix[3]
    )
}

/// Parse an ERA filename, returning (network, era_number, root_hex).
pub fn parse_era_filename(name: &str) -> Option<(String, u64, String)> {
    let name = name.strip_suffix(".era")?;
    let mut parts = name.rsplitn(3, '-');
    let root_hex = parts.next()?;
    let era_str = parts.next()?;
    let network = parts.next()?.to_string();
    let era_number = era_str.parse().ok()?;
    Some((network, era_number, root_hex.to_string()))
}
