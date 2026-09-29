# Immutable blob store

The Rust `BlobStore` imports complete files or readers in 1 MiB heap-buffered chunks into unique `tmp/` files, hashing exactly the
bytes written. It checks the expected hash for `write_verified` before syncing the staged file, then atomically renames it to the
hash-named final file and syncs the parent directory on a best-effort basis. Write-time hashing satisfies SPIRIT-01; rehashing the
same page-cache contents adds no integrity assurance. Existing files are reused only after verification; corrupt files are
replaced.

`open` exclusively locks `<store>/lock` for the store's lifetime. A second owner fails while the first is alive, including during
imports. The store requires flock-style locking; `open` returns `Io` on filesystems without it. Only the lock holder cleans
crash-left staging files and directories in `tmp/`. Reopening after dropping the owner is supported.

`has` means a completed final name exists, not that a later disk mutation has been ruled out. `get` and `export_file` rehash reads
and return `Corrupt` rather than delivering altered bytes. `export_file` stages a `.<dest-name>.spirit-<pid>-<n>.tmp` file beside
the destination, truncating the name component at a character boundary to fit 255 bytes even with the largest pid and counter. It
checks the hash before syncing and renaming, leaving the destination untouched on failed verification. An existing destination is
replaced by a new regular file, not modified in place: symlinks are replaced rather than followed, and the permissions are those
of a new file. Destination create, write, sync and commit failures return `Destination`, distinct from store-side `Io` failures.
`open_reader` and `size` are the transfer-facing exception: they expose an unverified stream and its metadata; the receiver must
call `write_verified` to check the whole file before reporting availability. Missing blobs return `NotFound` from all operations.
Parse text addresses as `BlobHash` before calling the typed APIs; parse errors convert to `StoreError::InvalidHash` when
store-level error handling is needed.

Staging files are removed on normal errors and panic unwind; `open` cleans crash leftovers. After a successful rename, directory
fsync is best-effort for every error: a renamed file may already be visible even when the directory cannot be opened or synced.
Non-Unix platforms skip directory sync.

The node-owned store serves only per-mesh shares. `has` and `size` inspect metadata.
`fetch` verifies `open_reader` bytes with `write_verified` before exposing them and cleans up staging on cancellation.
