#!/bin/bash
# Everything, in one go. Run from the repository root:
#
#     bash tools/check.sh
#
# Order is cheapest first, so a break shows up quickly:
#
#   build       the port and its tools
#   tests       what can be checked without the original data at all
#   interpreter the few instructions in tools/x86.py that no oracle query
#               reaches, and so nothing else would notice were wrong
#   verify      the archive, the scripts, the generated tables, and the
#               handful of facts about the data the port's simplifications
#               rest on
#   difftest    the Rust decoder against the Python one, 3,096 lines
#   sweep       all 65 scenes, with the repaint check
#   soaks       three long runs of the whole island, with the repaint check
#   coverage    every story day, every holiday, both halves of the clock
#   framecheck  a look at what the scenes actually draw (slow)
#   oracle      twenty questions put to the 1992 binary itself and to the
#               port, and diffed -- including its own LZW and RLE run over
#               every compressed leaf in the archive
#
# The last one needs `original/scrantic.exe` and capstone; the rest need
# `original/RESOURCE.001`. Nothing here needs a network.
set -u
cd "$(dirname "$0")/.." || exit 1
fail=0
step() { echo; echo "== $1"; }

# Run a command, show the last few lines of what it said, and remember
# whether it failed. A pipeline's exit status is its *last* command's, so
# every one of these has to be run first and filtered after.
run() {
    local lines=$1; shift
    local out
    out=$("$@" 2>&1) || fail=1
    echo "$out" | tail -"$lines"
}

step build
run 1 cargo build --release --manifest-path johnny/Cargo.toml
step tests
tests=$(cargo test --release --manifest-path johnny/Cargo.toml 2>&1) || fail=1
echo "$tests" | grep -E "^test result: ok\. [1-9][0-9]* passed|FAILED|^error" | head -2
step interpreter
run 3 python3 tools/x86.py --selftest
step verify
run 14 python3 tools/verify.py
step difftest
run 1 bash tools/difftest.sh
step sweep
run 8 python3 tools/sweep.py
step soaks
for seed in 7 99 4242; do
    run 2 env JC_CHECK=1 JC_NOWRITE=1 ./johnny/target/release/jcisland 30000 "$seed"
done
step coverage
run 8 python3 tools/coverage.py
step framecheck
run 8 python3 tools/framecheck.py
step oracle
run 2 python3 tools/oracle.py

echo
[ "$fail" = 0 ] && echo "all clean" || echo "SOMETHING FAILED"
exit $fail
