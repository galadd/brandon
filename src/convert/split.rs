//! Split an ERA/ERA1 file into individual block files.
//!
//! Extract compressed block data to `{slot}.snappy` files.
//! Streams entries one at a time - no full-file memory load.
//!
//! For ERA1 files, this extracts only the compressed header.
//! For ERA files, this extracts the compressed signed beacon block.

use std::{
    fs::{self, File},
    io::{BufWriter, Read, Seek, Write},
    path::Path,
};

use crate::{
    EraRandomReader,
    error::{E2StoreError, Error},
    format::{
        e2store::E2StoreReader, era::TYPE_COMPRESSED_SIGNED_BEACON_BLOCK,
        era1::TYPE_COMPRESSED_HEADER,
    },
};

/// Split an ERA/ERA1 file into individual compressed block files.
///
/// Creates `{slot}.snappy` in the output directory for each block.
pub fn split_blocks<R, P: AsRef<Path>>(reader: R, output_dir: P) -> Result<u64, Error>
where
    R: Read + Seek,
{
    let dir = output_dir.as_ref();
    fs::create_dir_all(dir).map_err(Error::Io)?;

    let rr = EraRandomReader::new(reader)?;
    let block_slots: Vec<u64> = rr
        .block_index()
        .map(|idx| {
            idx.offsets
                .iter()
                .enumerate()
                .filter_map(|(i, &off)| {
                    if off != 0 {
                        Some(idx.starting_slot + i as u64)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut reader = rr.into_inner();
    reader
        .seek(std::io::SeekFrom::Start(0))
        .map_err(Error::Io)?;

    let mut e2s = E2StoreReader::new(&mut reader);

    let first = e2s
        .next_entry()
        .map_err(Error::Io)?
        .ok_or(E2StoreError::MissingVersion)?;
    if !first.header.is_version() {
        return Err(E2StoreError::MissingVersion.into());
    }

    let mut block_idx = 0usize;
    let mut written = 0u64;

    while let Some(entry) = e2s.next_entry().map_err(Error::Io)? {
        if entry.header.typ == TYPE_COMPRESSED_SIGNED_BEACON_BLOCK
            || entry.header.typ == TYPE_COMPRESSED_HEADER
        {
            let slot = block_slots
                .get(block_idx)
                .copied()
                .unwrap_or(block_idx as u64);

            let path = dir.join(format!("{slot}.snappy"));
            let file = File::create(&path).map_err(Error::Io)?;
            let mut buf_writer = BufWriter::new(file);
            buf_writer.write_all(&entry.data).map_err(Error::Io)?;
            buf_writer.flush().map_err(Error::Io)?;

            block_idx += 1;
            written += 1;
        }
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use snap::write::FrameEncoder;
    use tempfile::tempdir;

    use crate::{
        format::{
            Entry,
            era::{SlotIndex, TYPE_BLOCK_INDEX},
        },
        write::E2StoreWriter,
    };

    use super::*;

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut enc = FrameEncoder::new(Vec::new());
        enc.write_all(data).unwrap();
        enc.into_inner().unwrap()
    }

    #[test]
    fn split_creates_correct_files() {
        let dir = tempdir().unwrap();

        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();

        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x01])))
            .unwrap();
        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, compress(&[0x05])))
            .unwrap();

        let idx = SlotIndex::with_count(100, vec![-8i64, 0, 0, 0, 0, -8i64], 2);
        w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, idx.encode()))
            .unwrap();

        let count = split_blocks(Cursor::new(buf), dir.path()).unwrap();

        assert_eq!(count, 2);
        assert!(dir.path().join("100.snappy").exists());
        assert!(dir.path().join("105.snappy").exists());
        assert!(!dir.path().join("101.snappy").exists());
    }

    #[test]
    fn split_file_contents_match() {
        let dir = tempdir().unwrap();

        let payload = compress(&[0xAB, 0xCD, 0xEF]);
        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();
        w.write_entry(&Entry::new(TYPE_COMPRESSED_HEADER, payload.clone()))
            .unwrap();
        let idx = SlotIndex::with_count(42, vec![-8164], 1);
        w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, idx.encode()))
            .unwrap();

        split_blocks(Cursor::new(buf), dir.path()).unwrap();

        let on_disk = std::fs::read(dir.path().join("42.snappy")).unwrap();
        assert_eq!(on_disk, payload);
    }

    #[test]
    fn split_empty_file() {
        let dir = tempdir().unwrap();

        let mut buf = Vec::new();
        let mut w = E2StoreWriter::new(&mut buf);
        w.write_entry(&Entry::version()).unwrap();
        let idx = crate::format::era::SlotIndex::new(0, vec![]);
        w.write_entry(&Entry::new(TYPE_BLOCK_INDEX, idx.encode()))
            .unwrap();

        let count = split_blocks(Cursor::new(buf), dir.path()).unwrap();
        assert_eq!(count, 0);
    }
}
