#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("e2store parse error: {0}")]
    E2Store(#[from] E2StoreError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum E2StoreError {
    #[error("truncated header: expected 8 bytes, got {0}")]
    TruncatedHeader(usize),
    #[error("non-zero reserved field: {0}")]
    NonZeroReserved(u16),
    #[error("unexpected entry type: expected {expected:?}, got {actual:?}")]
    UnexpectedType { expected: [u8; 2], actual: [u8; 2] },
    #[error("missing version entry as first entry")]
    MissingVersion,
    #[error("entry data length exceeds remaining file: declared {declared}, available {available}")]
    OverlongEntry { declared: u64, available: u64 },
    #[error("invalid era file: {0}")]
    InvalidEra(String),
    #[error("invalid era1 file: {0}")]
    InvalidEra1(String),
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
}
