#!/bin/bash
# The deep tier, one piece at a time, so it survives a machine that is not
# fast enough to run it in one go. Run from the repository root:
#
#     bash tools/deep.sh            # everything not yet done, into deep/
#     bash tools/deep.sh reach      # just the reach pieces
#     JC_DEEP_OUT=elsewhere bash tools/deep.sh
#
# Each piece is one oracle query, or one source of `reach.py`, at the deep
# case lists -- with the `tick` and `seqrun` queries and sources split per
# `.ADS` file, because at the deep tier those are the long ones. A piece
# that finishes leaves a `.log` and, if it passed, an `.ok` marker; a piece with a marker
# is skipped next time, so a run that is cut off picks up where it stopped.
# Delete the marker to run a piece again.
#
# When every reach piece has run, the saved reached sets are merged and held
# to the deep floor, which is every executable word in the ten scripts.
set -u
cd "$(dirname "$0")/.." || exit 1
out=${JC_DEEP_OUT:-deep}
mkdir -p "$out"
what=${1:-all}
fail=0

piece() {
    # piece <name> <command...>: run once, remember the outcome.
    local name=$1; shift
    if [ -e "$out/$name.ok" ]; then
        echo "-- $name (done)"
        return
    fi
    echo "== $name"
    if "$@" > "$out/$name.log" 2>&1; then
        touch "$out/$name.ok"
        tail -3 "$out/$name.log"
    else
        fail=1
        echo "   FAILED; see $out/$name.log"
        tail -5 "$out/$name.log"
    fi
}

ads_files() {
    python3 -c "import sys; sys.path.insert(0, 'tools'); import scenes
print(' '.join(sorted(set(n for n, _ in scenes.scenes()))))"
}

if [ "$what" = all ] || [ "$what" = oracle ]; then
    for q in rand weighted phase choose choose2 choose4 queue queue6 place \
             adslen holiday backdrop repeat island route turn blocks \
             seqpass codec ttm water run; do
        piece "oracle-$q" env JC_DEEP=1 python3 tools/oracle.py "$q"
    done
    for f in $(ads_files); do
        piece "oracle-seqrun-${f%.ADS}" env JC_DEEP=1 JC_ADS_ONLY="$f" \
            python3 tools/oracle.py seqrun
        piece "oracle-tick-${f%.ADS}" env JC_DEEP=1 JC_TICK_ONLY="$f" \
            python3 tools/oracle.py tick
    done
fi

if [ "$what" = all ] || [ "$what" = reach ]; then
    for s in seqpass ttm; do
        piece "reach-$s" env JC_DEEP=1 python3 tools/reach.py "$s" \
            --save="$out/reach-$s.json"
    done
    for f in $(ads_files); do
        piece "reach-seqrun-${f%.ADS}" env JC_DEEP=1 JC_ADS_ONLY="$f" \
            python3 tools/reach.py seqrun --save="$out/reach-seqrun-${f%.ADS}.json"
        piece "reach-tick-${f%.ADS}" env JC_DEEP=1 JC_TICK_ONLY="$f" \
            python3 tools/reach.py tick --save="$out/reach-tick-${f%.ADS}.json"
    done
    echo "== reach, all pieces together"
    if ls "$out"/reach-*.ok >/dev/null 2>&1 && \
       [ "$(ls "$out"/reach-*.ok | wc -l)" = "$(( 2 + 2 * $(ads_files | wc -w) ))" ]; then
        JC_DEEP=1 python3 tools/reach.py --load="$out/reach-*.json" || fail=1
    else
        echo "   not every reach piece has run yet"
        fail=1
    fi
fi

echo
[ "$fail" = 0 ] && echo "all clean" || echo "SOMETHING FAILED OR IS UNFINISHED"
exit $fail
