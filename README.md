# Brandon

Standalone Rust toolkit for Ethereum ERA/ERA1 archives.

✓ Read and stream ERA files  
✓ Random-access blocks by slot  
✓ Build new ERA archives  
✓ Verify archive integrity  
✓ Convert between archive layouts  
✓ Zero-copy block iteration  
✓ Works across Ethereum client implementations  

```
$ brandon info mainnet-00000-5ec1ffb8.era1
Format:          ERA1
File size:       3.71 MiB (3891337)
Starting slot:   0
Slot range:      0..8191
Block count:     8191
State present:   no
Total entries:     32770

Entry breakdown:
  BlockAccumulator 1
  BlockBody        8192
  BlockIndex       1
  CompressedHeader 8192
  Receipts         8192
  TotalDifficulty  8192
```

## Install

From source:

```bash
git clone https://github.com/galadd/brandon.git
cd brandon
cargo install --path .
```

## CLI

### Inspect a file

```bash
brandon info file.era1
brandon info file.era --json
```

### Verify integrity

```bash
brandon verify file.era1
brandon verify file.era1 --manifest checksums.txt
```

### Read blocks

```bash
# Single block by slot
brandon read file.era1 --slot 42
brandon read file.era1 --slot 42 --format raw --output block42.bin

# Stream all blocks
brandon read file.era1 --all
brandon read file.era1 --count 10

# Extract blocks to directory
brandon read file.era1 --all --output-dir ./blocks/

# Include beacon state (ERA files only)
brandon read file.era --all --state
```

### Convert and restructure
```bash
# Rebuild indexes (fixes corrupt offsets without touching block data)
brandon convert reindex file.era1 -o fixed.era1

# Strip receipts to save disk space
brandon convert strip file.era1 -o slim.era1 --strip-receipts

# Strip everything except block headers
brandon convert strip file.era1 -o headers-only.era1 --strip-receipts --strip-bodies --strip-td

# Split into individual {slot}.snappy files
brandon convert split file.era1 --output-dir ./blocks/
```

### Build an ERA file

```bash
# From directory of {slot}.snappy files
brandon build --blocks-dir ./compressed/ --output out.era

# With beacon state
brandon build --blocks-dir ./compressed/ --state state.snappy --state-slot 8192 --output out.era
```

## Library

Add to `Cargo.toml`:

```toml
[dependencies]
brandon = "0.1"
```

### Read a file

```rust
use brandon::{EraReader, TypedEntry};

let file = std::fs::File::open("file.era1")?;
let mut reader = EraReader::new(file);

while let Some(entry) = reader.next_entry()? {
    match entry {
        TypedEntry::Header { data } => {
            println!("block header: {} bytes", data.len());
        }
        TypedEntry::BeaconBlock { data } => {
            println!("beacon block: {} bytes", data.len());
        }
        _ => {}
    }
}
```

### Random access by slot

```rust
use brandon::EraRandomReader;

let file = std::fs::File::open("file.era1")?;
let mut reader = EraRandomReader::new(file)?;

println!("slots: {}..{}", 
    reader.starting_slot()?,
    reader.starting_slot()? + reader.slot_count()? as u64
);

if let Some(block) = reader.read_block_at_slot(42)? {
    println!("found block: {} bytes", block.primary_data().len());
} else {
    println!("slot 42 is empty (skipped)");
}
```

### Build an ERA file

```rust
use brandon::write::EraBuilder;
use snap::write::FrameEncoder;
use std::io::Write;

let compressed = {
    let mut enc = FrameEncoder::new(Vec::new());
    enc.write_all(&ssz_block_bytes)?;
    enc.finish()?
};

let mut builder = EraBuilder::new();
builder.add_block(0, compressed);
builder.set_state(8192, compressed_state);

let mut output = std::fs::File::create("out.era")?;
builder.build(&mut output)?;
```

### Convert and transform
```rust
use brandon::convert::{self, strip::StripConfig};
use std::fs::File;

// Reindex a file with corrupt offsets
let input = File::open("corrupt.era1")?;
let output = File::create("fixed.era1")?;
convert::reindex::reindex(input, output)?;

// Strip receipts to save space
let input = File::open("full.era1")?;
let output = File::create("slim.era1")?;
convert::strip::strip(input, output, &StripConfig::receipts_only())?;

// ERA1 to ERA synthesis (bring your own SSZ library)
use brandon::convert::era1_to_era;
use brandon::read::Era1Block;

let input = File::open("mainnet-00000-5ec1ffb8.era1")?;
let output = File::create("synthetic.era")?;

// Buffers are reused across all blocks—zero per-block allocation
era1_to_era(input, output, |era1_block: &Era1Block, ssz_buf: &mut Vec<u8>| {
    ssz_buf.clear();
    my_ssz_lib::wrap_in_beacon_block(era1_block, ssz_buf)?;
    Ok(())
})?;
```

### Verify a file

```rust
use brandon::verify::verify_era;

let data = std::fs::read("file.era1")?;
let result = verify_era(&data[..]);

if !result.valid {
    for err in &result.errors {
        eprintln!("error: {err}");
    }
    std::process::exit(1);
}

println!("{} blocks verified", result.block_count);
```

## What it does

| CAPABILITY | DESCRIPTION |
|---|---|
| Read | Stream ERA/ERA1 files, random access by slot |
| Write | Build ERA files from compressed block data |
| Verify | Structural validation, index integrity, manifest hashes |
| Convert | Rebuild indexes, remove entries, extract block |

## Format support

| FORMAT | SPEC | DESCRIPTION |
|---|---|---|
| ERA1 | [e2store era1](https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md) | Pre-merge execution blocks |
| ERA | [e2store era](https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md) | Post-merge beacon chain |
| e2store | [e2store](https://github.com/eth-clients/e2store-format-specs) | Container format |


## Development

```bash
# Run tests
cargo test

# Run CLI from source
cargo run -- info test.era1

# Build release binary
cargo build --release
```

## License

MIT OR Apache-2.0
