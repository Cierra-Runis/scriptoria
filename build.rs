//! Gives the application's main thread (which runs the UI) an 8 MiB stack on Windows, as other
//! platforms' main threads have: GPUI views build their element trees on the stack, and in debug
//! builds (every builder step its own slot) a large view outgrows Windows' default of 1 MiB.

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target == "windows" {
        if env == "msvc" {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
