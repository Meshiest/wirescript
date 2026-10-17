// A wasm main thread's stack is fixed at link time (1 MiB by default), and the
// WASI server runs on it; this matches `HANDLER_STACK` in main.rs. Native
// builds size their stacks there instead.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("wasm") {
        println!("cargo:rustc-link-arg-bins=-zstack-size={}", 64 * 1024 * 1024);
    }
}
