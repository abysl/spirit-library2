# Spirit2

Spirit2 is organized by implementation dependency.

| Directory | Role |
|---|---|
| `rust/` | Rust workspace containing CLI, core, node, SDK, and FFI crates |
| `rust/crates/cli/` | Spirit command-line application |
| `kmp/sdk/` | Kotlin Multiplatform SDK |
| `kmp/demo/` | Kotlin Multiplatform SDK demo |
| `docs/` | Design and usage documentation |

Consumers such as AFM use `kmp/sdk` through its Gradle project and use `rust` through the CLI or native bindings.

## Private mesh CLI

Build from `rust/` with `cargo build --release -p spirit-cli`. The binary is `target/release/spirit`.

The CLI creates persistent device identities, enrolls devices through single-use QR tickets, and pings mesh members by nickname. Nicknames default to the hostname. Any member can enroll another device, and members communicate without keeping the introducer online.

See the [CLI walkthrough](docs/stories/mesh-cli.md) and [node design](wiki/design/nodes.md). The [KMP app](kmp/README.md#device-pairing-and-presence) supports Android camera pairing and desktop QR-image import, plus live peer status and pings. Blob transfer is not implemented yet.

## Build downloads

The [Build artifacts workflow](https://github.com/abysl/spirit-library2/actions/workflows/build.yml) runs on every push to `main`, pull request, or manual dispatch. Open the latest successful run and download its **Artifacts**:

| Artifact | Contents |
|---|---|
| `spirit2-android-apk` | Installable debug APK with arm64-v8a and x86_64 native libraries |
| `spirit2-desktop-linux-x64` | Debian installer |
| `spirit2-desktop-macos-arm64` | Apple Silicon DMG |
| `spirit2-desktop-windows-x64` | Windows MSI |
| `spirit2-cli-<platform>` | CLI in a `.tar.gz` archive, preserving executable permissions |
| `spirit2-web-js` | JavaScript production website |
| `spirit2-web-wasm` | WebAssembly production website |

Artifacts are retained for 30 days and require a GitHub login to download. Desktop installers include Java and the native Spirit library. They are unsigned development builds. Android uses a runner-generated debug signing key, so installing an APK from another run may require uninstalling the previous one; uninstalling removes that device's local identity. Stable release signing is not configured yet.

Serve either extracted web artifact with an HTTP server, for example `python3 -m http.server 8080`. Browser builds currently show the demo without native mesh pairing or pings; those are available on Android and desktop.

CI uses Rust 1.98.1, Java 25, the checked-in Gradle wrapper, and Android NDK 26.3.11579264. To reproduce the native preparation from the repository root, run `python3 ci/native.py` with Cargo available, then run the relevant Gradle task from `kmp/`: `:demo:desktopApp:packageDistributionForCurrentOS`, `:demo:androidApp:assembleDebug`, `:demo:webApp:jsBrowserDistribution`, or `:demo:webApp:wasmJsBrowserDistribution`. Android additionally requires `android-native` in the KMP devenv shell. Desktop installers must be built on their target operating system.
