//! e2store container format - the shared substrate for era, era1, erb files
//!
//! Spec: <https://github.com/eth-clients/e2store-format-specs>
//!
//! An e2store file is a sequence of entries. Each entry is:
//!   - 8-byte header: type[2] | length[4 LE] | reserved[2] (=0)
//!   - `length` bytes of data
//!
//! The first entry must be a Version entry (type 0x6532, length 0).

use crate::error::E2StoreError;
use std::io::{self, Read};

// ── Type constants ──────────────────────────────────────────────────

/// Version entry — must be the first entry in every e2store file.
pub const TYPE_VERSION: [u8; 2] = [0x65, 0x32];

// ── Header ──────────────────────────────────────────────────────────

/// e2store 8-byte entry header.
///
/// ```text
/// [type: 2 bytes] [length: 4 bytes LE] [reserved: 2 bytes]
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub typ: [u8; 2],
    pub length: u32,
    pub reserved: u16,
}

impl Header {
    pub const SIZE: usize = 8;

    pub fn new(typ: [u8; 2], length: u32) -> Self {
        Self {
            typ,
            length,
            reserved: 0,
        }
    }

    /// Decode from 8 bytes.
    pub fn decode(src: &[u8]) -> Result<Self, E2StoreError> {
        if src.len() < Self::SIZE {
            return Err(E2StoreError::TruncatedHeader(src.len()));
        }
        let typ = [src[0], src[1]];
        let length = u32::from_le_bytes([src[2], src[3], src[4], src[5]]);
        let reserved = u16::from_le_bytes([src[6], src[7]]);
        if reserved != 0 {
            return Err(E2StoreError::NonZeroReserved(reserved));
        }
        Ok(Header {
            typ,
            length,
            reserved,
        })
    }

    /// Encode to 8 bytes.
    pub fn encode(&self) -> [u8; 8] {
        let mut buf = [0u8; 8];
        buf[0..2].copy_from_slice(&self.typ);
        buf[2..6].copy_from_slice(&self.length.to_le_bytes());
        buf[6..8].copy_from_slice(&self.reserved.to_le_bytes());
        buf
    }

    pub fn is_version(&self) -> bool {
        self.typ == TYPE_VERSION
    }
}

// ── Entry ───────────────────────────────────────────────────────────

/// A parsed e2store entry: header + owned data.
#[derive(Debug, Clone)]
pub struct Entry {
    pub header: Header,
    pub data: Vec<u8>,
}

impl Entry {
    pub fn new(typ: [u8; 2], data: Vec<u8>) -> Self {
        let length = u32::try_from(data.len()).expect("entry data exceeds u32");
        Self {
            header: Header::new(typ, length),
            data,
        }
    }

    pub fn version() -> Self {
        Self::new(TYPE_VERSION, Vec::new())
    }
}

// ── Streaming reader ────────────────────────────────────────────────

/// Reads e2store entries from any `Read` source.
pub struct E2StoreReader<R> {
    inner: R,
}

impl<R: Read> E2StoreReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Read the next entry. Returns `Ok(None)` at EOF.
    pub fn next_entry(&mut self) -> Result<Option<Entry>, io::Error> {
        let mut hdr_buf = [0u8; Header::SIZE];
        match self.inner.read_exact(&mut hdr_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let header = Header::decode(&hdr_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        let mut data = vec![0u8; header.length as usize];
        self.inner.read_exact(&mut data)?;
        Ok(Some(Entry { header, data }))
    }

    /// Read all entries, validating that the first is a Version entry.
    pub fn read_all(mut self) -> Result<Vec<Entry>, io::Error> {
        let mut entries = Vec::new();
        let first = self.next_entry()?;
        match first {
            Some(e) if e.header.is_version() => entries.push(e),
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "first entry is not a version entry",
                ));
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "empty e2store file",
                ));
            }
        }
        while let Some(entry) = self.next_entry()? {
            entries.push(entry);
        }
        Ok(entries)
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_version_header() {
        let bytes = [0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let h = Header::decode(&bytes).unwrap();
        assert_eq!(h.typ, TYPE_VERSION);
        assert_eq!(h.length, 0);
        assert!(h.is_version());
    }

    #[test]
    fn encode_decode_roundtrip() {
        let h = Header::new([0x01, 0x00], 42);
        let encoded = h.encode();
        let decoded = Header::decode(&encoded).unwrap();
        assert_eq!(h, decoded);
    }

    #[test]
    fn reject_nonzero_reserved() {
        let bytes = [0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00];
        assert!(Header::decode(&bytes).is_err());
    }

    #[test]
    fn reject_truncated() {
        assert!(Header::decode(&[0x65, 0x32, 0x00]).is_err());
    }

    #[test]
    fn read_single_version_entry() {
        let data = [0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut reader = E2StoreReader::new(&data[..]);
        let entry = reader.next_entry().unwrap().unwrap();
        assert!(entry.header.is_version());
        assert!(entry.data.is_empty());
        assert!(reader.next_entry().unwrap().is_none());
    }

    #[test]
    fn read_all_validates_version_first() {
        // Version + one data entry (type 0x0100, 3 bytes "foo")
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x65, 0x32, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // version
        buf.extend_from_slice(&[0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]); // header
        buf.extend_from_slice(b"foo"); // data

        let reader = E2StoreReader::new(&buf[..]);
        let entries = reader.read_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].data, b"foo");
    }

    #[test]
    fn read_all_rejects_missing_version() {
        let buf = [
            0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, b'f', b'o', b'o',
        ];
        let reader = E2StoreReader::new(&buf[..]);
        assert!(reader.read_all().is_err());
    }
}
