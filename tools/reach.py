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
  against the 2,308 in the ten scripts it can ever be handed. This is the
  branch number -- a guard that never fired, a random block that always went
  one way, and a sequence nothing ever started are all instructions that
  never ran.
* **TTM frames.** Every one `seg12:0x040C` is handed, against all 6,631.
* **Scenes.** Every `1111` marker whose frame ran, against all 611 -- and,
  separately and not as a gate, how many of them the scheduler itself chose.

The sources are the queries that *execute* something. `blocks` and `adslen`
are not here: they ask a static question at every instruction in the data, so
they cover all 2,688 by construction and would flatter the number without
telling anyone anything.

The denominator is the other half of not flattering it. Of the 2,688 words in
the scripts, 380 are structure the dispatcher is never handed at all -- the
sequence headers, the `FFFF` terminators, the two-word joined conditions, the
random-block separators, and the words whose opcode has no handler in the
table at `cs:0x1875`. Counting those as unreached branches would be inventing
a gap; quietly dropping the ones a run happened to miss would be inventing a
pass. So `structural()` reads the dispatch table out of the binary, classifies
every word with the reason it is set aside, and `--missing` prints whatever is
left over by opcode, so the shortfall can be looked at rather than taken on
trust.
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


# The opcode table `seg10:0x1519` searches, and the handlers that follow it.
# The gap is the table: 0x58 bytes of words, so forty-four entries, and that
# is a fact about the binary rather than a count I made by eye.
TABLE, HANDLERS = 0x1875, 0x18CD


