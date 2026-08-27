"""What the oracle actually reaches, as a fraction of what there is.

    python3 tools/reach.py              # every source, in parallel
    python3 tools/reach.py tick         # one source on its own
    JC_DEEP=1 python3 tools/reach.py    # the exhaustive case lists

`tools/oracle.py` diffs the port against the 1992 binary and says the two
agree. It says that *wherever the run went*. It does not say where the run
went, and a harness can agree perfectly about four percent of a program.

This measures the other half. It drives the same engine through the same
queries, counts what each one touches, and reports the union:

* **ADS instructions.** Every one the interpreter at `seg10:0x1519` is handed,
  against all 2,688 in the ten scripts. This is the branch number -- a guard
  that never fired, a random block that always went one way, and a sequence
  nothing ever started are all instructions that never ran.
* **TTM frames.** Every one `seg12:0x040C` is handed, against all 6,631.
* **Scenes.** Every `1111` marker that got played, against all 611.

The sources are the queries that *execute* something. `blocks` and `adslen`
are not here: they ask a static question at every instruction in the data, so
they cover all 2,688 by construction and would flatter the number without
telling anyone anything.
"""

import collections
import multiprocessing
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.getcwd())
import oracle

# Where the dispatcher's argument sits, which is not the same in the two ways
# of watching it. `watch` fires inside `call_far`, before the return address
# goes on; `watch_at` fires at the entry, after it. Both were checked against
# the instruction offsets in the scripts rather than assumed.
AT_CALL = 2
AT_ENTRY = 4


def totals():
    """What there is to reach."""
    ads = {name: set(off for off, _op, _a in oracle.ads_walk(code))
           for name, code in oracle.ads_scripts()}
    frames, scenes = {}, set()
    for name, code in oracle.ttm_scripts():
        n, p = 0, 0
        while p + 2 <= len(code):
            op = struct.unpack_from('<H', code, p)[0]
            count = op & 0x0F
            if count == 0x0F:
                end = code.index(0, p + 2)
                size = (end + 1) - p
                p += size + (size & 1)
                continue
            if op == 0x0FF0:
                n += 1
            elif op == 0x1111 and count == 1:
                scenes.add((name, struct.unpack_from('<H', code, p + 2)[0]))
            p += 2 + count * 2
        frames[name] = n
    return ads, frames, scenes


def blank():
    return {'ads': collections.defaultdict(set),
            'frames': collections.defaultdict(set),
            'scenes': set()}


def merge(into, other):
    for name, hits in other['ads'].items():
        into['ads'][name] |= hits
    for name, hits in other['frames'].items():
        into['frames'][name] |= hits
    into['scenes'] |= other['scenes']
    return into


# ---- the sources ---------------------------------------------------------

def do_seqrun(item):
    """One sequence run over and over, the way `seqrun` runs it."""
    name, seq, passes = item
    code = dict(oracle.ads_scripts())[name]
    layout = oracle.ads_layout(code)
    got = blank()
    m = oracle.fresh()
    m.watch_at(10, 0x1519, lambda mm: got['ads'][name].add(
        mm.read16(mm.ss, mm.regs['sp'] + AT_ENTRY) - oracle.ADS_SCRIPT))
    oracle.ads_run(m, code, layout, seq, oracle.ads_all_pairs(layout), passes)
    return got


def do_seqpass(item):
    """One pass of one sequence from one starting position, as `seqpass`."""
    name, seq, states = item
    code = dict(oracle.ads_scripts())[name]
    layout = oracle.ads_layout(code)
    got = blank()
    m = oracle.fresh()
    m.watch_at(10, 0x1519, lambda mm: got['ads'][name].add(
        mm.read16(mm.ss, mm.regs['sp'] + AT_ENTRY) - oracle.ADS_SCRIPT))
    pairs = sorted(set(p for _o, ps in layout.values() for p in ps))
    oracle.ads_pass(m, code, layout, seq, states, pairs)
    return got


