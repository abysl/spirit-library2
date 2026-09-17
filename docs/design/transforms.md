# Nix transforms

Spirit2 records a transformation separately from running it. A transform is immutable content: its address is the BLAKE3 hash of its canonical bytes.

## First document shape

The first document supports exactly one executor and one result shape:

```text
TransformDocument {
  version: 1,
  executor: Nix,
  flake_source: BlobHash,
  attribute: String,
  inputs: BTreeMap<InputName, BlobHash>,
  output: AssetManifest,
}
```

`flake_source` identifies a source archive containing `flake.nix`, `flake.lock`, and any transform support files. The archive is stored as a normal Spirit blob. A transform therefore never references a mutable Git branch, a local checkout, or an ambient Nix registry.

`attribute` selects the flake output that runs the transform. `inputs` names immutable Spirit blobs made available to that build. Input names are lower-case ASCII identifiers beginning with a letter and containing only letters, digits, `_`, and `-`.

`AssetManifest` is the first output contract. A future Nix runner will walk its `$out` tree, store each produced file as a blob, and return one canonical asset-manifest blob that names those file blobs. Archive extraction is consequently one transform with many content-addressed file outputs and one content-addressed root result.

## Hashing

The document uses a fixed field order, length-prefixed UTF-8 strings, and raw 32-byte blob hashes. Input names are sorted lexicographically. The hash excludes a result because execution must not change a transform's identity.

## Boundaries

Spirit2 owns input materialization, result capture, output validation, and the transform-to-result cache. Nix owns the pinned tool environment and build execution. The first document does not run Nix, evaluate flakes, define a sandbox, or support a generic command executor.

A flake transform writes only to its declared Nix output. It does not invoke the Spirit CLI or mutate a Spirit store directly.
