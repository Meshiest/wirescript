//! Compiler-internal hash maps.
//!
//! The default std hasher (`SipHash`) is DoS-resistant but slow, and compiler tables hash
//! non-adversarial data (`NodeId`s, small tuples), so the rustc-style Fx hasher is used.
//! Construct with `HashMap::default()` (the `new()` constructor only exists for the std
//! hasher).
//!
//! Fx maps iterate in a deterministic per-content order, unlike std's per-process random
//! seed.

pub type HashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type HashSet<T> = rustc_hash::FxHashSet<T>;
