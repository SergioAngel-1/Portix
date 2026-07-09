/// Parse a hex string to u32, returning None on failure.
pub fn parse_hex_u32(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

/// Parse a hex string to u64, returning None on failure.
pub fn parse_hex_u64(s: &str) -> Option<u64> {
    u64::from_str_radix(s, 16).ok()
}
