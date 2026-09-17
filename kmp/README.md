# Spirit2 Kotlin Multiplatform

Kotlin Multiplatform bindings and demo applications for the sibling `../rust` workspace. The Rust `spirit-ffi` crate generates the low-level UniFFI bindings; `sdk` exposes the Kotlin API used by the demo.

## Gradle workspace

| Project | Role |
|---|---|
| `:sdk` | Kotlin SDK, `blue.rae.spirit:spirit-sdk`, targeting JVM and Android |
| `:demo:shared` | Compose Multiplatform UI and platform-neutral demo logic |
| `:demo:androidApp` | Android application |
| `:demo:desktopApp` | Desktop JVM application |
| `:demo:webApp` | JavaScript and Wasm web application |

`demo:shared` depends directly on `:sdk`. The SDK wraps generated JNA bindings with `suspend` store methods, a `BlobHash` value class, and `AutoCloseable` lifecycle management.

## Building

Run commands from this directory:

```text
direnv allow
generate-bindings
jvm-native
./gradlew :sdk:jvmTest
./gradlew :demo:shared:jvmTest
./gradlew :demo:desktopApp:run
./gradlew :demo:androidApp:assembleDebug
```

The root `devenv.nix` supplies the Android SDK and NDK, Rust targets, JDK 25, Node, Yarn, Binaryen, and the Linux runtime libraries needed by Compose Desktop. It provides `jvm-test`, `unit-test`, `desktop`, `apk`, `install`, and `assemble` convenience commands.

## IntelliJ IDEA

Start IntelliJ from the project devenv so Gradle-run desktop applications inherit the Nix OpenGL runtime libraries:

```text
devenv shell -- idea .
```

Quit existing IntelliJ processes before using this command. The project-local IntelliJ settings use the Gradle wrapper and the devenv-provided Gradle JVM.

`generate-bindings` and the native build commands compile the sibling Rust workspace and write generated bindings or Android JNI libraries into ignored SDK directories. The JVM SDK package embeds the release native library as a JNA classpath resource; Android consumes the JNI library and JNA AAR.
