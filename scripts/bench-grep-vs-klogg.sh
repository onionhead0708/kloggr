#!/usr/bin/env bash
# Compares a whole search, from opening the file to printing the matching
# lines, of kloggr-grep and klogg_grep on the same generated files.
#
#   scripts/bench-grep-vs-klogg.sh [size_mb ...]      default: 1024
#
# Environment:
#   KLOGG_GREP  klogg_grep binary (default: klogg_grep on PATH)
#   RUNS        runs per tool and search, best is reported (default: 3)
#   KLOGGR_BENCH_DIR  where test files are kept (default: target/bench-data;
#               must be on disk, not tmpfs, for multi-GB files)
#
# klogg_grep only takes a regular expression (-e), so every search here is
# one; the files and patterns are those of the criterion search benches.
# Times are wall clock for the whole process (startup, indexing, search,
# output to a file), with a warm page cache. kloggr-grep's own split between
# indexing and search (--time) is shown too. Each run's output is compared
# between the tools; a difference is reported. klogg_grep reads the UTF-16
# file without decoding it (it finds nothing for ERROR there), so that row
# only times kloggr-grep meaningfully.
#
# klogg_grep keeps its settings next to its binary (portable mode), so a copy
# of it runs from a temporary folder, with throw-away HOME and XDG dirs, and
# the user's klogg settings are not touched.

set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
data=${KLOGGR_BENCH_DIR:-$root/target/bench-data}
klogg_grep=${KLOGG_GREP:-$(command -v klogg_grep || true)}
runs=${RUNS:-3}
sizes=("$@")
[ ${#sizes[@]} -eq 0 ] && sizes=(1024)

if [ -z "$klogg_grep" ] || [ ! -x "$klogg_grep" ]; then
    echo "klogg_grep not found (set KLOGG_GREP)" >&2
    exit 1
fi

cargo build --release -q -p kloggr-core --example genlog
cargo build --release -q -p kloggr-grep
genlog=$root/target/release/examples/genlog
kgrep=$root/target/release/kloggr-grep

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cp "$klogg_grep" "$tmp/klogg_grep"
mkdir -p "$tmp/home"
export HOME=$tmp/home XDG_CONFIG_HOME=$tmp/home/config XDG_DATA_HOME=$tmp/home/data \
    XDG_CACHE_HOME=$tmp/home/cache QT_QPA_PLATFORM=offscreen

# Prints the wall time in milliseconds of "$@", its output going to $out.
time_ms() {
    local start end
    start=$(date +%s%N)
    "$@" >"$out" 2>"$tmp/stderr" || true
    end=$(date +%s%N)
    echo $(((end - start) / 1000000))
}

# name profile pattern
searches=(
    "plain_rare typical id=123456 "
    "plain_common typical ERROR"
    "regex typical took=\\d{4}ms"
    "short_lines short cache"
    "utf16 utf16 ERROR"
)

echo
echo "| search | file | kloggr-grep | (index + search) | klogg_grep | speedup | matches |"
echo "|---|---|---|---|---|---|---|"
for size in "${sizes[@]}"; do
    for entry in "${searches[@]}"; do
        read -r name profile pattern <<<"$entry"
        file=$("$genlog" "$profile" "$size" "$data")
        cat "$file" >/dev/null # warm the page cache

        r_best= k_best= split=
        for _ in $(seq 1 "$runs"); do
            out=$tmp/r.out
            ms=$(time_ms "$kgrep" --time -e "$pattern" "$file")
            if [ -z "$r_best" ] || [ "$ms" -lt "$r_best" ]; then
                r_best=$ms
                # "...: indexed in 74.2 ms, searched in 219.3 ms, N matches"
                split=$(sed -nE 's/.*indexed in ([0-9.]+) ms, searched in ([0-9.]+) ms.*/\1 + \2 ms/p' "$tmp/stderr")
            fi
            out=$tmp/k.out
            ms=$(cd "$tmp" && time_ms ./klogg_grep -e "$pattern" "$file")
            if [ -z "$k_best" ] || [ "$ms" -lt "$k_best" ]; then k_best=$ms; fi
        done
        matches=$(wc -l <"$tmp/r.out")
        if ! cmp -s "$tmp/r.out" "$tmp/k.out"; then
            matches="$matches (klogg_grep: $(wc -l <"$tmp/k.out"), output differs)"
        fi

        awk -v name="$name" -v file="$(basename "$file")" -v r="$r_best" -v k="$k_best" \
            -v parts="$split" -v m="$matches" 'BEGIN {
            printf "| %s | %s | %d ms | %s | %d ms | %.1f× | %s |\n", name, file, r, parts, k, k / r, m
        }'
    done
done
