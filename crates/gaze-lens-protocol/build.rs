// Only the harness-free allocation test imports this std-only allocator crate.
// Production targets retain unsafe_code = "forbid" and never link the meter.
use std::{env, path::PathBuf, process::Command};

fn main() {
    let source = "tests/support/allocation_meter.rs";
    println!("cargo::rerun-if-changed={source}");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let mut rustc = Command::new(env::var_os("RUSTC").expect("Cargo RUSTC"));
    rustc.args([
        "--crate-name",
        "gaze_lens_allocation_meter",
        "--crate-type",
        "rlib",
        "--edition=2024",
        "--target",
        &env::var("TARGET").expect("Cargo TARGET"),
    ]);
    // Use Cargo's compiler, target and effective flags, including config-file
    // flags. The protocol itself is compiled and linked exclusively by Cargo.
    if let Ok(flags) = env::var("CARGO_ENCODED_RUSTFLAGS") {
        rustc.args(flags.split('\x1f').filter(|flag| !flag.is_empty()));
    }
    let result = rustc
        .arg(source)
        .arg("-o")
        .arg(out.join("libgaze_lens_allocation_meter.rlib"))
        .output()
        .expect("compile allocation meter with Cargo's rustc");
    assert!(
        result.status.success(),
        "allocation meter compilation failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo::rustc-link-search=crate={}", out.display());
}
