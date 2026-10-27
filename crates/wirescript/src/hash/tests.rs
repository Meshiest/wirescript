use super::*;

#[test]
fn xxh64_canonical_empty_vector() {
    // Reference XXH64 test vector (seed 0).
    assert_eq!(xxh64(b"", 0), 0xEF46_DB37_51D8_E999);
}

#[test]
fn atom_hash_reinterprets_bits_as_i64() {
    assert_eq!(atom_hash("red"), xxh64(b"red", 0) as i64);
    assert_eq!(atom_hash("my-text"), xxh64(b"my-text", 0) as i64);
}

#[test]
fn xxh64_stripe_path_regression_lock() {
    // Golden value for a >32-byte input (the stripe path). It locks the
    // implementation's own output, not an independent canonical vector, so a
    // change to the algorithm cannot silently change baked atom hashes (atom
    // hashes are persisted as MapVar keys in saved worlds).
    let input = b"the quick brown fox jumps over the lazy dog!!"; // 45 bytes, hits the stripe loop
    assert_eq!(input.len(), 45);
    assert_eq!(xxh64(input, 0), 0xC40B_3EEE_2C01_1AF9);
}
