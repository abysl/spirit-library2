# Nix transform foundation

## Goal

Define the first immutable Spirit2 transform document for a Nix-backed archive-to-asset pipeline.

## Scope

The document identifies a content-addressed flake source, Nix output attribute, named content-addressed input blobs, and the `AssetManifest` output contract. Its canonical bytes and BLAKE3 address are tested.

## Non-goals

This phase does not execute Nix, unpack archives, define asset manifests, materialize input blobs, add a transform cache, or expose CLI commands.

## Tasks

1. Define and test the transform document in `spirit-core`.
2. Re-export the public transform types from `spirit-sdk`.
3. Record the document format and executor boundary in `docs/design/transforms.md`.
4. Add a follow-up task for a Nix runner once the document review is complete.

## Acceptance criteria

- A document cannot be created with an empty or malformed named input or an empty Nix attribute.
- Documents with identical fields have identical hashes.
- Input insertion order does not affect the document hash.
- Changing the flake source, attribute, or input changes the document hash.
- Downstream Rust consumers can use the types through `spirit-sdk`.
