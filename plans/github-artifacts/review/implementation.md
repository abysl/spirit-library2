# Implementation review

Spirit2 builds Android APKs, Linux x64/macOS arm64/Windows x64 desktop installers and CLIs, and JavaScript/WebAssembly distributions through `.github/workflows/build.yml`. Main pushes, pull requests, and manual dispatch trigger independent platform jobs; every artifact is required and retained for 30 days. Actions are pinned to full commit IDs. CI generates UniFFI bindings and compiles native libraries from the locked Rust workspace.

The desktop package now has a valid Debian package name, a stable macOS bundle ID, and a bundled Java runtime with the Java SE, native-access, crypto, locale, charset, and ZIP modules. CLI archives preserve executable permissions. Downloads and local reproduction commands are documented in the README.

## Validation

- Actionlint and Python compilation pass.
- The cross-platform native build helper builds the release FFI, generates Kotlin bindings, and archives the Linux CLI successfully.
- Linux Debian installer and self-contained app directory build successfully.
- Both JavaScript and WebAssembly production distributions build successfully, with webpack bundle-size warnings.
- The existing Android arm64/x86_64 debug APK build, 33 Rust tests, and 13 Kotlin tests passed during the mesh implementation immediately before this work.
- Hosted validation is in progress. The first run passed Rust checks and macOS JVM tests, then exposed an obsolete Android SDK package default and a Java runtime packaging failure. Android setup now explicitly installs `platform-tools`. Desktop packaging selects application runtime modules instead of every JDK module and preserves diagnostics on failure. Web builds now download the Kotlin plugin’s matching toolchain outside devenv; the first hosted run had no `wasm-opt` because automatic downloads were unconditionally disabled.

Temurin 24+ uses runtime-image linking, which cannot include `jdk.jlink` itself; see [Adoptium’s packaging guidance](https://adoptium.net/news/2025/03/eclipse-temurin-jdk24-JEP493-enabled).

## Manual checks

Download the artifacts from the latest successful GitHub Actions run. Install the APK and each desktop installer, pair two devices, and check pings and presence. Android debug signing keys are generated per runner; cross-run upgrades may require uninstalling and lose local identity. Desktop builds are unsigned. The web demo has no native mesh support yet.

Atlas keeps the existing project path and repository visibility. Its source files are replaced with a pinned gitlink only after the child revision has been pushed. Unrelated dirty submodules and infrastructure files are excluded from the parent commit.
