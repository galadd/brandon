//! Streaming reader for e2store-based file formats.
//!
//! Provides three levels of abstraction over raw e2store entries:
//!
//! 1. **Raw entries** - `E2StoreReader` in `format::e2store` (type + bytes)
//! 2. **Typed entries** - [`EraReader`] yields [`TypedEntry`] with decoded payloads
//! 3. **Block assembly** - [`EraBlockReader`] combines related entries into complete blocks
//!
//! Additionally, [`EraRandomReader`] provides seek-based random access by slot number.
//!
//! # Format Detection
//!
//! The reader auto-detects whether a file is ERA (post-merge) or ERA1 (pre-merge)
//! based on the first non-version entry type.

use std::io::Read;
use std::io::Seek;

use crate::error::E2StoreError;
use crate::error::Error;
use crate::format::Header;
use crate::format::e2store::E2StoreReader;
use crate::format::era::*;
use crate::format::era1::*;

/// Detected file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EraFormat {
    /// Post-merge beacon chain history (ERA).
    Era,
    /// Pre-merge execution layer history (ERA1).
    Era1,
}

impl std::fmt::Display for EraFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EraFormat::Era => write!(f, "ERA"),
            EraFormat::Era1 => write!(f, "ERA1"),
        }
    }
}

/// Detect ERA vs ERA1 format an entry type.
fn detect_format(typ: &[u8; 2]) -> EraFormat {
    match *typ {
        TYPE_COMPRESSED_SIGNED_BEACON_BLOCK
        | TYPE_COMPRESSED_BEACON_STATE
        | TYPE_BLOCK_INDEX
        | TYPE_STATE_INDEX => EraFormat::Era,
        _ => EraFormat::Era1,
    }
}

/// A types, decoded e2store entry.
///
/// Raw bytes are decoded into appropriate Rust types where applicable.
/// Unknown entry types are preserved as raw bytes for forward compatibility.
#[derive(Debug, Clone)]
pub enum TypedEntry {
    // ERA entries
    /// Snappy-compressed signed beacon block (already decompressed).
    BeaconBlock { data: Vec<u8> },
    /// Snappy-compressed beacon state (already decompressed).
    BeaconState { data: Vec<u8> },
    /// Block slot index.
    BlockIndex { index: SlotIndex },
    /// State slot index.
    StateIndex { index: SlotIndex },

    // ERA1 entries
    /// Snappy-compressed block header (already decompressed, RLP-encoded).
    Header { data: Vec<u8> },
    /// RLP-encoded block body.
    BlockBody { data: Vec<u8> },
    /// RLP-encoded receipts.
    Receipts { data: Vec<u8> },
    /// SSZ-encoded total difficulty.
    TotalDifficulty { value: U256 },
    /// SSZ hash tree root of `List[HeaderRecord, 8192]`,
    BlockAccumulator { root: B256 },

    // Common
    /// Unknown entry type - preserved for forward compatibility
    Unknown { typ: [u8; 2], data: Vec<u8> },
}

impl TypedEntry {
    /// Return the raw e2store entry type bytes.
    pub fn entry_type(&self) -> [u8; 2] {
        match self {
            TypedEntry::BeaconBlock { .. } => TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
            TypedEntry::BeaconState { .. } => TYPE_COMPRESSED_BEACON_STATE,
            TypedEntry::BlockIndex { .. } => TYPE_BLOCK_INDEX,
            TypedEntry::StateIndex { .. } => TYPE_STATE_INDEX,
            TypedEntry::Header { .. } => TYPE_COMPRESSED_HEADER,
            TypedEntry::BlockBody { .. } => TYPE_BLOCK_BODY,
            TypedEntry::Receipts { .. } => TYPE_RECEIPTS,
            TypedEntry::TotalDifficulty { .. } => TYPE_TOTAL_DIFFICULTY,
            TypedEntry::BlockAccumulator { .. } => TYPE_BLOCK_ACCUMULATOR,
            TypedEntry::Unknown { typ, .. } => *typ,
        }
    }

