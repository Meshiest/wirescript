//! Deterministic hashing for `:atom` literals — canonical XXH64 (seed 0).
//! Self-contained (no external crate); an atom's integer value is
//! `xxh64(name_bytes, 0) as i64`.

const PRIME64_1: u64 = 0x9E3779B185EBCA87;
const PRIME64_2: u64 = 0xC2B2AE3D27D4EB4F;
const PRIME64_3: u64 = 0x165667B19E3779F9;
const PRIME64_4: u64 = 0x85EBCA77C2B2AE63;
const PRIME64_5: u64 = 0x27D4EB2F165667C5;

#[inline]
fn read_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}
#[inline]
fn read_u32(b: &[u8]) -> u64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64
}
#[inline]
fn round(acc: u64, input: u64) -> u64 {
    let acc = acc.wrapping_add(input.wrapping_mul(PRIME64_2));
    acc.rotate_left(31).wrapping_mul(PRIME64_1)
}
#[inline]
fn merge_round(mut acc: u64, val: u64) -> u64 {
    let val = round(0, val);
    acc ^= val;
    acc.wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4)
}

/// Canonical XXH64.
pub fn xxh64(data: &[u8], seed: u64) -> u64 {
    let len = data.len() as u64;
    let mut i = 0usize;
    let mut h64;

    if data.len() >= 32 {
        let mut v1 = seed.wrapping_add(PRIME64_1).wrapping_add(PRIME64_2);
        let mut v2 = seed.wrapping_add(PRIME64_2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(PRIME64_1);
        while data.len() - i >= 32 {
            v1 = round(v1, read_u64(&data[i..]));
            v2 = round(v2, read_u64(&data[i + 8..]));
            v3 = round(v3, read_u64(&data[i + 16..]));
            v4 = round(v4, read_u64(&data[i + 24..]));
            i += 32;
        }
        h64 = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        h64 = merge_round(h64, v1);
        h64 = merge_round(h64, v2);
        h64 = merge_round(h64, v3);
        h64 = merge_round(h64, v4);
    } else {
        h64 = seed.wrapping_add(PRIME64_5);
    }

    h64 = h64.wrapping_add(len);

    while data.len() - i >= 8 {
        let k1 = round(0, read_u64(&data[i..]));
        h64 ^= k1;
        h64 = h64.rotate_left(27).wrapping_mul(PRIME64_1).wrapping_add(PRIME64_4);
        i += 8;
    }
    if data.len() - i >= 4 {
        h64 ^= read_u32(&data[i..]).wrapping_mul(PRIME64_1);
        h64 = h64.rotate_left(23).wrapping_mul(PRIME64_2).wrapping_add(PRIME64_3);
        i += 4;
    }
    while i < data.len() {
        h64 ^= (data[i] as u64).wrapping_mul(PRIME64_5);
        h64 = h64.rotate_left(11).wrapping_mul(PRIME64_1);
        i += 1;
    }

    h64 ^= h64 >> 33;
    h64 = h64.wrapping_mul(PRIME64_2);
    h64 ^= h64 >> 29;
    h64 = h64.wrapping_mul(PRIME64_3);
    h64 ^= h64 >> 32;
    h64
}

/// An atom's wire integer: xxHash64(name, 0) bit-reinterpreted to i64.
pub fn atom_hash(name: &str) -> i64 {
    xxh64(name.as_bytes(), 0) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxh64_canonical_empty_vector() {
        // Reference XXH64 test vector (seed 0).
        assert_eq!(xxh64(b"", 0), 0xEF46DB3751D8E999);
    }

    #[test]
    fn xxh64_is_deterministic_and_length_sensitive() {
        assert_eq!(xxh64(b"red", 0), xxh64(b"red", 0));
        assert_ne!(xxh64(b"red", 0), xxh64(b"blue", 0));
        // A >32-byte input exercises the 4-accumulator stripe path.
        let long = b"the quick brown fox jumps over the lazy dog!!";
        assert!(long.len() > 32);
        assert_eq!(xxh64(long, 0), xxh64(long, 0));
    }

    #[test]
    fn atom_hash_reinterprets_bits_as_i64() {
        assert_eq!(atom_hash("red"), xxh64(b"red", 0) as i64);
        assert_eq!(atom_hash("my-text"), xxh64(b"my-text", 0) as i64);
    }

    #[test]
    fn xxh64_stripe_path_regression_lock() {
        // Golden value: locks the current XXH64 output for a >32-byte input so a
        // future edit to the algorithm can't silently change baked atom hashes
        // (atom hashes are persisted as MapVar keys in saved worlds). This is a
        // regression lock, not an independently-sourced canonical vector: the
        // expected value below was computed once from THIS implementation
        // (`cargo test -p wirescript --lib hash::tests::xxh64_stripe_path_regression_lock`
        // after a deliberate algorithm change would be the only legitimate
        // reason to update it).
        let input = b"the quick brown fox jumps over the lazy dog!!"; // 45 bytes, hits the stripe loop
        assert_eq!(input.len(), 45);
        assert_eq!(xxh64(input, 0), 0xC40B3EEE2C011AF9);
    }
}
