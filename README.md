# kloggr

A fast viewer for very large log files (multi-GB), written in Rust with a
[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) interface.
kloggr is a rewrite of [klogg](https://github.com/variar/klogg) and follows its
behaviour and keyboard commands closely.

> Status: early. Linux is the main platform; Windows and macOS builds are not
> available yet.

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
[releases page](https://github.com/onionhead0708/kloggr/releases):

- **AppImage**: `chmod +x kloggr-*.AppImage` and run it.
- **Debian / Ubuntu**: `sudo apt install ./kloggr_*.deb`
- **Tarball**: extract it and copy `bin/` and `share/` into `~/.local` or
  `/usr/local`.

kloggr draws with Vulkan, so it needs a Vulkan driver (`mesa-vulkan-drivers`
on most Linux systems).

## Build from source

Requirements (Ubuntu): `build-essential pkg-config cmake libfontconfig-dev
libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libdbus-1-dev libvulkan-dev
mesa-vulkan-drivers`, and Rust through [rustup](https://rustup.rs) (the
version is pinned in `rust-toolchain.toml`).

```bash
cargo run --release -p kloggr-app -- path/to/file.log
cargo test
scripts/package-linux.sh        # packages in target/package/
```

## License

GPL-3.0-or-later, the same as klogg. See [`LICENSE`](LICENSE). kloggr
reproduces klogg's behaviour and ports parts of its code; klogg is
Copyright (C) its authors (Nicolas Bonnefon, Anton Filimonov and contributors).
