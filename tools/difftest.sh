#!/bin/sh
# Differential-test the Rust decoder against the Python reference.
#
# Both print the same canonical digest of every decoded chunk, sprite frame,
# background and palette, so any divergence shows up as a diff rather than as
# a picture that looks plausible but is subtly wrong.
set -e

DATA="${1:-original/RESOURCE.001}"
OUT="${TMPDIR:-/tmp}/jcdiff.$$"
mkdir -p "$OUT"
trap 'rm -rf "$OUT"' EXIT

if [ ! -f "$DATA" ]; then
    echo "no $DATA -- run tools/extract_original.py first" >&2
    exit 1
fi

cargo build --release --quiet --manifest-path johnny/Cargo.toml

python3 tools/dgds_digest.py "$DATA" > "$OUT/python.txt"
./johnny/target/release/jcdump "$DATA" > "$OUT/rust.txt"

if diff -u "$OUT/python.txt" "$OUT/rust.txt"; then
    echo "identical: $(wc -l < "$OUT/rust.txt" | tr -d ' ') lines match"
else
    echo "DIVERGED" >&2
    exit 1
fi
