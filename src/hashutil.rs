//! blake3 helpers: full hex and JSON `blake3:` prefixed form/file hashes.

pub fn file_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn tagged(full_hex: &str) -> String {
    format!("blake3:{full_hex}")
}
