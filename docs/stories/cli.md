# spirit2 — CLI user stories

Spirit2 is a smaller successor informed by `../spirit`, not a line-by-line
reimplementation of its architecture or roadmap. The first shared-storage
milestone keeps immutable whole files and application-owned references while
excluding the original record, transform, broad mesh, and platform ambitions.
Every story lists the commands a user runs, what they see, and the tests that
prove it. Markers describe Spirit2 itself: **[built]**, **[partial]**,
**[planned]**.

The bounded AFM and Kai target is defined in the
[SPIRIT-01 plan](../../plans/spirit-01-immutable-whole-file/plan.md). Legacy
Spirit documentation is design history unless a Spirit2 plan deliberately
adopts part of it.

The binary is `spirit`, built from the `spirit-cli` crate. The store lives
at `--store <dir>`, else `$SPIRIT_STORE`, else `~/.spirit2/store`. The
default deliberately differs from spirit's `~/.spirit/store` so the two
never share a directory while spirit is live.

## Crates

| Crate | Path | Role |
|---|---|---|
| `spirit-core` | `rust/crates/core/` | the primitives; today the blob store. Private modules, a curated `pub use` list in `lib.rs` |
| `spirit-sdk` | `rust/crates/sdk/` | the public surface: re-exports what consumers may depend on. Downstream code depends on this crate, never on `spirit-core` directly |
| `spirit-cli` | `rust/crates/cli/` | the `spirit` binary: clap types and `run()`, a consumer of `spirit-sdk` like any other |
| `spirit-ffi` | `rust/crates/ffi/` | the uniffi boundary over `spirit-sdk`: a `cdylib`/`staticlib` with `String` hashes and a flat error enum, from which Kotlin, Swift and Python bindings are generated. Not part of the sdk crate because `crate-type` is crate-level, uniffi must not be a dependency of Rust consumers, and its type vocabulary is an adapter over the sdk, not the sdk |
| (Gradle) `spirit-sdk` | independent `spirit2/kmp` project | the Kotlin Multiplatform library (`blue.rae.spirit:spirit-sdk`, jvm + android): generated bindings plus a hand-written idiomatic wrapper |
| (Gradle) `demo` | `spirit2/kmp/demo` | the Compose Multiplatform demo app consuming its parent SDK project as a composite build; a put/get panel on desktop and Android, "not available" on web |

The CLI crate is the first consumer of the sdk and proves the surface is
enough to build an application on.

## 1. Blobs in and out **[built]**

As a user I want to put bytes into the store and read them back by hash, so
that anything I hand to spirit2 is content-addressed and verified.

```
$ spirit blob put notes.txt
9f3c…e1                       # 64 hex, the BLAKE3 hash of the bytes

$ echo hello | spirit blob put -
…                             # `-` reads stdin

$ spirit blob get 9f3c…e1
<the bytes, raw, on stdout>   # so `blob get … > file` round-trips

$ spirit blob get 9f3c…e1 --out copy.txt
wrote 12 bytes to copy.txt

$ spirit blob get 0000…00
spirit: blob 0000…00 not in store
$ echo $?
1
```

Rules the story pins down:

- `put` is idempotent: the same bytes yield the same hash and one file.
- `get` re-hashes what it reads and refuses a file whose name lies:
  `blob <hash> is corrupt (hashes to <other>)`.
- A hash argument that is not 64 hex characters is rejected by the parser
  before the store is touched.
- Blobs are the files `<store>/<hex>`; nothing else is written for this story.

Covered by `rust/crates/cli/tests/cli.rs` (the binary,
end to end), the unit tests in `crates/core/src/store.rs` (the store on its own), the
`spirit2/kmp` SDK's `sdk/src/jvmTest` (the same story through the Kotlin API),
and its `ktdemo/shared/src/jvmTest` (through the demo app's `BlobStore`).

Maps to spirit's `spirit-node blob put|get` (`node/src/cli.rs`) over
`spirit-core::BlobStore` (`crates/core/src/store.rs`).

## 2. Blob has and list **[planned]**

`blob has <hash>` prints `true`/`false`; `blob list` prints every hash with
its size.

## 3. Blobs larger than memory **[planned]**

`blob put` streams a file through the hasher and into the store without
holding it in memory; `blob get --out` verifies while it streams. The
`Blobs` API gains a reader form. Motivated by flac and video, the content
spirit was designed for.

## Small blobs and records **[deferred]**

The legacy Spirit design used CIR, TDR, attestation, and collection records.
Those records, a small-blob SQLite tier, a local semantic index, and a browser
store are not part of the first AFM and Kai milestone. Revisit them only from a
separate consumer need; do not treat them as implied follow-ups to blob import
and fetch.

## 4. Private device mesh **[built]**

Initialize device identities, create a mesh, enroll devices using QR-compatible tickets, and ping members by nickname. Nicknames default to the hostname. Any member can admit new devices, and membership propagates automatically. See the [mesh CLI user story](mesh-cli.md) for the commands and acceptance tests.

`spirit-node` owns networking and signed membership. `spirit-sdk` exposes it through the optional `node` feature, enabled by the CLI.
