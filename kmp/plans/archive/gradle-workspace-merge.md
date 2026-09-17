# Gradle workspace merge

## Goal

Make `spirit2/kmp` one Gradle workspace with the Kotlin SDK and KMP demo as root-owned subprojects.

## Structure

```text
spirit2/kmp/
├── spirit-sdk/
├── ktdemo/
│   ├── androidApp/
│   ├── desktopApp/
│   ├── shared/
│   └── webApp/
├── gradle/libs.versions.toml
├── settings.gradle.kts
└── build.gradle.kts
```

The demo modules are `:ktdemo:androidApp`, `:ktdemo:desktopApp`, `:ktdemo:shared`, and `:ktdemo:webApp`. `:ktdemo:shared` depends directly on `:sdk`.

## Completed

- Moved shared Gradle configuration and the version catalog to the root build.
- Registered SDK and demo modules as root subprojects.
- Replaced composite-build and published-coordinate SDK references with project dependencies.
- Consolidated Gradle wrappers and development commands at the root.
- Validated the root Gradle project configuration, generated bindings, and JVM test paths.
