"""Check the port against the original's own code, not against my own reasoning.

`tools/difftest.sh` compares the Rust decoder with the Python one. Both came
out of the same reading of the same disassembly, so a shared misunderstanding
passes it clean. This is different: it runs the functions inside
`SCRANTIC.EXE` in the 16-bit interpreter in `tools/x86.py`, asks the port the
same questions through `jcoracle`, and diffs. A disagreement is a bug in the
port, full stop -- there is no interpretation involved and no opinion of mine
on either side.

    python3 tools/oracle.py                    # everything
    python3 tools/oracle.py rand choose        # just those

Anything the interpreter cannot execute raises rather than guessing, so a gap
shows up as a failure and not as a false pass.
"""

import subprocess
import sys

sys.path.insert(0, 'tools')
import x86

EXE = 'original/scrantic.exe'
PORT = './johnny/target/release/jcoracle'

# Where things live in the data segment.
SCENES = 0x1756          # the scene table, 0x11 bytes a row (seg4:0x0DC4)
ROW = 0x11
ISLAND_STATE = 0x30C0    # seg3:0x1C41 writes it, seg4:0x0EDB reads it
STORY_DAY = 0x2D9B       # the story day on offer
STORY_REACHED = 0x2D9C   # the story day reached; the chooser copies it over
QUEUE = 0x30F4           # the queue itself, a null-terminated pointer array
LAST_GAG = 0x30E8        # what closed the last queue
LAST_MATE = 0x30EA
STORY_FLAG = 0x30EE
LAYOUT = 0x30C2          # the rock-and-sandbar coin
ISLAND_X = 0x30C6        # where seg3:0x1B00 leaves the island
ISLAND_Y = 0x30C4


def port(*args):
    out = subprocess.run([PORT] + [str(a) for a in args],
                         capture_output=True, text=True)
    if out.returncode != 0:
        raise SystemExit('jcoracle %s failed: %s' % (args, out.stderr.strip()))
    return out.stdout.splitlines()


def fresh():
    return x86.Machine(EXE)


# ---- the questions -------------------------------------------------------

def q_rand(n=256):
    """seg4:0x1E86, the engine's rand()."""
    m = fresh()
    return ['rand %d %d' % (i, m.call(4, 0x1E86)) for i in range(n)]


def q_weighted(weights=(10, 20, 30, 20, 10, 10), n=256):
    """seg4:0x1F1B, the weighted chooser everything else is built on."""
    m = fresh()
    args = list(weights) + [0]
    return ['weighted %d %d' % (i, m.call(4, 0x1F1B, args)) for i in range(n)]


def q_phase():
    """seg4:0x0554's arithmetic: half hours from dawn to now, mod sixteen.

    Every half hour against every dawn, which is 2,304 answers -- small enough
    to be exhaustive rather than a sample.
    """
    m = fresh()
    out = []
    for h in range(24):
        for mm in (0, 30):
            for dh in range(24):
                for dm in (0, 30):
                    now, dawn = h * 100 + mm, dh * 100 + dm
                    a = m.call(4, 0x0C48, [now])
                    b = m.call(4, 0x0C48, [dawn])
                    out.append('phase %d %d %d' % (now, dawn, (a - b + 16) % 16))
    return out


def q_choose(category=1, phase=0, budget=300, n=256):
    """seg4:0x0D76, the scene chooser, reported as a row number."""
    m = fresh()
    m.write16(m.ds, ISLAND_STATE, 0xFFFF)   # island unconstrained, as the port starts
    out = []
    for i in range(n):
        ptr = m.call(4, 0x0D76, [category, phase, budget])
        row = -1 if ptr == 0 else (ptr - SCENES) // ROW
        out.append('choose %d %d' % (i, row))
    return out


def q_queue(phase=0, n=16):
    """seg4:0x08B9, the queue builder -- the whole outer loop in one call.

    The queue lands in the pointer array at `ds:0x30F4`, null terminated, so
    it reads back as a list of row numbers.
    """
    m = fresh()
    m.write16(m.ds, ISLAND_STATE, 0xFFFF)
    m.write16(m.ds, LAST_GAG, 0)
    m.write16(m.ds, LAST_MATE, 0)
    m.write16(m.ds, STORY_FLAG, 0)
    m.write8(m.ds, STORY_DAY, 1)
    m.write8(m.ds, STORY_REACHED, 1)
    out = []
    for i in range(n):
        # `seg4:0x0633` resets the island before every build.
        m.call(3, 0x1C41, [0xFFFF, 0xFFFF])
        m.call(4, 0x08B9, [phase, 300])
        rows = []
        k = 0
        while True:
            ptr = m.read16(m.ds, QUEUE + k * 2)
            if ptr == 0:
                break
            rows.append(str((ptr - SCENES) // ROW))
            k += 1
            if k > 400:
                raise RuntimeError('queue never terminated')
        out.append('queue %d %s' % (i, ','.join(rows)))
    return out


def q_place(n=64):
    """seg3:0x1B00, where the island gets dropped.

    Every island state that means anything, with both layouts, and the
    resulting offset -- which is what the whole picture is drawn against.
    """
    m = fresh()
    out = []
    states = [0xFFFF, 0, 1, 2, 4, 8, 0x10, 0x20, 0x0C, 0x30, 0x14, 0x28]
    for state in states:
        for layout in (0, 1):
            for i in range(n):
                m.write16(m.ds, ISLAND_STATE, state)
                m.write16(m.ds, LAYOUT, layout)
                m.call(3, 0x1B00)
                x = m.read16(m.ds, ISLAND_X)
                y = m.read16(m.ds, ISLAND_Y)
                out.append('place %d %d %d %d %d' % (state, layout, i, x, y))
    return out


QUERIES = {
    'rand': (q_rand, lambda: port('rand', 256)),
    'weighted': (q_weighted, lambda: port('weighted', '10,20,30,20,10,10', 256)),
    'phase': (q_phase, lambda: port('phase')),
    'choose': (q_choose, lambda: port('choose', 1, 0, 300, 256)),
    'choose2': (lambda: q_choose(2, 6, 200), lambda: port('choose', 2, 6, 200, 256)),
    'choose4': (lambda: q_choose(4, 12, 50), lambda: port('choose', 4, 12, 50, 256)),
    'queue': (q_queue, lambda: port('queue', 0, 16)),
    'queue6': (lambda: q_queue(6, 16), lambda: port('queue', 6, 16)),
    'place': (q_place, lambda: port('place', 64)),
}


def main():
    wanted = sys.argv[1:] or list(QUERIES)
    failures = 0
    for name in wanted:
        if name not in QUERIES:
            print('  %-10s no such query' % name)
            failures += 1
            continue
        original, mine = QUERIES[name]
        try:
            want = original()
        except Exception as e:                      # noqa: BLE001 - report, don't crash
            print('  %-10s the interpreter could not run it: %s' % (name, e))
            failures += 1
            continue
        got = mine()
        if want == got:
            print('  %-10s %4d answers, all identical' % (name, len(want)))
            continue
        failures += 1
        bad = [(a, b) for a, b in zip(want, got) if a != b]
        print('  %-10s %d of %d differ' % (name, len(bad) + abs(len(want) - len(got)),
                                           len(want)))
        for a, b in bad[:5]:
            print('       original: %s' % a)
            print('       port:     %s' % b)
        if len(want) != len(got):
            print('       lengths differ: %d vs %d' % (len(want), len(got)))

    print()
    if failures:
        print('%d of %d disagree with the binary' % (failures, len(wanted)))
        return 1
    print('the port agrees with the binary on all %d' % len(wanted))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
