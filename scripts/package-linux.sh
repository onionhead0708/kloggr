#!/usr/bin/env bash
# Builds rlogg's Linux packages into target/package/:
#   rlogg-<version>-<arch>-linux.tar.gz   a prefix tree (bin/, share/) to extract into ~/.local or /usr/local
#   rlogg_<version>-1_<debarch>.deb        for Debian and Ubuntu (needs dpkg-deb, fakeroot)
#   rlogg-<version>-<arch>.AppImage        only when appimagetool is on PATH (or $APPIMAGETOOL)
#
# Usage: scripts/package-linux.sh [--no-build]
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)

if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release -p rlogg-app
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
bin=target/release/rlogg
out=target/package
[[ -x $bin ]] || { echo "missing $bin: build it first" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"

# Installs the files of a prefix tree (bin/, share/) under $1.
install_tree() {
    local prefix=$1
    install -Dm755 "$bin" "$prefix/bin/rlogg"
    install -Dm644 packaging/linux/rlogg.desktop "$prefix/share/applications/rlogg.desktop"
    install -Dm644 packaging/linux/rlogg.svg "$prefix/share/icons/hicolor/scalable/apps/rlogg.svg"
    install -Dm644 LICENSE "$prefix/share/doc/rlogg/LICENSE"
}

# --- tarball ---------------------------------------------------------------
name="rlogg-$version-$arch-linux"
install_tree "$out/$name"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
rm -rf "${out:?}/$name"
echo "built $out/$name.tar.gz"

# --- .deb ------------------------------------------------------------------
if command -v dpkg-deb >/dev/null; then
    debarch=$(dpkg --print-architecture)
    deb="$out/deb"
    install_tree "$deb/usr"
    # Debian keeps the license in copyright, not a separate LICENSE.
    rm "$deb/usr/share/doc/rlogg/LICENSE"
    cat > "$deb/usr/share/doc/rlogg/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: rlogg

Files: *
License: GPL-3.0-or-later
 On Debian systems, the full text of the GNU General Public License
 version 3 can be found in /usr/share/common-licenses/GPL-3.
EOF

    # Linked libraries, with versions when dpkg-shlibdeps is there.
    depends="libc6, libgcc-s1, libxcb1, libxkbcommon0, libxkbcommon-x11-0"
    if command -v dpkg-shlibdeps >/dev/null; then
        tmp=$(mktemp -d)
        mkdir -p "$tmp/debian"
        printf 'Source: rlogg\n\nPackage: rlogg\nArchitecture: any\n' > "$tmp/debian/control"
        if shlibs=$(cd "$tmp" && dpkg-shlibdeps -O "$root/$bin" 2>/dev/null); then
            depends=${shlibs#shlibs:Depends=}
        fi
        rm -rf "$tmp"
    fi
    # Loaded at run time (dlopen), so dpkg-shlibdeps can't see them.
    depends="$depends, libvulkan1, libwayland-client0"
    size=$(du -sk "$deb/usr" | cut -f1)
    mkdir -p "$deb/DEBIAN"
    cat > "$deb/DEBIAN/control" <<EOF
Package: rlogg
Version: $version-1
Architecture: $debarch
Maintainer: rlogg developers
Installed-Size: $size
Depends: $depends
Recommends: mesa-vulkan-drivers | vulkan-icd, fonts-dejavu-core | fonts-noto-core
Section: utils
Priority: optional
Description: fast viewer for very large log files
 A Rust rewrite of klogg: opens multi-GB log files quickly, searches and
 filters them with regular expressions and boolean combinations, follows
 growing files, highlights lines, and opens compressed files and archives.
EOF
    debfile="$out/rlogg_${version}-1_${debarch}.deb"
    fakeroot dpkg-deb --build --root-owner-group "$deb" "$debfile" >/dev/null
    rm -rf "$deb"
    echo "built $debfile"
else
    echo "skipped .deb: dpkg-deb not found"
fi

# --- AppImage --------------------------------------------------------------
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [[ -n $tool ]]; then
    app="$out/rlogg.AppDir"
    install_tree "$app/usr"
    cp packaging/linux/rlogg.desktop packaging/linux/rlogg.svg "$app/"
    ln -s usr/bin/rlogg "$app/AppRun"
    ARCH=$arch "$tool" --no-appstream "$app" "$out/rlogg-$version-$arch.AppImage" >/dev/null
    rm -rf "$app"
    echo "built $out/rlogg-$version-$arch.AppImage"
else
    echo "skipped AppImage: appimagetool not found (set APPIMAGETOOL or put it on PATH)"
fi
