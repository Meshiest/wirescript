//! Compiler-internal hash maps.
//!
//! The default std hasher (SipHash) is DoS-resistant but slow; profiling a
//! large-program compile showed ~29% of total wall time inside SipHash for
//! maps keyed by `NodeId` and small tuples. Compiler tables hash untrusted
//! but non-adversarial data, so the rustc-style Fx hasher is the right
//! trade. Construct with `HashMap::default()` (the `new()` constructor only
//! exists for the std hasher).
//!
//! Iteration order note: Fx maps iterate in a deterministic (per-content)
//! order, unlike std's per-process random seed — anything that was already
//! order-independent stays correct, and run-to-run output becomes *more*
//! stable, never less.

pub type HashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type HashSet<T> = rustc_hash::FxHashSet<T>;
