# RustRover stdlib cache

`scripts/rustrover-stdlib-cache`, invoked from `.envrc`, pre-populates RustRover's
local copy of the Rust standard library sources. Without it, code insight reports
either a missing or a corrupted standard library.

## Why it is needed

RustRover reads the stdlib sources from `$(rustc --print sysroot)/lib/rustlib/src/rust`.
Because that path is under `/nix/store` and read-only, `ToolchainSourcesDiscovery`
mirrors it into `~/.cache/JetBrains/<IDE>/intellij-rust/stdlib-local-copy/<name>`
before indexing.

The copy preserves the source's permissions, so it creates the destination directory
as mode `555` and then fails to write `library/` into it:

```
java.nio.file.AccessDeniedException: .../stdlib-local-copy/<name>/library
```

The IDE keeps the empty directory and reports the stdlib as corrupted. Nothing in
RustRover's settings avoids this; the destination has to already exist, populated
and writable, before the IDE looks.

## Cache directory name

Derived in `RsPathManager.sourcesLocalCopyDir` / `createDirAndVersionHash`:

```
name    = "<release>-<hash>"
hash    = SHA1( SHA1(srcDir) ‖ SHA1(commitHash) )
srcDir  = $(rustc --print sysroot)/lib/rustlib/src/rust
release = rustc --version --verbose | release:
commit  = rustc --version --verbose | commit-hash:      (falls back to release)
```

`SHA1(...)` inside the concatenation is the raw 20-byte digest, not hex. The script
reproduces this so it can stage the directory ahead of the IDE rather than repairing
it afterwards.

## Behaviour

Steady state is a stat check, roughly 0.2s. When the directory is absent, empty, or
read-only, it stages a writable copy (~80MB, symlinks dereferenced) into a temporary
sibling and renames it into place, so the IDE never observes a partial tree.

It targets `RustRover*` cache directories, plus any other JetBrains cache directory
that already has an `intellij-rust/` subdirectory. It exits quietly when rustc is
absent, when the sysroot ships no sources, or when the sources are already writable.

## Maintenance

The name is keyed to the rustc commit, so every toolchain bump stages a fresh
directory on the next direnv load. Old versions are left alone, since the cache is
shared with other projects; delete them by hand if they accumulate.

The derivation above is read out of `intellij.rustrover.core.jar` and
`intellij.rustrover.common.jar`. A future RustRover release could change it, in which
case the symptom returns as a corrupted stdlib and the formula needs rechecking.
