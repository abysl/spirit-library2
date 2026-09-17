{pkgs, ...}: {
  languages.rust = {
    enable = true;
    channel = "stable";
    components = ["rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" "rust-src"];
  };

  packages = with pkgs; [
    cargo-nextest
  ];

  enterShell = ''
    echo "spirit2 workspace — rust $(rustc --version)"
  '';

  scripts.build.exec = "cargo build --workspace --all-targets";
  scripts."unit-test".exec = "cargo test --workspace";
  scripts.clippy.exec = "cargo clippy --workspace --all-targets -- -D warnings";
  scripts.fmt.exec = "cargo fmt";
  scripts."fmt-check".exec = "cargo fmt --check";
}