def dispatch_table():
    """The opcodes `seg10:0x1519` has a handler for, out of the binary.

    An opcode that is not in it walks off the end of the search at
    `seg10:0x152C` and nothing happens, so it can never be a branch that went
    unexercised.
    """
    import x86
    m = x86.Machine(oracle.EXE)
    base = m._lin(10, 0)
    return set(struct.unpack_from('<H', m.mem, base + TABLE + i * 2)[0]
               for i in range((HANDLERS - TABLE) // 2))


# Words that are in the scripts and that the dispatcher can never be handed.
# Every one of these was checked against the interpreter rather than inferred
# from the fact that a run did not reach it -- which is the whole difference
# between a denominator and an excuse.
JOINERS = (0x1420, 0x1430)


def structural(code, table):
    """The words in one script that are structure rather than instructions."""
    out = {}
    ops = list(oracle.ads_walk(code))
    # The headers as `oracle.ads_layout` finds them, rather than by value
    # again here -- the two have to be the same set or the sequencer is being
    # measured against a layout it does not use.
    heads = set(off - 2 for off, _pairs in oracle.ads_layout(code).values())
    joined = False
    for off, op, _a in ops:
        why = None
        if off in heads:
            why = 'a sequence header; the interpreter is entered at the body'
        elif op == 0xFFFF:
            why = 'a terminator; `seg10:0x195D` returns without dispatching'
        elif op not in table:
            why = 'no handler in the table at `cs:0x1875`'
        elif op in JOINERS:
            why = 'a joiner, consumed inside `seg10:0x13E7`'
        elif joined:
            why = ('the second half of a joined condition, consumed inside '
                   '`seg10:0x1402`')
        elif op == 0x3020:
            why = 'a random-block separator, consumed by `seg10:0x0CDA`'
        joined = op in JOINERS
        if why:
            out[off] = why
    return out


def totals():
    """What there is to reach, once the structure is set aside."""
    table = dispatch_table()
    ads, dead, aside = {}, collections.Counter(), {}
    for name, code in oracle.ads_scripts():
        skip = structural(code, table)
        for why in skip.values():
            dead[why] += 1
        aside[name] = skip
        ads[name] = set(off for off, _op, _a in oracle.ads_walk(code)
                        if off not in skip)
    totals.dead = dead
    totals.aside = aside
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
            'scenes': set(),
            'played': set()}


def merge(into, other):
    for name, hits in other['ads'].items():
        into['ads'][name] |= hits
    for name, hits in other['frames'].items():
        into['frames'][name] |= hits
    into['scenes'] |= other['scenes']
    into['played'] |= other['played']
    return into


# ---- the sources ---------------------------------------------------------

def do_seqrun(item):
    """One sequence run over and over, the way `seqrun` runs it."""
    name, seq, passes, seed = item
    code = dict(oracle.ads_scripts())[name]
    layout = oracle.ads_layout(code)
    got = blank()
    m = oracle.fresh()
    for _ in range(seed):
        m.call(4, 0x1E86)
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
    # The same pairs `q_seqpass` sets up, or this counts guards firing in a
    # world the compared run never had.
    oracle.ads_pass(m, code, layout, seq, states, oracle.ads_all_pairs(layout))
    return got


def do_ttm(item):
    """Every frame of one script, the way `ttm` runs them: one at a time,
    with nothing else on screen.

    Both rows are what the engine did, not what the loop asked for. A frame
    counts when `seg12:0x040C` was actually entered for it, and a scene counts
    when its `1111` reached the dispatcher at `seg12:0x0620` -- so a frame the
    runner declines and a marker inside one are misses, which is the whole
    point of measuring instead of asserting.
    """
    name, = item
    code = dict(oracle.ttm_scripts())[name]
    got = blank()
    e = oracle.Interpreter()
    ran = []
    e.m.watch_at(12, 0x040C, lambda _mm: ran.append(True))
    n = e.load(code)
    for k in range(n):
        del ran[:]
        ops, _draws, _record = e.frame(k)
        if not ran:
            continue
        got['frames'][name].add(k)
        for line in ops:
            bits = line.split()
            if len(bits) >= 3 and bits[1] == '1111':
                got['scenes'].add((name, int(bits[2])))
    return got


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
            scene = (ttm, mm.read16(mm.ds, bx + 2))
            got['frames'][ttm].add(mm.read16(mm.ds, bx + 8))
            got['scenes'].add(scene)
            got['played'].add(scene)

    m.watch(10, 0x1519, instruction)
    m.watch_at(12, 0x040C, frame)
    for _ in range(ticks):
        world.tick()
    return got


def work(which):
    """Every unit of work a source is made of, so they can run in parallel."""
    if which == 'seqrun':
        passes, seeds = oracle.seqrun_shape()
        return do_seqrun, [(name, seq, passes, seed)
                           for name, code in oracle.ads_scripts()
                           for seq in sorted(oracle.ads_layout(code))
                           for seed in range(seeds)]
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

# What the union has to reach before this passes, and the tier that number is
# about. The default tier runs a fraction of the cases so the measurement
# itself can be smoke-tested in a couple of minutes; it is not a coverage
# claim and is not gated. `JC_DEEP=1` runs the case lists the deep oracle
# runs, and that is the one the floor is set against.
FLOOR = 0.95


def report(got, ads, frames, scenes):
    """The three gated rows, and one that is reported for contrast.

    The last row is the scenes the *scheduler* picked, and it is deliberately
    not a gate. A scene is compared frame by frame against the binary as soon
    as anything runs it, and `ttm` runs every frame of every script -- so the
    row above it is the coverage claim. This one says something narrower and
    much more variable: how many scenes the tick sweep's eighty starting
    points happened to select inside their hundred and twenty ticks. Gating
    on it would be gating on the story's own dice.
    """
    rows = [
        ('ADS instructions', sum(len(got['ads'][n] & ads[n]) for n in ads),
         sum(len(v) for v in ads.values()), True),
        ('TTM frames', sum(len(got['frames'][n]) for n in frames),
         sum(frames.values()), True),
        ('TTM scenes run', len(got['scenes'] & scenes), len(scenes), True),
        ('of those, scheduled', len(got['played'] & scenes), len(scenes),
         False),
    ]
    worst = 1.0
    for label, hit, have, gated in rows:
        pct = hit / have if have else 0.0
        if gated:
            worst = min(worst, pct)
        print('  %-20s %5d of %5d   %5.1f%%%s'
              % (label, hit, have, pct * 100, '' if gated else '   (not gated)'))
    return worst


def missing(got, ads):
    """The executable words nothing ran, by opcode.

    A number on its own does not say whether the gap is a handful of scripts
    nothing starts or one guard in every file that never fires, and those are
    very different findings. This says which.
    """
    by_op = collections.Counter()
    where = collections.defaultdict(set)
    for name, code in oracle.ads_scripts():
        left = ads[name] - got['ads'][name]
        for off, op, _a in oracle.ads_walk(code):
            if off in left:
                by_op[op] += 1
                where[op].add(name)
    if not by_op:
        print('  nothing -- every executable word ran')
        return
    for op, n in by_op.most_common():
        print('  %04X  %4d  in %d of the ten scripts'
              % (op, n, len(where[op])))
    print('  ----  %4d' % sum(by_op.values()))


def wrongly_aside(got):
    """Anything the run dispatched that the denominator excused.

    `totals()` sets 380 words aside on the strength of a reading of the
    interpreter. This is the reading being wrong: a word that was excused and
    then handed to the dispatcher anyway. Nothing else in the harness would
    notice, because an excused word is not in the denominator and not in the
    numerator either -- it would just quietly make every percentage larger.
    """
    out = collections.defaultdict(set)
    for name, code in oracle.ads_scripts():
        ops = dict((off, op) for off, op, _a in oracle.ads_walk(code))
        for off, why in totals.aside[name].items():
            if off in got['ads'][name]:
                out[name].add((off, why, ops[off]))
    return dict(out)


def main():
    args = sys.argv[1:]
    show = '--missing' in args
    wanted = [a for a in args if not a.startswith('-')] or SOURCES
    deep = bool(os.environ.get('JC_DEEP'))
    ads, frames, scenes = totals()
    total = blank()
    # Two of these runs share a machine with the oracle itself often
    # enough to be worth a knob.
    jobs = int(os.environ.get('JC_JOBS') or 0) or min(4, os.cpu_count() or 1)
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
    else:
        # One source on its own is a diagnostic, not the claim, so it is
        # printed above and nothing is gated on it.
        worst = None
    if show:
        print('\nnever handed to `seg10:0x1519`:')
        missing(total, ads)
    # The denominator says 380 words can never be dispatched. If one of them
    # just was, that is not a rounding difference -- it is a wrong
    # denominator, and every percentage above it was inflated by exactly the
    # words it wrongly excused. So it fails rather than being noted.
    wrong = wrongly_aside(total)
    if wrong:
        print('\n%d words set aside as structure were dispatched anyway:'
              % sum(len(v) for v in wrong.values()))
        for name, hits in sorted(wrong.items()):
            for off, why, op in sorted(hits):
                print('  %s +%04X  %04X  was called %s' % (name, off, op, why))
        return 1
    print()
    if not deep:
        print('the default tier, which is a check of the measurement rather '
              'than of coverage;\nJC_DEEP=1 runs the cases the %.0f%% is '
              'claimed against.' % (FLOOR * 100))
        return 0
    if worst is None:
        return 0
    return 0 if worst >= FLOOR else 1


if __name__ == '__main__':
    sys.exit(main())
