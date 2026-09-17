# Spirit2

Spirit2 is organized by implementation dependency.

| Directory | Role |
|---|---|
| `rust/` | Rust workspace containing CLI, core, node, SDK, and FFI crates |
| `rust/crates/cli/` | Spirit command-line application |
| `kmp/sdk/` | Kotlin Multiplatform SDK |
| `kmp/demo/` | Kotlin Multiplatform SDK demo |
| `docs/` | Design and usage documentation |

Consumers such as AFM use `kmp/sdk` through its Gradle project and use `rust` through the CLI or native bindings.

## Private mesh CLI

Build from `rust/` with `cargo build --release -p spirit-cli`. The binary is `target/release/spirit`.

The CLI creates persistent device identities, enrolls devices through single-use QR tickets, and pings mesh members by nickname. Nicknames default to the hostname. Any member can enroll another device, and members communicate without keeping the introducer online.

See the [CLI walkthrough](docs/stories/mesh-cli.md) and [node design](wiki/design/nodes.md). Blob transfer and mobile enrollment are not implemented yet.
