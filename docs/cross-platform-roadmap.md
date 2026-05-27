# Cross-Platform Binary Roadmap

`gaze-lens` release automation is intentionally Apple Silicon-only today:

- `dist-workspace.toml` builds `aarch64-apple-darwin` only.
- The generated release workflow derives its build matrix from that dist config.
- Linux and Windows users should build from source until the checks below are green in CI.

## Current Assessment

Apple Silicon-only is no longer blocked solely by Gaze's old ONNX Runtime default. As of `gaze-recognizers 0.9.0-rc.1`, ONNX-backed recognizers are opt-in features (`runtime-candle` and `runtime-tract`), while the default recognizer set keeps phone parsing enabled without pulling `ort`.

That does not make Linux or Windows release binaries ready by itself. Lens still needs platform proof across its own filesystem, keyring, and release packaging paths.

## Known Blockers

- Windows does not compile today. `src/cli/init/atomic.rs` has `#[cfg(not(unix))] compile_error!` and uses Unix-only permissions APIs for atomic config writes and directory-mode checks.
- The keyring dependency is configured with macOS, Windows, and Linux Secret Service backends, but Linux keyring use requires a DBus/Secret Service provider. Headless Linux, containers, locked keyrings, and bare servers must keep using `password_env`.
- Linux release binaries need an explicit packaging choice for the Secret Service dependency surface. Source builds can document `pkg-config` and `libdbus-1-dev`; prebuilt binaries need CI proof on the chosen runner image and archive format.

## CI Proof Required Before Adding Dist Targets

Before adding Linux or Windows triples to `dist-workspace.toml`, land a CI PR that proves:

- `cargo check --all-targets` on native `ubuntu-latest`, `macos-latest`, and `windows-latest`.
- `cargo test --all-targets` on those same runners, excluding only Docker-backed integration features unless those services are provided.
- `gaze-lens demo` runs on each runner from the built binary.
- `gaze-lens check` reports keyring backend failures as `BACKEND UNAVAILABLE` or `ACCESS DENIED` without starting or synthesizing a DBus session.
- `cargo dist plan` and a dry-run artifact build succeed for every target proposed for release.

Only after that proof should `dist-workspace.toml` grow beyond `aarch64-apple-darwin`.