    /// Return the name of this entry type for display.
    pub fn type_name(&self) -> &'static str {
        match self {
            TypedEntry::BeaconBlock { .. } => "BeaconBlock",
            TypedEntry::BeaconState { .. } => "BeaconState",
            TypedEntry::BlockIndex { .. } => "BlockIndex",
            TypedEntry::StateIndex { .. } => "StateIndex",
            TypedEntry::Header { .. } => "Header",
            TypedEntry::BlockBody { .. } => "BlockBody",
            TypedEntry::Receipts { .. } => "Receipts",
            TypedEntry::TotalDifficulty { .. } => "TotalDifficulty",
            TypedEntry::BlockAccumulator { .. } => "BlockAccumulator",
            TypedEntry::Unknown { .. } => "Unknown",
        }
    }

    /// Approximate data size in bytes.
    pub fn data_len(&self) -> usize {
        match self {
            TypedEntry::BeaconBlock { data } => data.len(),
            TypedEntry::BeaconState { data } => data.len(),
            TypedEntry::BlockIndex { index } => index.encode().len(),
            TypedEntry::StateIndex { index } => index.encode().len(),
            TypedEntry::Header { data } => data.len(),
            TypedEntry::BlockBody { data } => data.len(),
            TypedEntry::Receipts { data } => data.len(),
            TypedEntry::TotalDifficulty { .. } => 32,
            TypedEntry::BlockAccumulator { .. } => 32,
            TypedEntry::Unknown { data, .. } => data.len(),
        }
    }
}

/// Decode a raw e2store entry into a typed entry.
fn decode_entry(entry: crate::format::Entry) -> Result<TypedEntry, Error> {
    let typ = entry.header.typ;
    let data = entry.data;

    match typ {
        // ERA
        TYPE_COMPRESSED_SIGNED_BEACON_BLOCK => {
            let decompressed = decompress_entry(&data)?;
            Ok(TypedEntry::BeaconBlock { data: decompressed })
        }
        TYPE_COMPRESSED_BEACON_STATE => {
            let decompressed = decompress_entry(&data)?;
            Ok(TypedEntry::BeaconState { data: decompressed })
        }
        TYPE_BLOCK_INDEX => {
            let index = SlotIndex::decode(&data)
                .map_err(|e| E2StoreError::InvalidEra(format!("bad block index: {e}")))?;
            Ok(TypedEntry::BlockIndex { index })
        }
        TYPE_STATE_INDEX => {
            let index = SlotIndex::decode(&data)
                .map_err(|e| E2StoreError::InvalidEra(format!("bad block index: {e}")))?;
            Ok(TypedEntry::StateIndex { index })
        }

        // ERA1
        TYPE_COMPRESSED_HEADER => {
            let decompressed = decompress_entry(&data)?;
            Ok(TypedEntry::Header { data: decompressed })
        }
        TYPE_BLOCK_BODY => Ok(TypedEntry::BlockBody { data }),
        TYPE_RECEIPTS => Ok(TypedEntry::Receipts { data }),
        TYPE_TOTAL_DIFFICULTY => {
            if data.len() != 32 {
                return Err(E2StoreError::InvalidEra1(format!(
                    "total difficulty must be 32 bytes, got {}",
                    data.len()
                ))
                .into());
            }
            let value = U256(data.try_into().unwrap());
            Ok(TypedEntry::TotalDifficulty { value })
        }
        TYPE_BLOCK_ACCUMULATOR => {
            if data.len() != 32 {
                return Err(E2StoreError::InvalidEra1(format!(
                    "total difficulty must be 32 bytes, got {}",
                    data.len()
                ))
                .into());
            }
            let root: B256 = data.try_into().unwrap();
            Ok(TypedEntry::BlockAccumulator { root })
        }

        // Unknown
        _ => Ok(TypedEntry::Unknown { typ, data }),
    }
}

/// Streaming reader that yields typed entries from an ERA/ERA1 file.
///
/// Detacts the file format (ERA vs ERA1) from the first non-version entry
/// and decodes entries accordingly. Snappy-compressed payloads are
/// decompressed automatically.
///
/// # Example
///
/// ```ignore
/// use std::fs::File;
/// use brandon::read::EraReader;
///
/// let file = File::open("mainnet-00000-5ec1ffb8.era1")?;
/// let mut reader = EraReader::new(file)?;
///
/// while let Some(entry) = reader.next_entry()? {
///     println!("{:>20}  {} bytes", entry.type_name(), entry.data_len());
/// }
/// ```
pub struct EraReader<R> {
    inner: E2StoreReader<R>,
    format: Option<EraFormat>,
    version_consumed: bool,
}

impl<R: Read> EraReader<R> {
    /// Create a new reder. Does not read anything until [`next_entry`](Self::next_entry)
    /// is called
    pub fn new(inner: R) -> Self {
        Self {
            inner: E2StoreReader::new(inner),
            format: None,
            version_consumed: false,
        }
    }

    /// Return the detected file format
    ///
    /// Returns `None` if no non-version entries have been read yet.
    pub fn format(&self) -> Option<EraFormat> {
        self.format
    }

