# Implementation review

Spirit2 builds Android APKs, Linux x64/macOS arm64/Windows x64 desktop installers and CLIs, and JavaScript/WebAssembly distributions through `.github/workflows/build.yml`. Main pushes, pull requests, and manual dispatch trigger independent platform jobs; every artifact is required and retained for 30 days. Actions are pinned to full commit IDs. CI generates UniFFI bindings and compiles native libraries from the locked Rust workspace.

The desktop package now has a valid Debian package name, a stable macOS bundle ID, and a bundled Java runtime with all modules. CLI archives preserve executable permissions. Downloads and local reproduction commands are documented in the README.

## Validation

- Actionlint and Python compilation pass.
- The cross-platform native build helper builds the release FFI, generates Kotlin bindings, and archives the Linux CLI successfully.
- Linux Debian installer and self-contained app directory build successfully.
- Both JavaScript and WebAssembly production distributions build successfully, with webpack bundle-size warnings.
- The existing Android arm64/x86_64 debug APK build, 33 Rust tests, and 13 Kotlin tests passed during the mesh implementation immediately before this work.
- Windows and macOS packaging require their hosted runners. GitHub CLI authentication is currently unavailable, so hosted run results cannot yet be inspected.

## Manual checks

Download the artifacts from the latest successful GitHub Actions run. Install the APK and each desktop installer, pair two devices, and check pings and presence. Android debug signing keys are generated per runner; cross-run upgrades may require uninstalling and lose local identity. Desktop builds are unsigned. The web demo has no native mesh support yet.

Atlas keeps the existing project path and repository visibility. Its source files are replaced with a pinned gitlink only after the child revision has been pushed. Unrelated dirty submodules and infrastructure files are excluded from the parent commit.