def do_ttm(item):
    """Every frame of one script, the way `ttm` runs them: one at a time,
    with nothing else on screen."""
    name, = item
    code = dict(oracle.ttm_scripts())[name]
    got = blank()
    e = oracle.Interpreter()
    n = e.load(code)
    for k in range(n):
        got['frames'][name].add(k)
        e.frame(k)
    # Every `1111` in the file gets an instance record when it loads, and
    # every frame of the file is run, so every scene is reached.
    for off, op, args in _markers(code):
        got['scenes'].add((name, args))
    return got


def _markers(code):
    p = 0
    while p + 2 <= len(code):
        op = struct.unpack_from('<H', code, p)[0]
        count = op & 0x0F
        if count == 0x0F:
            end = code.index(0, p + 2)
            size = (end + 1) - p
            p += size + (size & 1)
            continue
        if op == 0x1111 and count == 1:
            yield p, op, struct.unpack_from('<H', code, p + 2)[0]
        p += 2 + count * 2


def do_tick(item):
    """A whole tick, over and over: the script and the instances together."""
    name, seq, ticks = item
    got = blank()
    world = oracle.World(name, seq)
    files = world.files
    m = world.m

    def instruction(mm):
        got['ads'][name].add(
            mm.read16(mm.ss, mm.regs['sp'] + AT_CALL) - oracle.TICK_SCRIPT)

    def frame(mm):
        bx = mm.read16(mm.ds, 0x263E)
        ttm = files.get(mm.read16(mm.ds, bx))
        if ttm:
            got['frames'][ttm].add(mm.read16(mm.ds, bx + 8))
            got['scenes'].add((ttm, mm.read16(mm.ds, bx + 2)))

    m.watch(10, 0x1519, instruction)
    m.watch_at(12, 0x040C, frame)
    for _ in range(ticks):
        world.tick()
    return got


def work(which):
    """Every unit of work a source is made of, so they can run in parallel."""
    if which == 'seqrun':
        return do_seqrun, [(name, seq, 8)
                           for name, code in oracle.ads_scripts()
                           for seq in sorted(oracle.ads_layout(code))]
    if which == 'seqpass':
        return do_seqpass, [(name, seq, states)
                            for name, _c, _l, seq, _n, states
                            in oracle._seqpass_cases()]
    if which == 'ttm':
        return do_ttm, [(name,) for name, _code in oracle.ttm_scripts()]
    if which == 'tick':
        return do_tick, oracle.tick_cases()
    raise SystemExit('no such source: %s' % which)


SOURCES = ['seqrun', 'seqpass', 'ttm', 'tick']


def report(got, ads, frames, scenes):
    rows = [
        ('ADS instructions', sum(len(got['ads'][n] & ads[n]) for n in ads),
         sum(len(v) for v in ads.values())),
        ('TTM frames', sum(len(got['frames'][n]) for n in frames),
         sum(frames.values())),
        ('scenes played', len(got['scenes'] & scenes), len(scenes)),
    ]
    worst = 1.0
    for label, hit, have in rows:
        pct = hit / have if have else 0.0
        worst = min(worst, pct)
        print('  %-20s %5d of %5d   %5.1f%%' % (label, hit, have, pct * 100))
    return worst


def main():
    wanted = sys.argv[1:] or SOURCES
    ads, frames, scenes = totals()
    total = blank()
    jobs = min(4, os.cpu_count() or 1)
    for which in wanted:
        fn, items = work(which)
        got = blank()
        with multiprocessing.Pool(jobs) as pool:
            for n, one in enumerate(pool.imap_unordered(fn, items), 1):
                merge(got, one)
                print('  %s %d/%d' % (which, n, len(items)),
                      file=sys.stderr, flush=True)
        print('%s:' % which)
        report(got, ads, frames, scenes)
        merge(total, got)
    if len(wanted) > 1:
        print('\nall of them together:')
        worst = report(total, ads, frames, scenes)
        print()
        return 0 if worst >= 0.95 else 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