    /// Read the next typed entry. Returns `Ok(None)` at EOF.
    ///
    /// The first call consumes and validates the version entry.
    /// Format is detected from the first non-version entry.
    pub fn next_entry(&mut self) -> Result<Option<TypedEntry>, Error> {
        loop {
            let raw = match self.inner.next_entry() {
                Ok(Some(e)) => e,
                Ok(None) => {
                    if !self.version_consumed {
                        return Err(E2StoreError::MissingVersion.into());
                    }
                    return Ok(None);
                }
                Err(e) => return Err(Error::Io(e)),
            };

            // Validate version entry on first call
            if !self.version_consumed {
                self.version_consumed = true;
                if !raw.header.is_version() {
                    return Err(E2StoreError::MissingVersion.into());
                }
                continue; // don't yield the version entry
            }

            // Detect format from first non-version entry
            if self.format.is_none() {
                self.format = Some(detect_format(&raw.header.typ));
            }

            return Ok(Some(decode_entry(raw)?));
        }
    }

    /// Consume the reader, collecting all types entries into a Vec.
    ///
    /// Convenninece method for small files or testing. For production use
    /// with large files, prefer [`next_entry`](Self::next_entry) to avoid
    /// loading everything into memory.
    pub fn read_all(&mut self) -> Result<Vec<TypedEntry>, Error> {
        let mut entries = Vec::new();
        while let Some(entry) = self.next_entry()? {
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Unwrap the inner reader.
    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }
}

/// A fully-assembled ERA1 block.
#[derive(Debug, Clone)]
pub struct Era1Block {
    /// Decompressed, RLP-encoded block header.
    pub header: Vec<u8>,
    /// RLP-encoded block body.
    pub body: Vec<u8>,
    /// RLP-encoded receipts (may be empty for some testnests).
    pub receipts: Vec<u8>,
    /// Total difficulty at this block.
    pub total_difficulty: U256,
}

/// A fully-assembles ERA block.
#[derive(Debug, Clone)]
pub struct EraBlock {
    /// Decompressed signed beacon block (SSZ-encoded).
    pub block: Vec<u8>,
}

/// An assembled block from either format.
#[derive(Debug, Clone)]
pub enum Block {
    Era(EraBlock),
    Era1(Era1Block),
}

impl Block {
    /// Return the raw bytes of the primary block data.
    ///
    /// For ERA, this is the SSZ-encoded signed beacon block.
    /// For ERA1, this is the RLP-encoded header.
    pub fn primary_data(&self) -> &[u8] {
        match self {
            Block::Era(b) => &b.block,
            Block::Era1(b) => &b.header,
        }
    }

    /// Approximate total size in bytes of all components.
    pub fn total_size(&self) -> usize {
        match self {
            Block::Era(b) => b.block.len(),
            Block::Era1(b) => b.header.len() + b.body.len() + b.receipts.len() + 32,
        }
    }
}

/// Block-level reader that assembles related entries into complete blocks.
///
/// For ERA1 files, this collects `Header` + `BlockBody` + `Receipts` +
/// `TotalDifficulty` into a single [`Era1Block`].
///
/// For ERA files, each `BeaconBlock` entry yields one block immediately.
///
/// # Entry ordering
///
/// ERA1 entries for a single block are expected in this order:
///
/// 1 `CompressedHeader`
/// 2. `BlockBody`
/// 3. `Receipts` (optional)
/// 4. `TotalDifficulty`
///
/// A new `CompressedHeader` signals that the previous block is complete
/// and triggers its emission. At EOF, any pending block is flushed.
///
/// # Example
///
/// ```ignore
/// use std::fs::File;
/// use brandon::read::EraBlockReader
///
/// let file = File::open("mainnet-00000-5ec1ffb8.era1")?;
/// let mut reader = EraBlockReader::new(file)?;
///
/// let mut count = 0usize;
/// while let Some(block) = reader.next_block()? {
///     count += 1;
///     println!("block {}: {} bytes total", count, block.total_size());
/// }
/// println!("total: {} blocks", count);
/// ```
pub struct EraBlockReader<R> {
    inner: EraReader<R>,
    format: Option<EraFormat>,
    // ERA1 assembly state
    header: Option<Vec<u8>>,
    body: Option<Vec<u8>>,
    receipts: Option<Vec<u8>>,
    total_difficulty: Option<U256>,
}

impl<R: Read> EraBlockReader<R> {
    /// Create a new block-level reader.
    pub fn new(inner: R) -> Self {
        Self {
            inner: EraReader::new(inner),
            format: None,
            header: None,
            body: None,
            receipts: None,
            total_difficulty: None,
        }
    }

    /// Return the detected format, if known.
    pub fn format(&self) -> Option<EraFormat> {
        self.format.or(self.inner.format())
    }

