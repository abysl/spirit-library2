# SPIRIT-01 review summary

## Changes

- Defined the three-device immutable whole-file milestone with concrete CLI,
  Android AFM/Kotlin, and native Linux Kai/Rust roles.
- Separated the file-hash storage contract from application-owned names and
  semantics, and assigned ownership, retention, capacity, implementation,
  bindings, consumer integrations, and final proof to the existing Kanban
  cards.
- Reconciled Spirit2's README and CLI story with its narrower scope, and marked
  the existing Nix transform foundation as deferred outside SPIRIT-01 through
  SPIRIT-09.
- Explicitly excluded broad records, transforms, automatic full-mesh content
  replication, mutable synchronization, erasure coding, and extra platform
  acceptance.

## Verification

- Reviewed the accepted SPIRIT-00 pairing handoff and Spirit2 commit `84446ed`.
- Checked the current blob store, CLI, Rust SDK, UniFFI, Kotlin SDK, AFM demo,
  and Kai Spirit integration to distinguish built local byte operations from
  the planned network storage flow.
- Checked every relative Markdown link in the changed files.
- No code or lock files changed; build and runtime tests are not applicable to
  this documentation-only planning card.

## Manual review

- Confirm the selected physical acceptance roles: Linux CLI workstation,
  Android AFM phone, and native Linux Kai computer.
- Confirm that retention level 3 to level 1 remains part of the final SPIRIT-09
  proof while its offline and capacity semantics remain owned by SPIRIT-03 and
  SPIRIT-04.
- Confirm that a small redistributable image is the right shared fixture for
  AFM to save or open and Kai to render.

## Follow-ups and risks

- Current stores are opened independently by the blob and node APIs. SPIRIT-02
  must resolve one-device ownership and concurrent embedded/CLI access before
  storage implementation begins.
- Current Rust, FFI, and Kotlin blob calls materialize full files in memory.
  SPIRIT-02 must bound that limitation or specify path and streaming APIs before
  the milestone can claim general file-size support.
- Kai currently integrates the broader legacy Spirit library. KAI-04 must use
  the narrow Spirit2 Rust SDK path without importing its record, transform, or
  automatic mesh behavior into this milestone.
