# SPIRIT-01 immutable whole-file milestone

## Status

Planned. SPIRIT-00 is satisfied by the accepted pairing review in commit
`84446ed`. Storage implementation starts only from that pairing and SDK
contract.

## Outcome

Prove one immutable file across three paired devices and the two first
consumers:

| Device | Required surface | Milestone action |
| --- | --- | --- |
| Linux workstation | CLI | Import one file, print its BLAKE3 hash, serve it, and report local availability. |
| Android phone | AFM through the Kotlin SDK and UniFFI | Fetch the file by hash, verify it, save or open it through AFM, and report local availability. |
| Native Linux computer | Kai through the Rust SDK | Fetch the same hash, verify it, and use the bytes as an application asset. |

The acceptance artifact is a small, redistributable image fixture that AFM can
save or open and Kai can render. The fixture is supplied during the test and is
not a bundled card collection. Every surface must report the same 64-character
hash, and every materialized file must match the imported bytes.

The three devices use the private membership and discovery established by the
SPIRIT-00 pairing contract. Pairing authorizes the devices to communicate; it
does not imply that every member automatically receives every file.

## Immutable file contract

- The identity and placement unit is the complete file. Its address is
  `BLAKE3(file bytes)` and never changes.
- Importing identical bytes is idempotent. Changing any byte creates a new
  hash. Replacing an AFM file or Kai asset updates an application-owned
  reference to a new hash; Spirit2 does not synchronize mutable files.
- A device reports a local copy only after the complete file has been written
  atomically and rehashed successfully. Temporary or interrupted transfers do
  not count.
- Reads verify the bytes against the requested hash. Corruption is an explicit
  error and must not be delivered to AFM or Kai.
- Network transfer may segment bytes internally, but chunks are not
  addressable storage objects, placement units, or public API values in this
  milestone.
- AFM owns paths, display names, and file-manager behavior. Kai owns asset keys
  and game-specific interpretation. Spirit2 stores and transfers hashes and
  bytes without learning either application's schema.

## Required surfaces

The CLI, Rust SDK, UniFFI adapter, and Kotlin SDK expose the same operations:

1. Import complete bytes from an application-selected file or buffer and
   return the hash.
2. Check whether a verified local copy is available.
3. Request a file by hash from an eligible paired device.
4. Materialize or return verified bytes.
5. Distinguish invalid hash, unavailable provider, interrupted transfer,
   missing local data, corrupt data, and destination I/O failure.

SPIRIT-02 chooses process ownership, store handles, application references,
and exact method and command names. That design must let the CLI and embedded
applications access one device store without independently opening conflicting
owners. It must not introduce a general record system to satisfy this list.

The existing in-memory byte APIs remain valid for small values. SPIRIT-02 must
decide which path or streaming operations are necessary for a real file flow;
the milestone cannot claim arbitrary file-size support while every boundary
requires the full file in memory.

## Retention and capacity layers

The end milestone exercises the later storage-policy cards without making
their policy decisions here:

1. Import the fixture on the CLI device and fetch it explicitly from AFM and
   Kai.
2. Request retention level 3 and confirm one verified retained copy on each of
   the three eligible devices.
3. Restart each device and confirm the file remains addressable and verified.
4. Interrupt a fresh transfer and confirm its partial data is not counted.
5. Make one device unavailable and show the resulting retention state without
   claiming success that was not observed.
6. Lower the request to level 1, apply cache pressure, and demonstrate safe
   reclamation while at least one required verified copy remains.

SPIRIT-03 defines offline counting and failure behavior. SPIRIT-04 defines
budget, headroom, eviction, and warnings. SPIRIT-06 and SPIRIT-07 implement
those decisions. No implementation card may infer those policies from this
scenario alone.

## Deliberate exclusions

- No Content Identity Records, attestations, collections, generic record
  framework, semantic identity above a file hash, or migration of the legacy
  Spirit record model.
- No transform execution, Nix pipeline, transform cache, archive expansion,
  format conversion, or derived-asset provenance.
- No automatic full-mesh replication. Only explicit fetches and later
  retention placement move bytes; membership gossip is not content gossip.
- No application schema in Spirit2. AFM folders and Kai asset indexes stay in
  their owning applications.
- No file mutation, folder synchronization, conflict resolution, version
  history, collaborative edits, or filesystem watch service.
- No addressable chunks, file sharding, parity blocks, or erasure coding.
- No browser, iOS, macOS, Windows, background Android service, public gateway,
  or release-distribution acceptance. Existing builds may continue to compile,
  but they do not expand this milestone.
- No unrelated pairing expansion, device removal, mesh policy, transforms, or
  broad Spirit1 compatibility work.

## Delivery sequence

| Card | Bounded responsibility |
| --- | --- |
| SPIRIT-02 | Decide store ownership, minimal application-owned references, concurrent access, and exact CLI/SDK API. |
| SPIRIT-03 | Define retention levels, offline-device accounting, repair triggers, and failure reporting. |
| SPIRIT-04 | Define disk budget, transfer headroom, cache eligibility, eviction, and warnings. |
| SPIRIT-05 | Implement verified atomic whole-file import, provider request, transfer, and fetch. |
| SPIRIT-06 | Implement retention commitments, repair, and coordinated safe cleanup. |
| SPIRIT-07 | Implement capacity accounting, cache pressure, eviction, and warnings. |
| SPIRIT-08 | Expose the agreed operations through the CLI, Rust SDK, UniFFI, and Kotlin SDK. |
| AFM-01 | Replace the text-byte demo with the Android file flow while keeping paths and names in AFM. |
| KAI-04 | Use Spirit2's Rust SDK for one native Linux asset without porting legacy record or mesh machinery. |
| SPIRIT-09 | Run the three-device scenario and record hashes, copy states, failures, restarts, and reclamation evidence. |

Each card must update this plan if implementation evidence invalidates a
boundary or reveals a missing prerequisite. Scope expansion requires a new
card rather than silently entering the milestone.

## Acceptance evidence

SPIRIT-09 is complete only with one report containing:

- the three physical devices and their assigned CLI, AFM, or Kai role;
- pairing and post-restart membership evidence;
- the source hash and independently verified hashes on all three devices;
- CLI output for import, availability, fetch, and errors;
- AFM Android evidence that the Kotlin SDK fetched and opened or saved the
  fixture;
- Kai native Linux evidence that the Rust SDK fetched and rendered the same
  fixture;
- retention-level transitions and per-device retained versus cache state;
- an interrupted transfer, an unavailable device, a corrupt local blob, and
  insufficient-capacity behavior;
- proof that cleanup never removed the final required copy; and
- explicit confirmation that no excluded platform or legacy Spirit subsystem
  was needed to pass.

Loopback processes, emulators, unit tests, and mocked providers support earlier
cards but do not replace the final three-physical-device run.