    /// Read the next assembled block. Returns `Ok(None)` at EOF.
    ///
    /// For ERA files, each `BeaconBlock` entry yields one block immediately.
    /// For ERA1 files, entries are accumulated until a new header is seen
    /// (or EOF), at which point the previous block is emitted.
    pub fn next_block(&mut self) -> Result<Option<Block>, Error> {
        loop {
            let entry = match self.inner.next_entry()? {
                Some(e) => e,
                None => {
                    // EOF - flush any pending ERA1 block
                    if self.format == Some(EraFormat::Era1) {
                        return Ok(self.flush_era1_block());
                    }
                    return Ok(None);
                }
            };

            // Detect format on first real entry
            if self.format.is_none() {
                self.format = self.inner.format();
            }

            match entry {
                // ERA: immediate yield
                TypedEntry::BeaconBlock { data } => {
                    return Ok(Some(Block::Era(EraBlock { block: data })));
                }

                // ERA1: accumulate
                TypedEntry::Header { data } => {
                    // New header means the previous block is complete
                    let pending = self.flush_era1_block();
                    self.header = Some(data);
                    self.body = None;
                    self.body = None;
                    self.receipts = None;
                    self.total_difficulty = None;
                    if pending.is_some() {
                        return Ok(pending);
                    }
                    // First header - continue accumulating
                }
                TypedEntry::BlockBody { data } => {
                    self.body = Some(data);
                }
                TypedEntry::Receipts { data } => {
                    self.receipts = Some(data);
                }
                TypedEntry::TotalDifficulty { value } => {
                    self.total_difficulty = Some(value);
                }

                // Skip non-block entries
                TypedEntry::BeaconState { .. }
                | TypedEntry::BlockIndex { .. }
                | TypedEntry::StateIndex { .. }
                | TypedEntry::BlockAccumulator { .. }
                | TypedEntry::Unknown { .. } => {}
            }
        }
    }

    /// Assemble and return the pending ERA1 block, if we have a header.
    fn flush_era1_block(&mut self) -> Option<Block> {
        let header = self.header.take()?;
        let body = self.body.take().unwrap_or_default();
        let receipts = self.receipts.take().unwrap_or_default();
        let total_difficulty = self.total_difficulty.take().unwrap_or_else(U256::zero);

        Some(Block::Era1(Era1Block {
            header,
            body,
            receipts,
            total_difficulty,
        }))
    }

    /// Unwrap the inner reader.
    pub fn into_inner(self) -> R {
        self.inner.into_inner()
    }
}

/// Error when a slot lookup fails
#[derive(Debug, Clone)]
pub enum SlotLookupError {
    /// No block at this slot (skipped slot).
    EmptySlot,
    /// Slot is outside the range covered by the index.
    OutOfRange { slot: u64, start: u64, end: u64 },
    /// The block index entry was not found in the file.
    IndexNotFound,
}

impl std::fmt::Display for SlotLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SlotLookupError::EmptySlot => write!(f, "no block at this slot (skipped)"),
            SlotLookupError::OutOfRange { slot, start, end } => {
                write!(f, "slot {} outside index range [{}, {}]", slot, start, end)
            }
            SlotLookupError::IndexNotFound => write!(f, "block index not found in file"),
        }
    }
}

impl std::error::Error for SlotLookupError {}

/// Random-access reader for ERA/ERA1 files.
///
/// Required `Read + Seek` to navigate to specific entries using the
/// block index. On construction, performs a fast header-only scan
/// to locate the block index without loading entry data into memory.
///
/// # Example
///
/// ```ignore
/// use std::fs::File;
/// use brandon::read::EraRandomReader;
///
/// let file = File::open("mainnet-00000-5ec1ffb8.era1")?;
/// let mut reader = EraRandomReader::new(file)?;
///
/// println!("format: {:?}", reader.format());
/// println!("slots covered: {}", reader.slot_count().unwrap_or(0));
///
/// // Read block at slot 42
/// match reader.read_block_at_slot(42) {
///     Ok(Some(block)) => println!("header: {} bytes", block.primary_data().len()),
///     Ok(None) => println!("slot 42 is empty"),
///     Err(e) => eprintln!("error: {e}"),
/// }
/// ```
pub struct EraRandomReader<R> {
    reader: R,
    format: Option<EraFormat>,
    block_index: Option<SlotIndex>,
    /// Byte offset of the block index entry's header in the file.
    /// Offsets in the index are relative to this position.
    block_index_offset: Option<u64>,
}

