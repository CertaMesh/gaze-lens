# Cross-Platform Binary Roadmap

`gaze-lens` release automation covers Apple Silicon and Linux today:

- `dist-workspace.toml` builds `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, and `aarch64-unknown-linux-gnu`.
- The generated release workflow derives its build matrix from that dist config and publishes tarballs plus `.sha256` sidecars.
- Windows users should build from source until the checks below are green in CI.

## Current Assessment

Apple Silicon-only is no longer blocked solely by Gaze's old ONNX Runtime default. As of `gaze-recognizers 0.9.0-rc.1`, ONNX-backed recognizers are opt-in features (`runtime-candle` and `runtime-tract`), while the default recognizer set keeps phone parsing enabled without pulling `ort`.

Linux is now CI-proven and wired into release automation. Windows still needs platform proof across its own filesystem, keyring, and release packaging paths.

## Known Blockers

- Windows does not compile today. `src/cli/init/atomic.rs` has `#[cfg(not(unix))] compile_error!` and uses Unix-only permissions APIs for atomic config writes and directory-mode checks.
- The keyring dependency is configured with macOS, Windows, and Linux Secret Service backends, but Linux keyring use requires a DBus/Secret Service provider. Headless Linux, containers, locked keyrings, and bare servers must keep using `password_env`; `check` reports `BACKEND UNAVAILABLE` or `ACCESS DENIED` instead of synthesizing a DBus session.
- Linux release binaries install `pkg-config` and `libdbus-1-dev` in the cargo-dist release workflow so the Secret Service build surface matches CI proof.

## CI Proof Required Before Adding Dist Targets

Before adding Windows triples to `dist-workspace.toml`, land a CI PR that proves:

- `cargo check --all-targets` on native `ubuntu-latest`, `macos-latest`, and `windows-latest`.
- `cargo test --all-targets` on those same runners, excluding only Docker-backed integration features unless those services are provided.
- `gaze-lens demo` runs on each runner from the built binary.
- `gaze-lens check` reports keyring backend failures as `BACKEND UNAVAILABLE` or `ACCESS DENIED` without starting or synthesizing a DBus session.
- `cargo dist plan` and a dry-run artifact build succeed for every target proposed for release.

Linux proof is complete for `x86_64-unknown-linux-gnu` on `ubuntu-latest` and `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`, plus the existing macOS lane. Windows remains out of scope until the compile blocker above is resolved.
