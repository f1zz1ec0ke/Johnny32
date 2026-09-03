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
#   island      the composed island against `ISLETEMP.SCR`, which is the
#               1992 engine's own render of it -- the one place in the data
#               where pixels can be compared rather than decisions
#   oracle      twenty-four questions put to the 1992 binary itself and to
#               the port, and diffed -- including its own LZW and RLE run
#               over every compressed leaf in the archive, its own TTM
#               interpreter run over every frame of every script, and its
#               own per-tick pass driven a scene at a time
#   listing     the oracle listing in the Readme against what the oracle just
#               said -- quoted output can be checked like output, and twice
#               a number in the prose has gone stale unnoticed
#   reach       and where those questions went: how much of the scripts the
#               oracle actually ran, which is the half `oracle` cannot
#               report on itself. Held to its own tier's floor, so a query
#               that quietly stops reaching a tenth of the scripts fails
#               here in two minutes rather than waiting for the deep tier
#
# The last two need `original/scrantic.exe` and capstone; the rest need
# `original/RESOURCE.001`. Nothing here needs a network.
#
# `JC_DEEP=1 bash tools/check.sh` runs the exhaustive case lists throughout --
# every byte of every compressed leaf, every sequence from every starting
# position, sixty passes from eight draws -- and is the tier `reach` holds to
# its floor, which is every executable word in the ten scripts. It takes about
# three hours.
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
step island
run 9 ./johnny/target/release/jcisle
step oracle
# Kept, because the listing step reads it rather than running the oracle
# again -- twenty-odd minutes is too much to spend twice on the same answers.
oracle_out=$(mktemp)
run 2 env JC_ORACLE_OUT="$oracle_out" bash -c \
    'set -o pipefail; python3 tools/oracle.py | tee "$JC_ORACLE_OUT"'
step listing
run 4 python3 tools/listing.py "$oracle_out"
rm -f "$oracle_out"
step reach
run 7 python3 tools/reach.py

echo
[ "$fail" = 0 ] && echo "all clean" || echo "SOMETHING FAILED"
exit $fail
