#!/usr/bin/env bash
# Builds kloggr's Linux packages into target/package/:
#   kloggr-<version>-<arch>-linux.tar.gz   a prefix tree (bin/, share/) to extract into ~/.local or /usr/local
#   kloggr_<version>-1_<debarch>.deb        for Debian and Ubuntu (needs dpkg-deb, fakeroot)
#   kloggr-<version>-<arch>.AppImage        only when appimagetool is on PATH (or $APPIMAGETOOL)
#
# Usage: scripts/package-linux.sh [--no-build]
set -euo pipefail

cd "$(dirname "$0")/.."
root=$(pwd)

if [[ "${1:-}" != "--no-build" ]]; then
    cargo build --release -p kloggr-app
fi

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
bin=target/release/kloggr
out=target/package
[[ -x $bin ]] || { echo "missing $bin: build it first" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"

# Installs the files of a prefix tree (bin/, share/) under $1.
install_tree() {
    local prefix=$1
    install -Dm755 "$bin" "$prefix/bin/kloggr"
    install -Dm644 packaging/linux/kloggr.desktop "$prefix/share/applications/kloggr.desktop"
    install -Dm644 packaging/linux/kloggr.svg "$prefix/share/icons/hicolor/scalable/apps/kloggr.svg"
    install -Dm644 LICENSE "$prefix/share/doc/kloggr/LICENSE"
}

# --- tarball ---------------------------------------------------------------
name="kloggr-$version-$arch-linux"
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
    rm "$deb/usr/share/doc/kloggr/LICENSE"
    cat > "$deb/usr/share/doc/kloggr/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: kloggr

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
        printf 'Source: kloggr\n\nPackage: kloggr\nArchitecture: any\n' > "$tmp/debian/control"
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
Package: kloggr
Version: $version-1
Architecture: $debarch
Maintainer: kloggr developers
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
    debfile="$out/kloggr_${version}-1_${debarch}.deb"
    fakeroot dpkg-deb --build --root-owner-group "$deb" "$debfile" >/dev/null
    rm -rf "$deb"
    echo "built $debfile"
else
    echo "skipped .deb: dpkg-deb not found"
fi

# --- AppImage --------------------------------------------------------------
tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
if [[ -n $tool ]]; then
    app="$out/kloggr.AppDir"
    install_tree "$app/usr"
    cp packaging/linux/kloggr.desktop packaging/linux/kloggr.svg "$app/"
    ln -s usr/bin/kloggr "$app/AppRun"
    ARCH=$arch "$tool" --no-appstream "$app" "$out/kloggr-$version-$arch.AppImage" >/dev/null
    rm -rf "$app"
    echo "built $out/kloggr-$version-$arch.AppImage"
else
    echo "skipped AppImage: appimagetool not found (set APPIMAGETOOL or put it on PATH)"
fi
