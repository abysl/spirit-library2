# GitHub artifacts and Atlas submodule

1. Publish the current CLI and KMP mesh implementation to the existing `abysl/spirit-library2` repository.
2. Build installable Android debug APKs, Linux x64/macOS arm64/Windows x64 desktop installers and CLIs, and JavaScript/WebAssembly production distributions on main pushes, pull requests, and manual dispatch.
3. Generate UniFFI bindings and native libraries in CI; upload outputs as named GitHub Actions artifacts with 30-day retention.
4. Validate build commands locally, inspect the first hosted run when API access is available, and document download instructions and platform limitations.
5. Replace Atlas's tracked Spirit2 sources with a submodule at their existing path, preserving the project checkout and unrelated work. Push the child before recording the parent pointer.
