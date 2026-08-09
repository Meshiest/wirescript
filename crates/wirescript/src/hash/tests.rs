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
