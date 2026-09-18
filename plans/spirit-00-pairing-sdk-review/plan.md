# SPIRIT-00 pairing and SDK skeleton review

## Goal

Finish and review the delivered pairing work and the Rust core to CLI/UniFFI to Kotlin Multiplatform skeleton. Verify pairing behavior end to end and keep the initial contract small before storage features begin.

## Tasks

- [x] Audit the CLI, UniFFI, and Kotlin pairing surfaces for behavioral drift; align or document each difference.
- [x] Record the deliberately minimal KMP pairing contract in the project docs.
- [x] Run the Rust workspace tests, formatting, and clippy.
- [x] Run a CLI loopback pairing smoke test across two node directories.
- [x] Run the Kotlin SDK and shared demo tests when the build slot allows; otherwise record them as outstanding manual checks.
- [x] Write the review summary with verification results and manual-test instructions.

## Decisions

- The Kotlin SDK exposes the standard five-minute pairing window only. The CLI's configurable ticket lifetime is a CLI capability, not an SDK requirement; no parameter is added just to equalize the surfaces.
- Ticket whitespace trimming in the Kotlin SDK is accepted as an input-source convenience, not a contract difference; malformed tickets fail on both surfaces.
- Storage, transforms, and blob transfer stay out of this review. The store surface already exposed through UniFFI is frozen as-is.

## Review

See [the review summary](review/implementation.md) for verification results and outstanding manual checks.
