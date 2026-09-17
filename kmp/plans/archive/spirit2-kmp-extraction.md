# Spirit2 KMP extraction

## Goal

Make the Kotlin Multiplatform SDK an independently owned sibling project named `spirit2/kmp`, with `ktdemo` as its consumer application.

## Structure

```text
libs/
├── spirit2/
│   ├── core/
│   ├── sdk/
│   ├── ffi/
│   └── cli/
└── spirit2/kmp/
    ├── spirit-sdk/
    └── ktdemo/
```

`spirit2/kmp` consumes the native UniFFI library from the adjacent `spirit2` Rust workspace. Its build configuration owns the relative Rust-workspace path.

## Completed

- Moved the Kotlin SDK project to `spirit/spirit2/kmp` and nested `ktdemo` within it.
- Updated Gradle composite-build, Cargo artifact, generated-binding, and Android JNI paths.
- Updated development scripts and documentation to describe the sibling Rust dependency.
- Validated the Rust workspace. Gradle configuration requires the JDK supplied by the project devenv.
