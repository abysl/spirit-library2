# Private mesh CLI review

## Implemented

- Added `spirit-node` with persistent Ed25519 identity, atomic state files, and exclusive process ownership.
- Added signed, transitive mesh admissions and automatic membership/address exchange over iroh 1.2.
- Added expiring, single-use pairing tickets, terminal QR rendering, and optional SVG export.
- Added `node init|id|serve|pair|ping` and `mesh create|add|status|members` through an authenticated local control service.
- Added hostname-default nicknames, explicit `--name`, nickname-based ping, and duplicate-name disambiguation through `mesh members --ids`.
- Exposed node APIs behind the SDK's optional `node` feature while retaining the blob-only dependency graph by default.
- Documented CLI workflows and the identity, membership, persistence, and protocol rules.

## Validation

- `cargo test --workspace --all-features --locked`: 29 tests passed, including the existing blob tests.
- `cargo fmt --all --check`: passed.
- `cargo clippy -p spirit-node --all-targets --locked -- -D warnings`: passed.
- Full workspace clippy passes with `-D warnings -A clippy::chunks_exact_to_as_chunks`. Without that exception, Rust 1.98 reports a pre-existing lint at `rust/crates/core/src/blob.rs:57`; that code was left unchanged.
- Separate CLI processes enrolled desktop and laptop through an introducer, exchanged membership automatically, and pinged each other by nickname after the introducer stopped.
- The CLI test restarted a device after process termination, verified persisted identity/membership, enrolled another member through the restarted device, and rejected an ambiguous nickname.
- Direct network tests rejected outsider pings, wrong-mesh synchronization, malformed pings, invalid secrets, replaced tickets, and receiver-side expired tickets.
- A two-process smoke test used the normal N0 preset, confirmed relay readiness, exported a QR SVG using a relative path, enrolled another device, and pinged by nickname. Both services exited successfully on SIGTERM and removed their control files.
- `cargo tree -p spirit-sdk --no-default-features -e normal --depth 1` confirmed that the SDK depends only on `spirit-core` without its node feature.

## Manual review

Run the walkthrough in `docs/stories/mesh-cli.md` on two separate machines without `--local` to exercise real NAT traversal. Automated and smoke-test peers ran on the same machine; relay readiness was tested, but cross-network fallback was not.

Terminal QR output and SVG generation were exercised. Scan readability on an actual phone camera is a manual check; no mobile enrollment app is included.

## Design decisions and limits

- Any admitted member can issue admissions. Membership is append-only; removal, renaming, mesh switching, and admission restrictions remain future protocol work.
- An enrollment acknowledgment can be lost after the receiving device commits. Background signed membership exchange repairs the introducer's view.
- The local control API uses authenticated loopback TCP rather than platform-specific sockets.
- Nicknames are immutable admission fields in this version. Duplicate names are accepted but never silently resolved to an arbitrary device.
- Local mode deliberately uses loopback without discovery; normal multi-machine operation uses the N0 preset.