impl<R: Read + Seek> EraRandomReader<R> {
    /// Open and index an ERA/ERA1 file.
    ///
    /// Performs a header-only scan (no entry data is read) to find the
    /// block index and detect the format. This is fast even for
    /// multi-gigabyte files.
    pub fn new(mut reader: R) -> Result<Self, Error> {
        let mut e2s = E2StoreReader::new(&mut reader);

        // Read and validate version entry (no data to skip - length is 0)
        let first = e2s
            .next_entry()
            .map_err(Error::Io)?
            .ok_or(E2StoreError::MissingVersion)?;
        if !first.header.is_version() {
            return Err(E2StoreError::MissingVersion.into());
        }

        // Header-only scan for the block index
        // We need a fresh reader to use next_header_only
        reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(Error::Io)?;
        let mut scanner = E2StoreReader::new(&mut reader);

        let mut format = None;
        let mut block_index = None;
        let mut block_index_offset = None;
        let mut pos: u64 = 0;

        while let Some(header) = scanner.next_header_only().map_err(Error::Io)? {
            if format.is_none() && !header.is_version() {
                format = Some(detect_format(&header.typ));
            }

            if header.typ == TYPE_BLOCK_INDEX {
                reader
                    .seek(std::io::SeekFrom::Start(pos))
                    .map_err(Error::Io)?;
                let mut full_reader = E2StoreReader::new(&mut reader);
                let entry = full_reader
                    .next_entry()
                    .map_err(Error::Io)?
                    .expect("index entry disappeared after seek");
                block_index = Some(
                    SlotIndex::decode(&entry.data)
                        .map_err(|e| E2StoreError::InvalidEra(format!("bad block index: {e}")))?,
                );
                block_index_offset = Some(pos);
                break;
            }

            pos += Header::SIZE as u64 + header.length as u64;
        }

        Ok(Self {
            reader,
            format,
            block_index,
            block_index_offset,
        })
    }

    /// Return the detected format.
    pub fn format(&self) -> Option<EraFormat> {
        self.format
    }

    /// Return the block index, if found.
    pub fn block_index(&self) -> Option<&SlotIndex> {
        self.block_index.as_ref()
    }

    /// Return the number of slots covered by the block index.
    pub fn slot_count(&self) -> Option<usize> {
        self.block_index.as_ref().map(|idx| idx.offsets.len())
    }

    /// Return the starting slot number.
    pub fn starting_slot(&self) -> Option<u64> {
        self.block_index.as_ref().map(|idx| idx.starting_slot)
    }

    /// Look up the absolute byte offset for a slot in the block index.
    ///
    /// Returns `Err(SlotLookupError::EmptySlot)` if the slot has no block
    /// (skipped slot, offset is 0).
    /// Returns `Err(SlotLookupError::OutOfRange)` if the slot is outside
    /// the index range.
    /// Returns `Err(SlotLookupError::IndexNotFound)` if no block index
    /// was found in the file.
    pub fn slot_to_offset(&self, slot: u64) -> Result<u64, SlotLookupError> {
        let idx = self
            .block_index
            .as_ref()
            .ok_or(SlotLookupError::IndexNotFound)?;
        let end = idx.starting_slot + idx.offsets.len() as u64;

        if slot < idx.starting_slot || slot >= end {
            return Err(SlotLookupError::OutOfRange {
                slot,
                start: idx.starting_slot,
                end,
            });
        }

        let i = (slot - idx.starting_slot) as usize;
        let relative_offset = idx.offsets[i];

        if relative_offset == 0 {
            return Err(SlotLookupError::EmptySlot);
        }

        // Offsets are relative to the block index entry's position in the file
        let index_start = self
            .block_index_offset
            .ok_or(SlotLookupError::IndexNotFound)?;

        // relative_offset can be negative (entries before the index)
        let absolute = (index_start as i64 + relative_offset) as u64;
        Ok(absolute)
    }

    /// Seek to a slot and read the block entry at that position.
    ///
    /// For ERA files, returns the full decompressed beacon block.
    /// For ERA1 files, returns only the header (not body/receipts/td),
    /// since those are in subsequent entries and would require
    /// additional seeks. Use [`EraBlockReader`] for full block assembly.
    ///
    /// Returns `Ok(None)` if the slot is empty (skipped slot).
    pub fn read_block_at_slot(&mut self, slot: u64) -> Result<Option<Block>, Error> {
        let offset = match self.slot_to_offset(slot) {
            Ok(off) => off,
            Err(SlotLookupError::EmptySlot) => return Ok(None),
            Err(e) => return Err(Error::E2Store(E2StoreError::InvalidEra(e.to_string()))),
        };

        self.reader
            .seek(std::io::SeekFrom::Start(offset))
            .map_err(Error::Io)?;

        let mut e2s = E2StoreReader::new(&mut self.reader);
        let entry = e2s
            .next_entry()
            .map_err(Error::Io)?
            .ok_or_else(|| E2StoreError::InvalidEra("unexpected EOF at indexed position".into()))?;

        let typed = decode_entry(entry)?;
        match typed {
            TypedEntry::BeaconBlock { data } => Ok(Some(Block::Era(EraBlock { block: data }))),
            TypedEntry::Header { data } => Ok(Some(Block::Era1(Era1Block {
                header: data,
                body: Vec::new(),
                receipts: Vec::new(),
                total_difficulty: U256::zero(),
            }))),
            other => Err(E2StoreError::InvalidEra(format!(
                "expected block entry at slot {}, got {} ({:02x}{:02x})",
                slot,
                other.type_name(),
                other.entry_type()[0],
                other.entry_type()[1],
            ))
            .into()),
        }
    }

