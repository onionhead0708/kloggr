#!/usr/bin/env bash
# Builds kloggr's macOS package into target/package/ (run on a Mac):
#   kloggr-<version>-macos-universal.dmg   kloggr.app for Apple Silicon and Intel
# The app is only ad-hoc signed, not signed with an Apple Developer ID or
# notarized: macOS blocks the first start until it's allowed in
# System Settings > Privacy & Security, or with xattr: the dmg has a
# README.txt with both ways (packaging/macos/README.txt).
#
# Usage: scripts/package-macos.sh [--no-build]
set -euo pipefail

cd "$(dirname "$0")/.."

targets=(aarch64-apple-darwin x86_64-apple-darwin)
if [[ "${1:-}" != "--no-build" ]]; then
    for t in "${targets[@]}"; do
        rustup target add "$t"
        cargo build --release -p kloggr-app --target "$t"
    done
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
out=target/package
rm -rf "$out"
mkdir -p "$out"

# --- kloggr.app ------------------------------------------------------------
app="$out/dmg/kloggr.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
bins=()
for t in "${targets[@]}"; do bins+=("target/$t/release/kloggr"); done
lipo -create -output "$app/Contents/MacOS/kloggr" "${bins[@]}"

# The icon, from the 1024 px PNG.
iconset="$out/kloggr.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size packaging/macos/kloggr-1024.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double packaging/macos/kloggr-1024.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/kloggr.icns"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>kloggr</string>
    <key>CFBundleDisplayName</key><string>kloggr</string>
    <key>CFBundleIdentifier</key><string>io.github.onionhead0708.kloggr</string>
    <key>CFBundleExecutable</key><string>kloggr</string>
    <key>CFBundleIconFile</key><string>kloggr</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>GPL-3.0-or-later</string>
    <!-- Finder's Open With (klogg: public.text); also the compressed files
         and archives kloggr opens. Alternate: never the default app. -->
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>Log or text file</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.text</string>
                <string>public.plain-text</string>
                <string>public.log</string>
            </array>
        </dict>
        <dict>
            <key>CFBundleTypeName</key><string>Compressed log or archive</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSHandlerRank</key><string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>org.gnu.gnu-zip-archive</string>
                <string>public.bzip2-archive</string>
                <string>org.tukaani.xz-archive</string>
                <string>public.zip-archive</string>
                <string>org.7-zip.7-zip-archive</string>
                <string>public.tar-archive</string>
            </array>
        </dict>
    </array>
</dict>
</plist>
EOF

# Apple Silicon only runs signed code: an ad-hoc signature (no identity).
codesign --force --deep --sign - "$app"

# --- dmg -------------------------------------------------------------------
ln -s /Applications "$out/dmg/Applications"
cp LICENSE "$out/dmg/LICENSE.txt"
# How to allow the unsigned app (Terminal command, or System Settings).
cp packaging/macos/README.txt "$out/dmg/README.txt"
dmg="$out/kloggr-$version-macos-universal.dmg"
# hdiutil sometimes fails with "Resource busy" on CI runners: retry.
for attempt in 1 2 3 4 5; do
    if hdiutil create -volname "kloggr $version" -srcfolder "$out/dmg" -ov -format UDZO "$dmg" >/dev/null; then
        break
    fi
    [[ $attempt == 5 ]] && exit 1
    echo "hdiutil failed, retrying ($attempt)"
    sleep $((attempt * 5))
done
rm -rf "$out/dmg"
echo "built $dmg"
