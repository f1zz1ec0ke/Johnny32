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
SCRATCH = 0x8000         # free space in the data segment for a script
CTX = 0x7000             # a scratch context; only its end pointer is read
WALK_DELAY = 0x30D6      # ticks left before the next waypoint
ROUTE = 0x30D2           # the route block in play
PATH = 0x30D0            # the walk path in play
PENDING = 0x30DE
MAJOR = 0x30E4           # where Johnny is
MINOR = 0x30E0
TARGET = 0x30E2          # where he is going
TURN_GOAL = 0x30CA       # the facing he is turning toward
POSE = 0x30C8            # what to draw: a standing pose or a walk waypoint


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


def q_adslen():
    """seg10:0x0303, walked over every shipped script.

    The engine's own instruction-length function, driven along each script
    from the top. If the port and the binary agree on where every instruction
    begins, the whole ADS decoder rests on something checked rather than on a
    hand-written table that happens to consume each file exactly.

    The function bounds-checks against the context's end pointer, so a scratch
    context is put up with that one field filled in.
    """
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    order = ['ACTIVITY.ADS', 'BUILDING.ADS', 'FISHING.ADS', 'JOHNNY.ADS',
             'MARY.ADS', 'MISCGAG.ADS', 'STAND.ADS', 'SUZY.ADS',
             'VISITOR.ADS', 'WALKSTUF.ADS']
    bodies = dict((n, b) for n, b, _ in recs)
    out = []
    for name in order:
        body = bodies.get(name)
        if body is None:
            continue
        code = None
        for ch in dgds.parse_chunks(body):
            if ch[0] != 'ADS:':
                continue
            for c2 in dgds.parse_chunks(ch[1]):
                if c2[0] == 'SCR:':
                    try:
                        code = dgds.decompress(c2[1])
                    except Exception:
                        code = c2[1]
        if not code:
            continue
        m = fresh()
        base = m._lin(m.ds, SCRATCH)
        m.mem[base:base + len(code)] = code
        m.write16(m.ds, CTX + 8, SCRATCH + len(code))
        m.write16(m.ds, 0x2608, 0)
        m.write16(m.ds, 0x460E, CTX)
        m.write16(m.ds, 0x4610, m.ds)
        p = SCRATCH
        while p < SCRATCH + len(code):
            op = m.read16(m.ds, p)
            nxt = m.call(10, 0x0303, [p, m.ds])
            if not nxt:
                break
            out.append('adslen %s %d %04X %d' % (name, p - SCRATCH, op, nxt - p))
            p = nxt
    return out


def q_route(n=32):
    """seg4:0x0094, the walk driver, stopped the moment it has picked a step.

    The driver looks the route block up (`seg4:0x03AC`), scans it for where
    Johnny is standing (`seg4:0x00EA`), draws `rand() % 100` and subtracts
    weights. It then goes on to turn him on the spot and animate the walk, so
    a hook on the step-to-path lookup answers the question and stops the
    machine there.

    Every (block, position) the six-by-six matrix offers, rolled `n` times.
    """
    import subprocess
    rows = subprocess.run(
        [PORT, 'routes'], capture_output=True, text=True).stdout.split()
    m = fresh()

    def grab(mm):
        raise x86.Answer(mm.read16(mm.ss, mm.sp))

    m.hook(4, 0x03EA, grab)
    out = []
    for row in rows:
        frm, to, at = (int(v) for v in row.split(','))
        for i in range(n):
            for off, v in [(WALK_DELAY, 0), (ROUTE, 0), (PATH, 0),
                           (PENDING, 0xFFFF), (MAJOR, at), (MINOR, 0),
                           (TARGET, to)]:
                m.write16(m.ds, off, v)
            # The block is the one for the pair he set out on, not the pair he
            # has got to: `seg4:0x0094` only looks it up when there is none.
            m.write16(m.ds, ROUTE, m.call(4, 0x03AC, [frm, to]))
            out.append('route %d %d %d %d %d' % (frm, to, at, i, m.call(4, 0x0094)))
    return out


def q_turn(n=8):
    """seg4:0x01D6, which way Johnny pivots.

    Every facing against every other, `n` times each, so the coin toss the
    engine makes when the two are dead opposite is compared draw for draw
    rather than in aggregate -- it comes out of the same generator everything
    else does, so getting it wrong moves every later decision as well.

    The driver is run with a route and a path already in play, so it goes
    straight to the turn; a hook on the blitter answers with the new facing
    and stops it before it draws anything.
    """
    m = fresh()
    # A path with room to spare, so a turn that finishes does not run off the
    # end of it and overwrite the answer with the arrival.
    for k in range(6):
        m.write16(m.ds, SCRATCH + k * 6 + 0, 1)
        m.write16(m.ds, SCRATCH + k * 6 + 2, 100)
        m.write16(m.ds, SCRATCH + k * 6 + 4, 100)
    m.hook(13, 0x007E, lambda mm: (_ for _ in ()).throw(
        x86.Answer(mm.read16(mm.ds, MINOR))))
    m.hook(4, 0x03EA, lambda mm: mm.regs.__setitem__('ax', SCRATCH))
    out = []
    for at in range(8):
        for to in range(8):
            for i in range(n):
                for off, v in [(WALK_DELAY, 0), (ROUTE, 1), (PATH, SCRATCH),
                               (PENDING, 0xFFFF), (POSE, SCRATCH),
                               (MAJOR, 1), (TARGET, 1), (MINOR, at), (TURN_GOAL, to)]:
                    m.write16(m.ds, off, v)
                out.append('turn %d %d %d %d' % (at, to, i, m.call(4, 0x0094)))
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
    'adslen': (q_adslen, lambda: port('adslen')),
    'route': (q_route, lambda: port('route', 32)),
    'turn': (q_turn, lambda: port('turn', 8)),
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
