# Allocation admission proof

`cargo test -p gaze-lens-protocol --test allocation_admission` runs the original
scalar, binding, body and outbound admission controls in a standalone process.
Cargo compiles the probe and supplies the exact protocol dependency for the
current source, compiler, target, profile and flags. There is no runtime compiler
invocation, rlib search, timestamp selection or cache cleanup.

The package build script compiles only the std-only allocator meter into its
Cargo-provided `OUT_DIR`, using Cargo's `RUSTC`, `TARGET` and effective Rust flags.
The test imports that helper through its build's crate search path. Production
source and the probe retain workspace `unsafe_code = "forbid"`; only the meter's
System delegation contains unsafe code. Production targets never import/link the
meter. This adds a small helper compilation even to non-test package builds.
No package dependency is added.

`harness = false` keeps libtest threads and other tests out of the process-wide
measurement interval. Fixture creation and reporting remain outside it. The
counters measure maximum requested allocation size and cumulative requested
bytes, **not RSS or peak live memory**. The probe always runs its complete controls,
including when Cargo is given a test-name filter or `--list`. Success prints
`allocation admission controls passed`; failures return nonzero.

## Mixed-cache regression

Run from any directory, with the two historical commits available locally:

```sh
bash crates/gaze-lens-protocol/tests/support/reproduce_mixed_cache.sh /absolute/unused/proof-directory
```

The script archives broken parent `6963affb` and corrected source `6a4fe798` into
owned fixture directories, overlays only the current test/build wiring, and uses
one shared target directory throughout. It verifies actual allocation failure
(49000 requested bytes, not a compiler error) before and after a newer corrected
alternate-RUSTFLAGS build. The corrected alternate build passes in the same mixed cache, including a
cached rerun, and the cached parent still fails afterwards. All sources, logs and artifacts are retained.
The initial empty target establishes the negative control; nothing cleans or
selects artifacts between runs. The optional argument must be an unused proof
location. The script needs Bash, Git, tar and locally cached locked dependencies;
Cargo runs offline with two jobs and does not access production or upstreams.

Normal cached developer builds remain valid; rerun the Cargo test command above
to check them without rebuilding. Cargo owns protocol freshness and linkage,
so replacing or manually executing stale binaries outside Cargo is not a source
freshness guarantee. Unsupported compiler/target/flag combinations fail at build
or execution; the test never substitutes another cached protocol artifact.
