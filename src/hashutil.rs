//! blake3 helpers: full hex, JSON `blake3:` prefixes, and 12+-char prefix
//! matching for `--expect` / `--expect-file`.

pub fn file_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

pub fn tagged(full_hex: &str) -> String {
    format!("blake3:{full_hex}")
}

/// Parse an `--expect` value: optional `blake3:` prefix, 12..=64 hex chars,
/// case-insensitive. Returns the lowercase prefix to match against.
pub fn parse_hash_prefix(input: &str) -> Result<String, String> {
    let stripped = input
        .strip_prefix("blake3:")
        .unwrap_or(input)
        .trim()
        .to_lowercase();
    if stripped.len() < 12 {
        return Err(format!(
            "hash prefix too short: {input:?} (minimum 12 hex characters)"
        ));
    }
    if stripped.len() > 64 {
        return Err(format!("hash too long: {input:?} (maximum 64 hex characters)"));
    }
    if !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("hash must be hex characters: {input:?}"));
    }
    Ok(stripped)
}

/// Does `full_hex` (64 lowercase hex chars) start with the parsed prefix?
pub fn matches_prefix(full_hex: &str, prefix: &str) -> bool {
    full_hex.starts_with(prefix)
}
