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
mod tests;
