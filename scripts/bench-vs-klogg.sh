#!/usr/bin/env bash
# Compares full-file indexing time of kloggr and klogg on the same generated
# files (the roadmap's "1–10 GB, compared with klogg").
#
#   scripts/bench-vs-klogg.sh [size_mb ...]      default: 1024
#
# Environment:
#   KLOGG     klogg binary (default: /usr/bin/klogg)
#   PROFILES  generated file shapes (default: "typical short long utf16")
#   RUNS      runs per tool and file, best is reported (default: 3)
#   KLOGGR_BENCH_DIR  where test files are kept (default: target/bench-data;
#             must be on disk, not tmpfs, for multi-GB files)
#   COLD      1: drop the file from the page cache before every run (with
#             posix_fadvise, no root needed), to time reading from the drive
#
# klogg runs headless (QT_QPA_PLATFORM=offscreen) with a throw-away HOME and
# XDG dirs, so the user's klogg settings and recent files are not touched. Its
# time is the "Indexing done, took …" line from its own log, which excludes
# startup. kloggr's time is LogFile::open (map + encoding detection + index).
# Both run with a warm page cache (every file is read once before timing),
# unless COLD=1.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
data=${KLOGGR_BENCH_DIR:-$root/target/bench-data}
klogg=${KLOGG:-/usr/bin/klogg}
profiles=${PROFILES:-typical short long utf16}
runs=${RUNS:-3}
cold=${COLD:-0}
sizes=("$@")
[ ${#sizes[@]} -eq 0 ] && sizes=(1024)

if [ ! -x "$klogg" ]; then
    echo "klogg not found at $klogg (set KLOGG)" >&2
    exit 1
fi

cargo build --release -q -p kloggr-core --example genlog --example index
bin=$root/target/release/examples

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Drops $1 from the page cache when COLD=1.
evict() {
    [ "$cold" = 1 ] || return 0
    python3 -I -c 'import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
os.posix_fadvise(fd, 0, 0, os.POSIX_FADV_DONTNEED)
os.close(fd)' "$1"
}

# Prints klogg's indexing time of $1 in milliseconds, and its index size.
klogg_once() {
    local file=$1 log=$tmp/klogg.log pid
    evict "$file"
    rm -rf "$tmp/home" && mkdir -p "$tmp/home"
    HOME=$tmp/home XDG_CONFIG_HOME=$tmp/home/config XDG_DATA_HOME=$tmp/home/data \
        XDG_CACHE_HOME=$tmp/home/cache QT_QPA_PLATFORM=offscreen \
        "$klogg" -n -m -d 3 "$file" >"$log" 2>&1 &
    pid=$!
    for _ in $(seq 1 12000); do
        grep -q 'Indexing done' "$log" && break
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    # "Indexing done, took 1234.567 ms, io 12.3 ms" (unit may be us, ms or s)
    sed -nE 's/.*Indexing done, took ([0-9.]+) ?(us|µs|ms|s)\b.*/\1 \2/p' "$log" | head -1 |
        awk '{ f = ($2 == "s") ? 1000 : ($2 == "ms") ? 1 : 0.001; printf "%.1f", $1 * f }'
    printf ' '
    sed -nE 's/.*Index size "?([^"]*)"?.*/\1/p' "$log" | head -1 | tr -d ' '
}

echo
echo "| file | size | kloggr | klogg | speedup | kloggr index | klogg index |"
echo "|---|---|---|---|---|---|---|"
for size in "${sizes[@]}"; do
    for profile in $profiles; do
        file=$("$bin/genlog" "$profile" "$size" "$data")
        if [ "$cold" = 1 ]; then
            # One run per process, the file evicted before each.
            r_ms= r=
            for _ in $(seq 1 "$runs"); do
                evict "$file"
                r=$("$bin/index" "$file" 1 | tail -1)
                ms=$(sed -E 's/.*best_ms=([0-9.]+).*/\1/' <<<"$r")
                if [ -z "$r_ms" ] || awk "BEGIN{exit !($ms < $r_ms)}"; then r_ms=$ms; fi
            done
        else
            cat "$file" >/dev/null # warm the page cache
            r=$("$bin/index" "$file" "$runs" | tail -1)
            r_ms=$(sed -E 's/.*best_ms=([0-9.]+).*/\1/' <<<"$r")
        fi
        r_idx=$(sed -E 's/.*index_bytes=([0-9]+).*/\1/' <<<"$r")
        bytes=$(sed -E 's/.*size=([0-9]+).*/\1/' <<<"$r")

        k_best= k_idx=
        for _ in $(seq 1 "$runs"); do
            read -r k_ms k_size <<<"$(klogg_once "$file")"
            [ -z "$k_ms" ] && continue
            k_idx=$k_size
            if [ -z "$k_best" ] || awk "BEGIN{exit !($k_ms < $k_best)}"; then k_best=$k_ms; fi
        done

        awk -v name="$(basename "$file")" -v bytes="$bytes" -v r="$r_ms" -v k="${k_best:-nan}" \
            -v ri="$r_idx" -v ki="${k_idx:-?}" 'BEGIN {
            gib = bytes / 1073741824
            printf "| %s | %.2f GiB | %.0f ms (%.2f GiB/s) | ", name, gib, r, gib / (r / 1000)
            if (k == "nan") printf "failed | – | "
            else printf "%.0f ms (%.2f GiB/s) | %.1f× | ", k, gib / (k / 1000), k / r
            printf "%.1f MiB | %s |\n", ri / 1048576, ki
        }'
    done
done
