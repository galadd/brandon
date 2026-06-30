//! Central registry for all e2store type codes.
//!
//! Based on the official e2store format specifications.
//! https://github.com/eth-clients/e2store-format-specs

// ── Generic e2store ──────────────────────────────────────────────

/// Version entry — must be the first entry in every e2store file.
pub const TYPE_VERSION: [u8; 2] = [0x65, 0x32];

/// Empty type — padding or alignment. May have a length > 0; data should be skipped.
pub const TYPE_EMPTY: [u8; 2] = [0x00, 0x00];

// ── Indexes ──────────────────────────────────────────────────────

/// Block index (Era1, E2HS). Fixed array of offsets.
pub const TYPE_BLOCK_INDEX: [u8; 2] = [0x66, 0x32];

/// Dynamic block index (Ere). Variable-length structure for genesis-to-head.
pub const TYPE_DYNAMIC_BLOCK_INDEX: [u8; 2] = [0x67, 0x32];

/// Generic slot index (Era, sometimes Era1). Distinguished by position.
pub const TYPE_SLOT_INDEX: [u8; 2] = [0x69, 0x32];

// ── Era (Beacon Chain History) ───────────────────────────────────

/// Snappy-compressed `SignedBeaconBlock`.
pub const TYPE_COMPRESSED_SIGNED_BEACON_BLOCK: [u8; 2] = [0x01, 0x00];

/// Snappy-compressed `BeaconState`.
pub const TYPE_COMPRESSED_BEACON_STATE: [u8; 2] = [0x02, 0x00];

// ── Era1 / E2HS (Execution Layer History) ───────────────────────

/// Snappy-compressed RLP block header.
pub const TYPE_COMPRESSED_HEADER: [u8; 2] = [0x03, 0x00];

/// Snappy-compressed RLP block header with canonical proof (E2HS).
pub const TYPE_COMPRESSED_HEADER_WITH_PROOF: [u8; 2] = [0x03, 0x01];

/// RLP-encoded block body.
pub const TYPE_COMPRESSED_BODY: [u8; 2] = [0x04, 0x00];

/// RLP-encoded receipts.
pub const TYPE_COMPRESSED_RECEIPTS: [u8; 2] = [0x05, 0x00];

/// SSZ-encoded total difficulty.
pub const TYPE_TOTAL_DIFFICULTY: [u8; 2] = [0x06, 0x00];

/// SSZ hash tree root of `List[HeaderRecord, 8192]`.
pub const TYPE_ACCUMULATOR: [u8; 2] = [0x07, 0x00];

// ── E2SS (Execution State Snapshots) ────────────────────────────

/// Snappy-compressed SSZ account.
pub const TYPE_COMPRESSED_ACCOUNT: [u8; 2] = [0x08, 0x00];

/// Snappy-compressed SSZ storage.
pub const TYPE_COMPRESSED_STORAGE: [u8; 2] = [0x09, 0x00];

// ── Ere (Full Execution History) ────────────────────────────────

/// Snappy-compressed slim receipts.
pub const TYPE_COMPRESSED_SLIM_RECEIPTS: [u8; 2] = [0x0a, 0x00];

/// Proof data.
pub const TYPE_PROOF: [u8; 2] = [0x0b, 0x00];

/// Map a 2-byte entry type to its official spec name.
pub fn entry_type_name(typ: &[u8; 2]) -> &'static str {
    match *typ {
        TYPE_VERSION => "Version",
        TYPE_EMPTY => "Empty",

        TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => "CompressedSignedBeaconBlock",
        TYPE_COMPRESSED_BEACON_STATE => "CompressedBeaconState",

        TYPE_COMPRESSED_HEADER => "CompressedHeader",
        TYPE_COMPRESSED_HEADER_WITH_PROOF => "CompressedHeaderWithProof",
        TYPE_COMPRESSED_BODY => "CompressedBody",
        TYPE_COMPRESSED_RECEIPTS => "CompressedReceipts",
        TYPE_TOTAL_DIFFICULTY => "TotalDifficulty",
        TYPE_ACCUMULATOR => "Accumulator",

        TYPE_COMPRESSED_ACCOUNT => "CompressedAccount",
        TYPE_COMPRESSED_STORAGE => "CompressedStorage",

        TYPE_COMPRESSED_SLIM_RECEIPTS => "CompressedSlimReceipts",
        TYPE_PROOF => "Proof",

        TYPE_BLOCK_INDEX => "BlockIndex",
        TYPE_DYNAMIC_BLOCK_INDEX => "DynamicBlockIndex",
        TYPE_SLOT_INDEX => "SlotIndex",

        _ => "Unknown",
    }
}
