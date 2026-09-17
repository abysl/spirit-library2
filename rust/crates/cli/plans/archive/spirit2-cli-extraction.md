# Spirit2 CLI extraction

## Goal

Make the `spirit` command-line application an independently owned project named `spirit2-cli`.

## Structure

```text
projects/
├── spirit/spirit2-cli/
└── spirit/spirit2/
    ├── core/
    ├── sdk/
    └── ffi/
```

`spirit2-cli` consumes `spirit-sdk` from the adjacent `spirit2` library workspace through a direct Cargo path dependency.

## Completed

- Moved the CLI package to `projects/spirit/spirit2-cli`.
- Replaced workspace-inherited package metadata and dependencies with standalone manifest values.
- Removed the CLI from the `spirit2` workspace and updated its documentation.
- Validated the standalone application and remaining library workspace.