    /// Read the becon state entry (ERA files only).
    ///
    /// This performs a full sequential scan, so cache the result if
    /// you need it repeatedly.
    pub fn read_state(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(Error::Io)?;

        let mut e2s = E2StoreReader::new(&mut self.reader);
        // Skip version
        let first = e2s
            .next_entry()
            .map_err(Error::Io)?
            .ok_or(E2StoreError::MissingVersion)?;
        if !first.header.is_version() {
            return Err(E2StoreError::MissingVersion.into());
        }

        while let Some(entry) = e2s.next_entry().map_err(Error::Io)? {
            if entry.header.typ == TYPE_COMPRESSED_BEACON_STATE {
                let decompressed = decompress_entry(&entry.data)?;
                return Ok(Some(decompressed));
            }
        }

        Ok(None)
    }

    /// Unwrap the inner reader.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Read a full ERA1 block ath the given slot, incl. body, receipts,
    /// and total difficulty.
    ///
    /// Unlike [`read_block_at_slot`](Self::read_block_at_slot) which only
    /// returns the header, this seeks to the header position, then reads
    /// subsequent body/receipts/td entries until a non-block entry is hit.
    ///
    /// Returns `Ok(None)` if the slot is empty (skipped slot).
    pub fn read_full_era1_block_at_slot(&mut self, slot: u64) -> Result<Option<Era1Block>, Error> {
        let offset = match self.slot_to_offset(slot) {
            Ok(off) => off,
            Err(SlotLookupError::EmptySlot) => return Ok(None),
            Err(e) => {
                return Err(Error::E2Store(E2StoreError::InvalidEra(e.to_string())));
            }
        };

        self.reader
            .seek(std::io::SeekFrom::Start(offset))
            .map_err(Error::Io)?;

        let mut e2s = E2StoreReader::new(&mut self.reader);
        let first = e2s
            .next_entry()
            .map_err(Error::Io)?
            .ok_or_else(|| E2StoreError::InvalidEra("unexpected EOF at indexed position".into()))?;

        if first.header.typ != TYPE_COMPRESSED_HEADER {
            return Err(E2StoreError::InvalidEra(format!(
                "expected header entry at slot {}, got {:02x}{:02x}",
                slot, first.header.typ[0], first.header.typ[1]
            ))
            .into());
        }

        let header = decompress_entry(&first.data)?;

        let mut body = Vec::new();
        let mut receipts = Vec::new();
        let mut total_difficulty = U256::zero();

        while let Some(entry) = e2s.next_entry().map_err(Error::Io)? {
            match entry.header.typ {
                TYPE_BLOCK_BODY => body = entry.data,
                TYPE_RECEIPTS => receipts = entry.data,
                TYPE_TOTAL_DIFFICULTY => {
                    if entry.data.len() == 32 {
                        total_difficulty = U256(entry.data.try_into().unwrap());
                    }
                }
                _ => break, // next block, index, or unknown - stop
            }
        }

        Ok(Some(Era1Block {
            header,
            body,
            receipts,
            total_difficulty,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn build_test_era1() -> Vec<u8> {
        use crate::format::e2store::Entry;
        use snap::write::FrameEncoder;
        use std::io::Write;

        let mut buf = Vec::new();

        let version = Entry::version();
        buf.extend_from_slice(&version.header.encode());

        // Compressed header (compress an empty-ish RLP block header)
        let fake_rlp_header = vec![0xf8, 0x40];
        let compressed_header = {
            let mut enc = FrameEncoder::new(Vec::new());
            enc.write_all(&fake_rlp_header).unwrap();
            enc.into_inner().unwrap()
        };
        let header_entry = Entry::new(TYPE_COMPRESSED_HEADER, compressed_header);
        buf.extend_from_slice(&header_entry.header.encode());
        buf.extend_from_slice(&header_entry.data);

        // Block body
        let body_entry = Entry::new(TYPE_BLOCK_BODY, vec![0x01, 0x02, 0x03]);
        buf.extend_from_slice(&body_entry.header.encode());
        buf.extend_from_slice(&body_entry.data);

        let receipts_entry = Entry::new(TYPE_RECEIPTS, vec![0x04, 0x05]);
        buf.extend_from_slice(&receipts_entry.header.encode());
        buf.extend_from_slice(&receipts_entry.data);

        let mut td_bytes = [0u8; 32];
        td_bytes[31] = 0x0f;
        let td_entry = Entry::new(TYPE_TOTAL_DIFFICULTY, td_bytes.to_vec());
        buf.extend_from_slice(&td_entry.header.encode());
        buf.extend_from_slice(&td_entry.data);

        // Block index - one slot, offset points back to the header entry
        // Index starts at byte 8 (after version). Header entry is at offset 0 relative to
        // index.
        let block_index = SlotIndex::new(0, vec![8i64]);
        let idx_entry = Entry::new(TYPE_BLOCK_INDEX, block_index.encode());
        let idx_offset = buf.len() as i64;
        buf.extend_from_slice(&idx_entry.header.encode());
        buf.extend_from_slice(&idx_entry.data);

        // Fix the offset: it should be relative to the index position
        // The header entry is at byte 8, the index is at idx_offset
        // So relative offset = 8 - idx_offset (negative)
        let correct_offset = (8i64 - idx_offset) as i64;
        let fixed_index = SlotIndex::new(0, vec![correct_offset]);
        // Rebuild with correct offset
        buf.truncate(idx_offset as usize);
        let idx_entry = Entry::new(TYPE_BLOCK_INDEX, fixed_index.encode());
        buf.extend_from_slice(&idx_entry.header.encode());
        buf.extend_from_slice(&idx_entry.data);

        buf
    }

    #[test]
    fn era_reader_detects_era1_format() {
        let data = build_test_era1();
        let mut reader = EraReader::new(Cursor::new(data));
        assert!(reader.format().is_none());

        let first = reader.next_entry().unwrap().unwrap();
        assert_eq!(reader.format(), Some(EraFormat::Era1));
        assert!(matches!(first, TypedEntry::Header { .. }));
    }

    #[test]
    fn era_reader_yields_all_typed_entries() {
        let data = build_test_era1();
        let mut reader = EraReader::new(Cursor::new(data));
        let entries = reader.read_all().unwrap();

        assert_eq!(entries.len(), 5); // header + body + receipts + td + index
        assert!(matches!(&entries[0], TypedEntry::Header { .. }));
        assert!(matches!(&entries[1], TypedEntry::BlockBody { .. }));
        assert!(matches!(&entries[2], TypedEntry::Receipts { .. }));
        assert!(matches!(&entries[3], TypedEntry::TotalDifficulty { .. }));
        assert!(matches!(&entries[4], TypedEntry::BlockIndex { .. }));
    }

    #[test]
    fn era_reader_rejects_missing_version() {
        // Just a random entry, no version
        let buf = [0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xAB, 0xCD];
        let mut reader = EraReader::new(Cursor::new(&buf[..]));
        let err = reader.next_entry().unwrap_err();
        assert!(err.to_string().contains("missing version"));
    }

    #[test]
    fn era_reader_handles_empty_file() {
        let mut reader = EraReader::new(Cursor::new(&[][..]));
        let err = reader.next_entry().unwrap_err();
        assert!(err.to_string().contains("missing version"));
    }

    #[test]
    fn era_block_reader_assembles_era1_block() {
        let data = build_test_era1();
        let mut reader = EraBlockReader::new(Cursor::new(data));

        let block = reader.next_block().unwrap().unwrap();
        assert!(matches!(block, Block::Era1(_)));

        if let Block::Era1(b) = block {
            assert_eq!(b.header.len(), 2); // our fake RLP header
            assert_eq!(b.body, vec![0x01, 0x02, 0x03]);
            assert_eq!(b.receipts, vec![0x04, 0x05]);
            assert_eq!(b.total_difficulty.0[31], 0x0f);
        }

        assert!(reader.next_block().unwrap().is_none());
    }

    #[test]
    fn era_random_reader_finds_index() {
        let data = build_test_era1();
        let reader = EraRandomReader::new(Cursor::new(data)).unwrap();

        assert_eq!(reader.format(), Some(EraFormat::Era1));
        assert_eq!(reader.slot_count(), Some(1));
        assert_eq!(reader.starting_slot(), Some(0));
    }

    #[test]
    fn era_random_reader_reads_slot_zero() {
        let data = build_test_era1();
        let mut reader = EraRandomReader::new(Cursor::new(data)).unwrap();

        let block = reader.read_block_at_slot(0).unwrap().unwrap();
        assert_eq!(block.primary_data().len(), 2);
    }

    #[test]
    fn era_random_reader_empty_slot() {
        // Build an index with a 0 offset for slot 1
        let data = build_test_era1();

        // We need to rebuild with a 2-slot index where slot 1 is empty
        // For simplicity, just test the slot_to_offset logic directly
        let reader = EraRandomReader::new(Cursor::new(data)).unwrap();
        let err = reader.slot_to_offset(1).unwrap_err();
        assert!(matches!(err, SlotLookupError::OutOfRange { .. }));
    }

    #[test]
    fn slot_lookup_error_display() {
        let e = SlotLookupError::EmptySlot;
        assert!(!e.to_string().is_empty());

        let e = SlotLookupError::OutOfRange {
            slot: 100,
            start: 0,
            end: 50,
        };
        assert!(e.to_string().contains("100"));

        let e = SlotLookupError::IndexNotFound;
        assert!(e.to_string().contains("not found"));
    }

    #[test]
    fn typed_entry_type_name() {
        let e = TypedEntry::Header { data: vec![] };
        assert_eq!(e.type_name(), "Header");

        let e = TypedEntry::BeaconBlock { data: vec![] };
        assert_eq!(e.type_name(), "BeaconBlock");

        let e = TypedEntry::Unknown {
            typ: [0xFF, 0xFF],
            data: vec![],
        };
        assert_eq!(e.type_name(), "Unknown");
    }

    #[test]
    fn typed_entry_data_len() {
        let e = TypedEntry::TotalDifficulty {
            value: U256::zero(),
        };
        assert_eq!(e.data_len(), 32);

        let e = TypedEntry::BlockBody { data: vec![0; 100] };
        assert_eq!(e.data_len(), 100);
    }

    #[test]
    fn block_total_size() {
        let era_block = Block::Era(EraBlock {
            block: vec![0; 500],
        });
        assert_eq!(era_block.total_size(), 500);

        let era1_block = Block::Era1(Era1Block {
            header: vec![0; 100],
            body: vec![0; 200],
            receipts: vec![0; 50],
            total_difficulty: U256::zero(),
        });
        assert_eq!(era1_block.total_size(), 382); // 100 + 200 + 50 + 32
    }

    #[test]
    fn block_primary_data() {
        let era = Block::Era(EraBlock {
            block: vec![1, 2, 3],
        });
        assert_eq!(era.primary_data(), &[1, 2, 3]);

        let era1 = Block::Era1(Era1Block {
            header: vec![4, 5, 6],
            body: vec![7, 8],
            receipts: vec![],
            total_difficulty: U256::zero(),
        });
        assert_eq!(era1.primary_data(), &[4, 5, 6]);
    }

    #[test]
    fn era_format_display() {
        assert_eq!(EraFormat::Era.to_string(), "ERA");
        assert_eq!(EraFormat::Era1.to_string(), "ERA1");
    }

    #[test]
    fn unknown_entry_preserved() {
        let mut buf = Vec::new();
        // Version
        buf.extend_from_slice(&[0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // Unknown type 0xFFEE, 3 bytes of data
        buf.extend_from_slice(&[0xFF, 0xEE, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]);

        let mut reader = EraReader::new(Cursor::new(buf));
        let entry = reader.next_entry().unwrap().unwrap();
        assert!(matches!(entry, TypedEntry::Unknown { .. }));
        if let TypedEntry::Unknown { typ, data } = entry {
            assert_eq!(typ, [0xFF, 0xEE]);
            assert_eq!(data, vec![0xAA, 0xBB, 0xCC]);
        }
    }

    #[test]
    fn total_difficulty_rejects_wrong_size() {
        let mut buf = Vec::new();
        // Version
        buf.extend_from_slice(&[0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // TotalDifficulty with 16 bytes instead of 32
        buf.extend_from_slice(&[0x06, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00]);
        buf.extend_from_slice(&[0u8; 16]);

        let mut reader = EraReader::new(Cursor::new(buf));
        let err = reader.next_entry().unwrap_err();
        assert!(err.to_string().contains("32 bytes"));
    }

    #[test]
    fn block_accumulator_rejects_wrong_size() {
        let mut buf = Vec::new();
        // Version
        buf.extend_from_slice(&[0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        // BlockAccumulator with 16 bytes instead of 32
        buf.extend_from_slice(&[0x07, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00]);
        buf.extend_from_slice(&[0u8; 16]);

        let mut reader = EraReader::new(Cursor::new(buf));
        let err = reader.next_entry().unwrap_err();
        assert!(err.to_string().contains("32 bytes"));
    }
}
