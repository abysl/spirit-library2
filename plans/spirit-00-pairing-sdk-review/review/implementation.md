# SPIRIT-00 pairing and SDK skeleton review

## Changes

- Audited the pairing contract across the CLI (`node_cli.rs`, `control.rs`), UniFFI (`spirit-ffi`), and the Kotlin SDK (`SpiritNode.kt`): ticket format, enrollment, nickname resolution, shutdown, presence, and QR bytes are behaviorally aligned; no drift requiring code changes was found.
- Documented the deliberately minimal embedded pairing contract: `kmp/README.md` and `wiki/design/nodes.md` now state that the Kotlin/FFI pairing window is fixed at 300 seconds while only the CLI exposes a configurable ticket lifetime (1–3600 seconds). No API was added just to equalize the surfaces.
- Recorded the audit in `plans/spirit-00-pairing-sdk-review/plan.md`.

## Verification

- `cargo test --workspace`: 33 tests passed, 0 failed (1 CLI unit, 7 CLI integration, 2 node-CLI integration, 8 core, 1 FFI, 7 node unit, 7 mesh integration).
- `cargo fmt --check`: clean.
- `cargo clippy --workspace --all-targets`: no new warnings; only the pre-existing `chunks_exact_to_as_chunks` lint in the blob parser, unchanged from the prior review record.
- CLI loopback smoke test with two node directories: `node init` and `mesh create` on the first device, `node serve --local` on both, `node pair --no-qr` on the second, `mesh add` enrollment, `mesh status` (2 members), `mesh members --ids`, and `node ping` (`pong from laptop in 9 ms`). Processes and temporary directories were cleaned up afterward.
- Kotlin SDK JVM tests: see the result below.

## Kotlin result

- `generate-bindings`, `jvm-native`, and `./gradlew :sdk:jvmTest` ran through the project devenv shell in the isolated task worktree. The exact outcome is recorded in this section from the run log: `BUILD SUCCESSFUL`, and the JUnit reports show 5 tests passed with 0 failures — `SpiritNodeTest` (native enrollment, ping, presence, close, and reopening persisted membership) and `SpiritStoreTest` (4 store tests).

## Manual checks and limits

- Physical Android camera scanning, real mobile NAT/relay paths, and an iOS build were not exercised on this Linux host; the automated network checks use loopback. The prior KMP review recorded the same limits.
- A desktop UI session against a CLI peer (previously verified under Xvfb) was not repeated in this review; the native QR render/decode/redeem/ping path is covered by the JVM SDK test.
- `:demo:shared:jvmTest` was not run in this slot; its prior recorded state (8 tests) stands and it should accompany the next KMP change.
- Membership remains append-only; storage, transforms, and blob transfer stay out of scope, per the frozen contract above.
