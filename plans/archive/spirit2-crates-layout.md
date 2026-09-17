# Spirit2 Rust layout

## Goal

Organize Spirit2 by implementation dependency, with the Rust workspace under `rust/` and its packages beneath a conventional `crates/` directory.

## Structure

```text
spirit2/
├── rust/
│   ├── Cargo.toml
│   └── crates/
│       ├── core/
│       ├── sdk/
│       └── ffi/
├── docs/
└── scripts/
```

The sibling `spirit2/cli` application continues to consume `spirit-sdk` through its direct path dependency.

## Completed

- Moved the Rust workspace into `rust/`.
- Kept the Rust library packages beneath `rust/crates/`.
- Updated workspace members and path dependencies.
- Updated documentation paths.
- Validated the library workspace and standalone CLI dependency.
