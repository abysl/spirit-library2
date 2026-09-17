import argparse
import platform
import subprocess
import tarfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RUST = ROOT / "rust"
RELEASE = RUST / "target" / "release"


def build_native():
    subprocess.run(
        [
            "cargo", "build", "--locked", "--release",
            "-p", "spirit-ffi", "-p", "spirit-cli", "--features", "uniffi/cli",
        ],
        cwd=RUST,
        check=True,
    )
    library = {
        "Linux": "libspirit_ffi.so",
        "Darwin": "libspirit_ffi.dylib",
        "Windows": "spirit_ffi.dll",
    }[platform.system()]
    executable_suffix = ".exe" if platform.system() == "Windows" else ""
    subprocess.run(
        [
            str(RELEASE / f"uniffi-bindgen{executable_suffix}"),
            "generate", "--library", str(RELEASE / library),
            "--language", "kotlin", "--out-dir",
            str(ROOT / "kmp/sdk/src/commonMain/kotlin"), "--no-format",
        ],
        cwd=RUST,
        check=True,
    )


def archive_cli(target):
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    binary = "spirit.exe" if platform.system() == "Windows" else "spirit"
    with tarfile.open(output / f"spirit-cli-{target}.tar.gz", "w:gz") as archive:
        archive.add(RELEASE / binary, arcname=binary)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", choices=["linux-x64", "macos-arm64", "windows-x64"])
    args = parser.parse_args()
    build_native()
    if args.archive:
        archive_cli(args.archive)


if __name__ == "__main__":
    main()
