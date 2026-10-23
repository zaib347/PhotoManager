//! Content hashing. The quick hash (size + first 64 KiB) is cheap enough to compute
//! for every file; full hashes are only computed when quick hashes collide.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const QUICK_BYTES: u64 = 64 * 1024;

pub fn quick_hash(path: &Path, size: u64) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut head = Vec::with_capacity(QUICK_BYTES as usize);
    File::open(path)?.take(QUICK_BYTES).read_to_end(&mut head)?;
    hasher.update(&head);
    Ok(*hasher.finalize().as_bytes())
}

pub fn full_hash(path: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(File::open(path)?)?;
    Ok(*hasher.finalize().as_bytes())
}
