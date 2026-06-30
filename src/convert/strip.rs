//! Strip optional data from ERA/ERA1 files to reduce size.
//!
//! Stream entries one at a time - no full-file memory load.
//!
//! Common use cases:
//! - Remove receipts
//! - Remove beacon state (useful if you only need block history)
//! - Remove block accumulator (if not verifying historical roots)

use std::io::{Read, Seek, Write};

use crate::{
    convert::reindex::reindex_filtered,
    error::Error,
    format::types::{
        TYPE_ACCUMULATOR, TYPE_COMPRESSED_BEACON_STATE, TYPE_COMPRESSED_BODY,
        TYPE_COMPRESSED_RECEIPTS, TYPE_TOTAL_DIFFICULTY,
    },
};

#[derive(Debug, Clone, Default)]
pub struct StripConfig {
    pub receipts: bool,
    pub bodies: bool,
    pub total_difficulty: bool,
    pub state: bool,
    pub accumulator: bool,
}

impl StripConfig {
    pub fn receipts_only() -> Self {
        Self {
            receipts: true,
            ..Default::default()
        }
    }

    /// Create a config that strips everything except block headers.
    pub fn keep_headers_only() -> Self {
        Self {
            receipts: true,
            bodies: true,
            total_difficulty: true,
            state: true,
            accumulator: true,
        }
    }

    fn should_keep(&self, typ: &[u8; 2]) -> bool {
        match *typ {
            TYPE_COMPRESSED_RECEIPTS if self.receipts => false,
            TYPE_COMPRESSED_BODY if self.bodies => false,
            TYPE_TOTAL_DIFFICULTY if self.total_difficulty => false,
            TYPE_COMPRESSED_BEACON_STATE if self.state => false,
            TYPE_ACCUMULATOR if self.accumulator => false,
            _ => true,
        }
    }
}

/// Strips configured entry typesfrom an ERA/ERA1 file and rebuilds indexes.
///
/// Streams entries one at a time, skipping those matched by the config.
/// Requires a seekable source to read the original index for slot numbers.
pub fn strip<R, W>(reader: R, writer: W, config: &StripConfig) -> Result<(), Error>
where
    R: Read + Seek,
    W: Write,
{
    reindex_filtered(reader, writer, |typ| config.should_keep(typ))
}
