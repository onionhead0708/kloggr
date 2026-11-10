# kloggr

A fast viewer for very large log files (multi-GB), written in Rust with a
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) interface.
kloggr is a rewrite of [klogg](https://github.com/variar/klogg) and follows its
behaviour and keyboard commands closely.

> Status: early. Linux is the main platform. Windows and macOS builds are
> made by CI but tested less, and they are not code-signed (see below).

## Features

- Opens multi-GB files quickly: memory-mapped, indexed in parallel, with
  progress. Detects UTF-8, UTF-16 and legacy encodings.
- Search with regular expressions, plain text or case-insensitive text, and
  klogg's boolean combinations (`"error" and not "timeout"`). Matching lines
  appear in a filtered view under the file.
- Quick find (`/`, `n`, `N`), marks, jump to line, an overview bar of matches.
- Follow mode for growing files, with rotation and truncation detection.
- Highlighters, highlighter sets and color labels; klogg `.conf` import and
  export.
- Tabs and sessions, frozen columns, wide (CJK) characters.
- Predefined filters, search history, scratchpad with encoders and decoders.
- Opens gzip, bzip2, xz, zip, 7z and tar files directly.

Differences from klogg are listed in [`docs/decisions.md`](docs/decisions.md).

## Install

Download a package from the
[releases page](https://github.com/onionhead0708/kloggr/releases).

### Linux

- **AppImage**: `chmod +x kloggr-*.AppImage` and run it.
- **Debian / Ubuntu**: `sudo apt install ./kloggr_*.deb`
- **Tarball**: extract it and copy `bin/` and `share/` into `~/.local` or
  `/usr/local`.

kloggr draws with Vulkan, so it needs a Vulkan driver (`mesa-vulkan-drivers`
on most Linux systems).

### Windows (10 and 11, x64)

- **Installer**: run `kloggr-*-x64.msi`. kloggr appears in the Start menu.
- **Portable**: unzip `kloggr-*-windows-x64.zip` anywhere and run
  `kloggr.exe`.

The packages are not code-signed, so SmartScreen may show *"Windows protected
your PC"* the first time. Click **More info**, then **Run anyway**.

### macOS (11 or later, Apple Silicon and Intel)

Open `kloggr-*-macos-universal.dmg` and drag **kloggr** to **Applications**.

The app is not signed with an Apple Developer ID or notarized, so macOS blocks
the first start (*"kloggr" can't be opened* or *Apple could not verify…*).
To allow it:

1. Try to open kloggr once and close the message.
2. Open **System Settings > Privacy & Security**, scroll down and click
   **Open Anyway** next to the message about kloggr, then confirm.

Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/kloggr.app`

On macOS, open files with **File > Open** or by dragging them onto the
window. Opening files from Finder with *Open With* doesn't work yet.

## Build from source

Install Rust through [rustup](https://rustup.rs); the version is pinned in
`rust-toolchain.toml`.

- **Linux** (Ubuntu): `build-essential pkg-config cmake libfontconfig-dev
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libdbus-1-dev
  libvulkan-dev mesa-vulkan-drivers`
- **Windows**: Visual Studio Build Tools with the C++ workload.
- **macOS**: Xcode (the full app, for the Metal shader compiler).

```bash
cargo run --release -p kloggr-app -- path/to/file.log
cargo test
```

Packages go to `target/package/`:

```bash
scripts/package-linux.sh     # AppImage (with appimagetool), .deb, tarball
scripts/package-windows.sh   # .zip, and .msi with WiX 5 (Git Bash on Windows)
scripts/package-macos.sh     # universal .dmg (on a Mac)
```

Pushing a tag like `v0.2.0` makes GitHub Actions build all of them and
publish a release.

## License

GPL-3.0-or-later, the same as klogg. See [`LICENSE`](LICENSE). kloggr
reproduces klogg's behaviour and ports parts of its code; klogg is
Copyright (C) its authors (Nicolas Bonnefon, Anton Filimonov and contributors).
