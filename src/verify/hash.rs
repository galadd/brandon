use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use crate::error::{E2StoreError, Error};

/// Compute the SHA256 hash of a file.
pub fn sha256_file(path: &Path) -> Result<[u8; 32], Error> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 8192];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

pub fn parse_manifest(path: &Path) -> Result<HashMap<String, [u8; 32]>, Error> {
    let content = fs::read_to_string(path)?;
    let mut map = HashMap::new();

    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let hash_str = parts.next().ok_or_else(|| {
            E2StoreError::InvalidManifest(format!("line {}: missing hash", line_no + 1))
        })?;
        let filename = parts.next().ok_or_else(|| {
            E2StoreError::InvalidManifest(format!("line {}: missing filename", line_no + 1))
        })?;

        if hash_str.len() != 64 {
            return Err(E2StoreError::InvalidManifest(format!(
                "line {}: invalid SHA256 length",
                line_no + 1
            ))
            .into());
        }

        let mut hash_bytes = [0u8; 32];
        hex::decode_to_slice(hash_str, &mut hash_bytes).map_err(|_| {
            E2StoreError::InvalidManifest(format!("line {}: invalid hex hash", line_no + 1))
        })?;

        map.insert(filename.to_string(), hash_bytes);
    }

    Ok(map)
}

pub fn verify_file_against_manifest(file: &Path, manifest: &Path) -> Result<bool, Error> {
    let manifest_map = parse_manifest(manifest)?;
    let file_name = file
        .file_name()
        .ok_or_else(|| E2StoreError::InvalidManifest("invalid file name".into()))?
        .to_string_lossy();

    let expected_hash = manifest_map.get(file_name.as_ref()).ok_or_else(|| {
        E2StoreError::InvalidManifest(format!("file {file_name} not in manifest"))
    })?;

    let actual_hash = sha256_file(file)?;

    Ok(&actual_hash == expected_hash)
}

// #[test]
// fn verify_mainnet_file_against_manifest() {
//     use crate::verify::hash::verify_file_against_manifest;
//     use std::path::Path;
//
//     let manifest = Path::new("checksums.txt");
//     let file = Path::new("mainnet-00000-5ec1ffb8.era1");
//
//     let ok = verify_file_against_manifest(file, manifest).expect("manifest parsing failed");
//
//     assert!(ok, "file hash does not match manifest");
// }
