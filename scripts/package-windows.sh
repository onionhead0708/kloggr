#!/usr/bin/env bash
# Builds kloggr's Windows packages into target/package/ (run in Git Bash on
# Windows, as GitHub's runners do):
#   kloggr-<version>-windows-x64.zip   portable: unzip and run kloggr.exe
#   kloggr-<version>-x64.msi           installer (needs WiX 5: dotnet tool install --global wix --version 5.0.2)
# Neither is code-signed: Windows SmartScreen warns on the first start.
#
# Usage: scripts/package-windows.sh [--no-build]
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release -p kloggr-app
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
bin=target/release/kloggr.exe
out=target/package
[[ -f $bin ]] || { echo "missing $bin: build it first" >&2; exit 1; }
rm -rf "$out"
mkdir -p "$out"

# --- zip -------------------------------------------------------------------
name="kloggr-$version-windows-x64"
mkdir -p "$out/$name"
cp "$bin" "$out/$name/"
cp LICENSE "$out/$name/LICENSE.txt"
cp README.md "$out/$name/"
(cd "$out" && 7z a -tzip -bso0 "$name.zip" "$name")
rm -rf "${out:?}/$name"
echo "built $out/$name.zip"

# --- msi -------------------------------------------------------------------
if command -v wix >/dev/null; then
    msi="$out/kloggr-$version-x64.msi"
    wix build packaging/windows/kloggr.wxs -arch x64 \
        -d "Version=$version" \
        -d "BinFile=$bin" \
        -d "IconFile=packaging/windows/kloggr.ico" \
        -d "LicenseFile=LICENSE" \
        -o "$msi"
    rm -f "$out"/*.wixpdb
    echo "built $msi"
else
    echo "skipped .msi: wix not found (dotnet tool install --global wix --version 5.0.2)"
fi
