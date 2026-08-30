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

import os
import struct
import subprocess
import sys
import zlib

sys.path.insert(0, 'tools')
import x86
from x86 import _s16

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


def q_repeat():
    """seg10:0x0DB6, over every repeat flag a `PlayScene` could carry.

    The flags operand decides how long a scene stays: zero once, positive that
    many times, negative for that many ticks. All three write a different run
    state, and two of them write a second field as well -- one less than the
    count, or the tick the deadline falls on. `seqrun` compares the states
    already; this compares what goes with them.
    """
    m = fresh()
    inst = ADS_INSTS
    # The engine reads the 32-bit tick counter for a deadline.
    m.write16(m.ds, 0x2E72, 1000)
    m.write16(m.ds, 0x2E74, 0)
    out = []
    for flags in [0, 1, 2, 3, 7, 100, 0x7FFF, -1, -2, -60, -300, -0x7FFF]:
        for k in range(0, 0x40, 2):
            m.write16(m.ds, inst + k, 0)
        m.write16(m.ds, inst + 0x00, 9)
        m.write16(m.ds, inst + 0x02, 9)
        m.write16(m.ds, inst + 0x3B, 0)
        m.write16(m.ds, ADS_CTX + 0x10, inst)
        m.write16(m.ds, ADS_CURCTX, 0)
        m.write16(m.ds, ADS_CTXPTR, ADS_CTX)
        m.write16(m.ds, ADS_CTXPTR + 2, m.ds)
        m.call(10, 0x0DB6, [9, 9, flags & 0xFFFF])
        deadline = (m.read16(m.ds, inst + 0x35) << 16) | m.read16(m.ds, inst + 0x33)
        out.append('repeat %d %d %d %d' % (
            flags, m.read16(m.ds, inst + 0x2F), m.read16(m.ds, inst + 0x31), deadline))
    return out


def q_holiday():
    """seg4:0x05B5, over every date in a year.

    Four decorations, on dates that between them cover eleven days out of
    three hundred and sixty-five -- so nobody would ever notice this being
    wrong. The engine reads the date through the C runtime's `getdate`, which
    is hooked here so the day can be handed in.
    """
    m = fresh()
    date = [0, 0]

    def getdate(mm):
        at = mm.read16(mm.ss, mm.sp)
        mm.write8(mm.ds, at, date[1])          # day
        mm.write8(mm.ds, at + 1, date[0])      # month
        mm.write16(mm.ds, at + 2, 1992)

    m.hook(1, 0x01F4, getdate)
    m.hook(1, 0x020F, lambda mm: None)
    # `seg4:0x054F` refreshes the screen before it looks at the date.
    m.hook(4, 0x0673)
    # `seg4:0x0540` goes on to refresh the whole screen, which is not what is
    # being asked about. `seg4:0x062B` is where all five arms of the date
    # check converge, so the answer exists there and the machine can stop.
    m.watch_at(4, 0x062B, lambda mm: (_ for _ in ()).throw(
        x86.Answer(mm.read16(mm.ds, 0x30BC))))
    days = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    out = []
    for month in range(1, 13):
        for day in range(1, days[month - 1] + 1):
            date[0], date[1] = month, day
            m.write16(m.ds, 0x30BC, 0xFFFF)
            out.append('holiday %d %d %d' % (month, day, m.call(4, 0x0540)))
    return out


def q_backdrop():
    """seg3:0x1C7C, over every combination of the six backdrop bits.

    A scene's table row asks for the island a particular way -- no raft, flat
    calm, night, a cutaway, no decoration, no rock -- and six bits of the low
    word say which. Two of them share one variable (`ds:0x30B4`: 1 for night,
    2 for a cutaway) and the cutaway is written second, so a row asking for
    both gets a cutaway and *not* a night. Sixty-four combinations is small
    enough to settle that rather than reason about it.
    """
    m = fresh()
    bits = [0x0010, 0x0200, 0x0800, 0x1000, 0x2000, 0x4000]
    out = []
    for combo in range(64):
        mask = 0
        for k, b in enumerate(bits):
            if combo & (1 << k):
                mask |= b
        m.call(3, 0x1C41, [0xFFFF, 0xFFFF])          # reset
        m.call(3, 0x1C41, [mask, 0])
        out.append('backdrop %04X raft=%d water=%d light=%d holiday=%d layout=%d' % (
            mask,
            m.read16(m.ds, 0x30B0),
            m.read16(m.ds, 0x30B2),
            m.read16(m.ds, 0x30B4),
            m.read16(m.ds, 0x30B6),
            0 if m.read16(m.ds, 0x30C2) == 0 else 1))
    return out


def q_island():
    """seg4:0x0EDB and seg3:0x1C41, exhaustively.

    A scene's table row carries six bits saying which parts of the water it
    needs free, and the queue keeps a running state of what has been claimed.
    `seg4:0x0EDB` says whether a scene can still join, and `seg3:0x1C41`
    folds it in when one does. Every state against every combination of
    wishes -- 64 by 64 -- is small enough to be exhaustive rather than
    sampled, and this is a bitmask machine, which is exactly the kind that
    passes a spot check and fails somewhere in the middle.
    """
    m = fresh()
    out = []
    for state in list(range(64)) + [0xFFFF]:
        for want in range(64):
            m.write16(m.ds, ISLAND_STATE, state)
            # The mask is a long: low word first, and the six wish bits
            # are in the high one (`seg4:0x0EF3` reads `[bp+8]`).
            allowed = m.call(4, 0x0EDB, [0, want])
            out.append('island allow %d %d %d' % (state, want, 1 if allowed else 0))
    for state in list(range(64)) + [0xFFFF]:
        for want in range(64):
            m.write16(m.ds, ISLAND_STATE, state)
            m.call(3, 0x1C41, [0, want])
            out.append('island take %d %d %d'
                       % (state, want, m.read16(m.ds, ISLAND_STATE)))
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


# A fabricated ADS context, and where its pieces live.
ADS_CTX = 0x7000
ADS_SCRIPT = 0x8000
ADS_INSTS = 0xB000
ADS_CTXPTR = 0x460E      # ds:0x460E[i*4] / 0x4610[i*4] -> context i
ADS_CURCTX = 0x2608
ADS_CURSEQ = 0x2606

# Each action handler near-calls a leaf, and every one of those call sites is
# `push cs; call near` -- so at the hook the segment sits on top of the
# operands and the first one is two bytes further in.
ADS_ACTIONS = {
    0x0DB6: ('play', 3),      # 2005, and what 2000's handler itself calls
    0x0E9B: ('stop', 2),      # 2010
    0x0E6E: ('finish', 2),    # 2015
    0x0EC8: ('reset', 2),     # 2020
    0x0F8C: ('front', 2),     # 4000
    0x0EF2: ('back', 2),      # 4010
    0x19E7: ('setseq', 1),    # F000 / F200 / F210
}


def ads_scripts():
    """Every shipped `.ADS`, decompressed, with its sequence entry points."""
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    out = []
    for name, body, _ in recs:
        if not name.endswith('.ADS'):
            continue
        code = None
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == 'SCR:':
                try:
                    code = dgds.decompress(payload)
                except Exception:               # noqa: BLE001 - stored raw
                    code = payload
        if code:
            out.append((name, code))
    return sorted(out)


def ads_walk(code):
    """(offset, opcode, operands) for every instruction, using the port's
    length rule -- which `adslen` has already checked against the binary."""
    p = 0
    while p + 2 <= len(code):
        op = int.from_bytes(code[p:p + 2], 'little')
        n = _ads_len(op)
        yield p, op, [int.from_bytes(code[p + 2 + k * 2:p + 4 + k * 2], 'little')
                      for k in range((n - 2) // 2)]
        p += n


def _ads_len(op):
    if op in (0x2000, 0x2005): return 10
    if op in (0x2010, 0x2015, 0x2020, 0x4000, 0x4010): return 8
    if (0x1010 <= op <= 0x1070 or 0x1310 <= op <= 0x1370) and op & 0xF == 0: return 6
    if op in (0xF010, 0xF200, 0xF210, 0x1080, 0x1380, 0x1390, 0x3020): return 4
    if op in (0x13A0, 0x13A1, 0x13B0, 0x13B1, 0x13C0, 0x13C1): return 4
    return 2


def ads_layout(code):
    """Sequence id -> (body offset, the (ttm, scene) pairs it mentions)."""
    seqs, current, pairs = {}, None, {}
    for off, op, a in ads_walk(code):
        if 0 < op <= 0xFF:
            current = op
            seqs[op] = off + 2
            pairs[op] = []
        elif current is not None and len(a) >= 2 and (
                0x1010 <= op <= 0x1080 or 0x1310 <= op <= 0x1370
                or op in (0x2000, 0x2005, 0x2010, 0x2015, 0x2020, 0x4000, 0x4010)):
            if (a[0], a[1]) not in pairs[current]:
                pairs[current].append((a[0], a[1]))
    return {k: (seqs[k], pairs[k]) for k in seqs}


def ads_owner(bodies, off):
    """The sequence an offset is in: the nearest body at or before it.

    `bodies` is sorted by offset, so this is the last one starting before
    `off` -- not the highest-numbered. Sequence ids are not in offset order,
    and taking the largest puts a rule in whichever sequence happens to be
    numbered highest, which has already been wrong twice.
    """
    owner = None
    for start, seq in bodies:
        if start <= off:
            owner = seq
    return owner


def ads_bodies(layout):
    return sorted((body, seq) for seq, (body, _p) in layout.items())


def ads_random(code, layout):
    """The sequences with a `30xx` in them, which are the only ones a draw
    can change the behaviour of."""
    bodies = ads_bodies(layout)
    return set(ads_owner(bodies, off) for off, op, _a in ads_walk(code)
               if 0x3010 <= op < 0x3100)


def ads_pass(machine, code, seqs, seq, states, pairs, extra=None):
    """One pass of one sequence inside the original interpreter.

    Every scene the script mentions gets an instance record up front, so the
    handlers find one and take their "already there" path -- which is nothing
    but state writes. Nothing below the sequencer is stubbed: the real
    `seg10:0x0DB6` decides what a `PlayScene` does to a scene's state, and the
    predicates on the next instruction read what it actually wrote.
    """
    m = machine
    base = m._lin(m.ds, ADS_SCRIPT)
    m.mem[base:base + len(code)] = code
    m.write16(m.ds, ADS_CURCTX, 0)
    m.write16(m.ds, ADS_CTXPTR, ADS_CTX)
    m.write16(m.ds, ADS_CTXPTR + 2, m.ds)
    m.write16(m.ds, ADS_CTX + 0x02, max(seqs) + 1)
    m.write16(m.ds, ADS_CTX + 0x08, ADS_SCRIPT + len(code))
    for sid, (off, _) in seqs.items():
        m.write16(m.ds, ADS_CTX + 0x152 + sid * 4, ADS_SCRIPT + off)
        m.write16(m.ds, ADS_CTX + 0x154 + sid * 4, m.ds)
        m.write16(m.ds, ADS_CTX + 0x12 + sid * 2, 1 if sid == seq else 0)
        m.write16(m.ds, ADS_CTX + 0x292 + sid * 4, 0)
        m.write16(m.ds, ADS_CTX + 0x294 + sid * 4, 0)
        m.write16(m.ds, ADS_CTX + 0x0B2 + sid * 2, 0)
    head, at_of = 0, {}
    for n, (ttm, scene) in enumerate(pairs):
        at = ADS_INSTS + n * 0x40
        for k in range(0, 0x40, 2):
            m.write16(m.ds, at + k, 0)
        played, state = states.get((ttm, scene), (0, 0))
        m.write16(m.ds, at + 0x00, ttm)
        m.write16(m.ds, at + 0x02, scene)
        m.write16(m.ds, at + 0x2D, played)
        m.write16(m.ds, at + 0x2F, state)
        m.write16(m.ds, at + 0x3B, head)
        head = at
        at_of[(ttm, scene)] = at
    m.write16(m.ds, ADS_CTX + 0x10, head)
    m.write16(m.ds, ADS_CURSEQ, seq)

    log = []

    def record(label, argc):
        def watcher(mm):
            args = [mm.read16(mm.ss, mm.sp + 2 + k * 2) for k in range(argc)]
            log.append('%s %s' % (label, ' '.join(str(a) for a in args)))
        return watcher

    m.watches.clear()
    for off, (label, argc) in ADS_ACTIONS.items():
        m.watch(10, off, record(label, argc))
    # For digging: `extra` sees every instruction the dispatcher is handed.
    if extra is not None:
        m.watch(10, 0x1519, extra)
    m.call(10, 0x1925, [ADS_SCRIPT + seqs[seq][0], m.ds])

    # What the pass left behind: the sequence states, and each scene's.
    tail = ['seq%d=%d' % (sid, m.read16(m.ds, ADS_CTX + 0x12 + sid * 2))
            for sid in sorted(seqs)]
    tail += ['%d/%d=%d' % (t, s, m.read16(m.ds, at_of[(t, s)] + 0x2F))
             for (t, s) in pairs]
    return log + ['|'] + tail


def q_blocks():
    """seg10:0x106B, asked where the block starting at every instruction ends.

    This is the scanner a refused guard uses to swallow its block. If the port
    stops short of where the engine stops, everything between the two runs as
    though it were the next rule -- several scenes starting that the guard had
    just said no to. Asking it at *every* instruction rather than only at the
    guards makes the answer a property of the script rather than of the walk
    that reached it.
    """
    out = []
    for name, code in ads_scripts():
        m = fresh()
        base = m._lin(m.ds, ADS_SCRIPT)
        m.mem[base:base + len(code)] = code
        m.write16(m.ds, ADS_CTX + 8, ADS_SCRIPT + len(code))
        m.write16(m.ds, 0x2608, 0)
        m.write16(m.ds, 0x460E, ADS_CTX)
        m.write16(m.ds, 0x4610, m.ds)
        for off, _op, _a in ads_walk(code):
            end = m.call(10, 0x106B, [ADS_SCRIPT + off, m.ds, 0])
            out.append('blocks %s %d %d'
                       % (name, off, -1 if end == 0 else end - ADS_SCRIPT))
    return out


def ads_run(machine, code, seqs, seq, pairs, passes=8):
    """Several passes of one sequence, with its state evolving between them.

    A single pass cannot see the two things that only exist across passes: the
    resume pointer a `10x0` guard parks at itself (`ctx+0x292+seq*4`), and the
    one-pass life of state 4. So this runs the interpreter repeatedly from
    whatever pointer the engine itself parked, and between passes moves the
    world on by a fixed rule -- the lowest-numbered scene that is playing has
    just ended, and whatever had just ended is now idle. That is what a scene
    finishing looks like to the sequencer, and it is the same rule on both
    sides.
    """
    m = machine
    base = m._lin(m.ds, ADS_SCRIPT)
    m.mem[base:base + len(code)] = code
    m.write16(m.ds, ADS_CURCTX, 0)
    m.write16(m.ds, ADS_CTXPTR, ADS_CTX)
    m.write16(m.ds, ADS_CTXPTR + 2, m.ds)
    m.write16(m.ds, ADS_CTX + 0x02, max(seqs) + 1)
    m.write16(m.ds, ADS_CTX + 0x08, ADS_SCRIPT + len(code))
    for sid, (off, _) in seqs.items():
        m.write16(m.ds, ADS_CTX + 0x152 + sid * 4, ADS_SCRIPT + off)
        m.write16(m.ds, ADS_CTX + 0x154 + sid * 4, m.ds)
        m.write16(m.ds, ADS_CTX + 0x12 + sid * 2, 1 if sid == seq else 0)
        m.write16(m.ds, ADS_CTX + 0x292 + sid * 4, 0)
        m.write16(m.ds, ADS_CTX + 0x294 + sid * 4, 0)
        m.write16(m.ds, ADS_CTX + 0x0B2 + sid * 2, 0)
    head, at_of = 0, {}
    for n, (ttm, scene) in enumerate(pairs):
        at = ADS_INSTS + n * 0x40
        for k in range(0, 0x40, 2):
            m.write16(m.ds, at + k, 0)
        m.write16(m.ds, at + 0x00, ttm)
        m.write16(m.ds, at + 0x02, scene)
        m.write16(m.ds, at + 0x3B, head)
        head = at
        at_of[(ttm, scene)] = at
    m.write16(m.ds, ADS_CTX + 0x10, head)
    m.write16(m.ds, ADS_CURSEQ, seq)

    log = []

    def record(label, argc):
        def watcher(mm):
            args = [mm.read16(mm.ss, mm.sp + 2 + k * 2) for k in range(argc)]
            log.append('%s %s' % (label, ' '.join(str(a) for a in args)))
        return watcher

    m.watches.clear()
    for off, (label, argc) in ADS_ACTIONS.items():
        m.watch(10, off, record(label, argc))

    out = []
    for turn in range(passes):
        mark = len(log)
        # `seg10:0x1B6B` walks every sequence in id order. Two rules matter
        # and both are read straight off the driver: states 3 and 4 become 1
        # **as the walk reaches each one** (`seg10:0x1BE8`), not before the
        # pass begins -- so a sequence an earlier one has just woken runs in
        # the same pass -- and a parked resume pointer beats the sequence's
        # own start (`seg10:0x1BCC`).
        for sid in sorted(seqs):
            state = m.read16(m.ds, ADS_CTX + 0x12 + sid * 2)
            if state in (3, 4):
                state = 1
                m.write16(m.ds, ADS_CTX + 0x12 + sid * 2, 1)
            if state != 1:
                continue
            parked = m.read16(m.ds, ADS_CTX + 0x292 + sid * 4)
            m.write16(m.ds, ADS_CURSEQ, sid)
            m.call(10, 0x1925,
                   [parked if parked else ADS_SCRIPT + seqs[sid][0], m.ds])
        out.append('pass %d %s | seq=%d %s' % (
            turn,
            ' ; '.join(log[mark:]),
            m.read16(m.ds, ADS_CTX + 0x12 + seq * 2),
            ' '.join('%d/%d=%d' % (t, s, m.read16(m.ds, at_of[(t, s)] + 0x2F))
                     for (t, s) in pairs)))
        # Move the world on.
        ended = None
        for (t, s) in sorted(pairs):
            at = at_of[(t, s)]
            state = m.read16(m.ds, at + 0x2F)
            if state == 4:
                m.write16(m.ds, at + 0x2F, 0)
            elif state in (1, 2, 3) and ended is None:
                ended = at
        if ended is not None:
            m.write16(m.ds, ended + 0x2F, 4)
    return out


def ads_all_pairs(layout):
    """Every scene the whole script mentions, in a stable order.

    `ads_pass` seeds one sequence's scenes because that is all one sequence
    can reach. A `seqrun` walks *every* sequence, so a `F200` can hand the
    pass to a sequence whose scenes are nowhere in the starting one's list --
    and a scene with no instance record reads as "never played" forever,
    which is a hole in the harness, not a rule of the engine.
    """
    seen = []
    for sid in sorted(layout):
        for pair in layout[sid][1]:
            if pair not in seen:
                seen.append(pair)
    return sorted(seen)


def seqrun_shape():
    """How far to run the sequencer, and from how many starting draws.

    Both numbers are coverage, not taste, and `tools/reach.py` is where they
    come from. Of the ADS instructions the dispatcher can see, eight passes
    from one draw reaches 64.7%. Passes matter because a scene has to end and
    start again before the guards waiting on it mean anything, so sixty of
    them from one draw reaches 80.9%; draws matter because a random block
    takes one branch and the rest are only reachable by drawing differently,
    so eight passes from eight draws reaches 75.6%. Neither knob substitutes
    for the other, and together they reach 88.7%.

    `Rng::new(n)` on the port and `n` calls of `seg4:0x1E86` here are the same
    thing -- the engine's only kind of seeding is to discard values -- so the
    two sides start from the same place.

    Both can be overridden, so what each one is worth can be measured rather
    than argued about: `JC_PASSES=8 JC_SEEDS=1 python3 tools/reach.py seqrun`
    against `JC_PASSES=60 JC_SEEDS=1` is the passes knob on its own.
    """
    passes, seeds = (60, 8) if os.environ.get('JC_DEEP') else (8, 2)
    return (int(os.environ.get('JC_PASSES') or passes),
            int(os.environ.get('JC_SEEDS') or seeds))


def q_seqrun():
    """seg10:0x1925 run over and over, which is the only way to see the parts
    of the sequencer that live between passes."""
    passes, seeds = seqrun_shape()
    out = []
    for name, code in ads_scripts():
        layout = ads_layout(code)
        pairs = ads_all_pairs(layout)
        for seq in sorted(layout):
            for seed in range(seeds):
                m = fresh()
                for _ in range(seed):
                    m.call(4, 0x1E86)
                for line in ads_run(m, code, layout, seq, pairs, passes):
                    out.append('seqrun %s %d %d %s' % (name, seq, seed, line))
    return out


def p_seqrun():
    passes, seeds = seqrun_shape()
    out = []
    for name, code in ads_scripts():
        layout = ads_layout(code)
        pairs = ','.join('%d:%d' % p for p in ads_all_pairs(layout)) or '-'
        for seq in sorted(layout):
            for seed in range(seeds):
                for line in port('seqrun', name, seq, pairs, passes, seed):
                    out.append('seqrun %s %d %d %s' % (name, seq, seed, line))
    return out


def seqpass_shape():
    """How many of a sequence's scenes to vary, and whether to try them all
    at once.

    `tools/reach.py` is where the numbers come from and `JC_CAP`,
    `JC_PASSSEEDS` and `JC_WIDESEEDS` re-derive them. Of the ADS instructions
    the dispatcher can see, varying the first six scenes reaches 55.0% and
    varying every scene reaches 84.3%, because a guard on a scene the case
    list never touched cannot fire. Setting them all at once as well, and
    varying the draw, reaches 99.4%: a block whose guard is a conjunction
    needs more than one scene in the right state at the same time -- `1420`
    joins two conditions and `seg10:0x1402` wants both -- and a weighted
    random block behind an open guard still only runs the entry the draw
    lands on.
    """
    deep = bool(os.environ.get('JC_DEEP'))
    cap, together = (99, (4, 1, 5)) if deep else (6, ())
    # Two seed counts, because the two kinds of case are worth very different
    # numbers of draws. A random block sits behind a guard, and the cases that
    # set every scene at once are the ones that open the most of them -- so
    # that is where varying the draw pays, and there are only three of them
    # per sequence, which makes forty-eight of each affordable. The
    # single-scene cases are the bulk of the list and get eight.
    seeds, wide = (8, 48) if deep else (2, 6)
    return (int(os.environ.get('JC_CAP') or cap), together,
            int(os.environ.get('JC_PASSSEEDS') or seeds),
            int(os.environ.get('JC_WIDESEEDS') or wide))


def _seqpass_cases():
    """Every (script, sequence, case) the comparison walks, and its states.

    One list, used by both sides, so the only thing being compared is what
    each does with the same starting position.
    """
    cap, together, seeds, wide = seqpass_shape()
    for name, code in ads_scripts():
        layout = ads_layout(code)
        # A draw can only change what a pass does if the pass makes one, so
        # the seeds go to the sequences with a `30xx` in them and nowhere
        # else. Seeding the other thirty-six would be the same answer eight
        # times over.
        drawing = ads_random(code, layout)
        for seq in sorted(layout):
            _, pairs = layout[seq]
            cases = [({}, seeds)]
            for pair in pairs[:cap]:
                for state in (4, 1, 5):
                    cases.append(({pair: (1, state)}, seeds))
            for state in together:
                cases.append((dict((p, (1, state)) for p in pairs), wide))
            for n, (states, draws) in enumerate(cases):
                for seed in range(draws if seq in drawing else 1):
                    yield name, code, layout, seq, n, states, seed


def q_seqpass():
    """One pass of every shipped sequence, in every state that matters.

    The instance records are the ones the *script* gets, not the ones this
    sequence mentions. `seg11:0x0645` makes one per `1111` marker when the
    file loads, so a guard in sequence 3 about a scene sequence 5 plays has
    something to read -- and with the narrower set it had nothing, took its
    absent path every time, and the other side of it was never compared.

    This is the sequencer's whole contract: given what is on screen, which
    scenes does a pass start, stop, finish or reorder, and in what order. It
    runs inside `seg10:0x1925` -- the engine's own interpreter loop, with its
    own dispatch, its own predicates and its own block scanners -- against a
    context put up by hand, with the action handlers hooked so that nothing
    below the sequencer has to exist.

    For each sequence: nothing playing, then each scene it mentions in turn as
    just-ended, playing, and finished. That exercises the predicate the
    scripts are built on (`1350`, five hundred of the six hundred and
    sixty-odd) from both sides.
    """
    out = []
    for name, code, layout, seq, n, states, seed in _seqpass_cases():
        m = fresh()
        for _ in range(seed):
            m.call(4, 0x1E86)
        got = ads_pass(m, code, layout, seq, states, ads_all_pairs(layout))
        out.append('seqpass %s %d %d %d %s'
                   % (name, seq, n, seed, ' ; '.join(got)))
    return out


def p_seqpass():
    out = []
    for name, _code, layout, seq, n, states, seed in _seqpass_cases():
        arg = ','.join('%d:%d:%d:%d' % (t, s, p, st)
                       for (t, s), (p, st) in sorted(states.items())) or '-'
        pairs = ','.join('%d:%d' % pair for pair in ads_all_pairs(layout)) or '-'
        mine = port('seqpass', name, seq, arg, seed, pairs)
        out.append('seqpass %s %d %d %d %s'
                   % (name, seq, n, seed, ' ; '.join(mine)))
    return out


# ---- the codecs ----------------------------------------------------------
#
# `RESOURCE.001`'s LZW and RLE are the last large thing this port reimplements
# rather than drives, and for a long time the reason was that the engine's
# decompressor pulls its input through a stream layer with a file mode, which
# would mean standing up the C runtime's `FILE` machinery. It has a second
# mode. `seg6:0x0185` picks between them on bit 5 of `ds:0x4030`, and with the
# bit clear the input is a plain far memcpy from `ds:0x4040` -- so a
# descriptor built by hand is enough, and none of the runtime has to exist.
#
#   ds:0x3F68[handle]            -> the descriptor
#   seg6:0x0C92(handle)           open it compressed: run the method's init
#                                 from the vtable at ds:0x1DC6 + method*14 + 6
#                                 and step past the five-byte header
#   seg6:0x0A0D(h, off, seg, n)   produce n decompressed bytes into off:seg
#
# The descriptor fields this needs are the source far pointer at +6, the
# position and end at +0x0A and +0x0E, the decompressed length at +0x12, the
# LZW work buffer at +2 (`seg6:0x0D4B` memsets 0x3AA1 bytes of it), and the
# mode byte at +0x20 -- bit 6 for "compressed", bit 5 clear for memory, and
# the method in the low five bits.

CODEC_DESC = 0x9200      # free space in DGROUP for a stream descriptor
CODEC_TABLE = 0x3F68     # ds:0x3F68[handle] -> descriptor
CODEC_HANDLE = 1
CODEC_WORK = 20          # three segments the NE does not use, 64K each
CODEC_SRC = 21
CODEC_DST = 22
CODEC_BITE = 0x2000      # produced per call, so nothing has to cross 64K


def codec_run(m, payload, usize, limit):
    """Decompress one leaf inside the original, and hand back what it made."""
    at = m._lin(CODEC_SRC, 0)
    m.mem[at:at + len(payload)] = payload
    for k in range(0, 0x24, 2):
        m.write16(m.ds, CODEC_DESC + k, 0)
    m.write16(m.ds, CODEC_DESC + 0x02, 0)                    # LZW work buffer
    m.write16(m.ds, CODEC_DESC + 0x04, CODEC_WORK)
    m.write16(m.ds, CODEC_DESC + 0x06, 0)                    # the input
    m.write16(m.ds, CODEC_DESC + 0x08, CODEC_SRC)
    m.write16(m.ds, CODEC_DESC + 0x0E, len(payload) & 0xFFFF)
    m.write16(m.ds, CODEC_DESC + 0x10, len(payload) >> 16)
    m.write16(m.ds, CODEC_DESC + 0x12, usize & 0xFFFF)
    m.write16(m.ds, CODEC_DESC + 0x14, usize >> 16)
    m.write8(m.ds, CODEC_DESC + 0x20, 0x40 | payload[0])
    m.write16(m.ds, CODEC_TABLE + CODEC_HANDLE * 2, CODEC_DESC)
    if m.call(6, 0x0C92, [CODEC_HANDLE]) == 0xFFFF:
        raise RuntimeError('seg6:0x0C92 refused the stream')
    want = min(usize, limit)
    out = bytearray()
    while len(out) < want:
        n = min(CODEC_BITE, want - len(out))
        got = m.call(6, 0x0A0D, [CODEC_HANDLE, 0, CODEC_DST, n])
        if got == 0:
            break
        base = m._lin(CODEC_DST, 0)
        out += m.mem[base:base + got]
        if got < n:
            break
    return bytes(out)


def codec_leaves():
    """Every compressed leaf, in the order both sides walk the archive."""
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    for name, body, _ in recs:
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if dgds.is_compressed(name, path[-1]):
                usize = struct.unpack_from('<I', payload, 1)[0]
                yield name, path[-1], payload, usize


def codec_limit():
    """How much of each leaf to decode.

    The whole archive is 5.2MB out and about twenty minutes of interpreter,
    which is a fine thing to do deliberately and a poor thing to put in front
    of every other check. The default reaches past the 9-to-10 and 10-to-11
    bit widths on every LZW leaf -- the block-boundary rule that is the
    easiest thing in this codec to get wrong -- and covers every RLE leaf
    whole. `JC_DEEP=1` decodes every byte of all 180.
    """
    return None if os.environ.get('JC_DEEP') else 8192


def q_codec():
    m = fresh()
    m.limit = 400_000_000
    limit = codec_limit()
    out = []
    leaves = total = 0
    for name, tag, payload, usize in codec_leaves():
        n = usize if limit is None else min(usize, limit)
        got = codec_run(m, payload, usize, n)
        leaves += 1
        total += len(got)
        out.append('%s %s %d %d %d %08x'
                   % (name, tag, payload[0], usize, len(got),
                      zlib.crc32(got) & 0xFFFFFFFF))
    # Covered, as a line both sides have to agree on -- so a run that quietly
    # compared a prefix cannot be mistaken for one that compared everything.
    out.append('total %d leaves %d bytes' % (leaves, total))
    return out


def p_codec():
    limit = codec_limit()
    args = ['codec'] if limit is None else ['codec', limit]
    return port(*args)


# ---- the TTM interpreter -------------------------------------------------

# The last large thing this port reimplements rather than drives is the TTM
# frame runner itself -- `seg12:0x040C`, the loop that fetches an opcode,
# collects its operands and dispatches. Running it means standing a loaded
# TTM up in memory and then keeping the rest of the program out of the way:
#
#   ds:0x2634/0x2636   the head of the loaded-TTM list
#   ds:0x2638/0x263A   the one in play; +2 is its frame count, +4/+6 its
#                      bytes, +8/+0xA their last byte, +0xD0/0xD2 the frame
#                      table `seg11:0x04D6` fills by counting `0FF0`s
#   ds:0x263E          the instance record the frame runs against, laid out
#                      by `seg11:0x042A` and reset by `seg10:0x0B3E`
#   ds:0x2562/0x2564   the screen, 640x480 (`seg2:0x1483`)
#
# and then a wall around segments 1 and 12: while the interpreter is running,
# any call that leaves them -- the blitter, the pager, the sound -- is
# answered here instead of running. Most are answered with zero; the ones a
# frame's *decisions* are made of are recorded first, arguments and all, and
# those recordings are the second half of what is compared.
#
# The one that has to do more than record is the shape loader. Left at zero
# the bitmap slots would stay empty and the sprite handler at `seg12:0x025B`
# would find nothing to draw, taking 15,368 of the 16,000-odd drawing
# decisions in the data out of reach. So `seg6:0x17FA` is answered with a
# table of the same length the real bitmap has, and `ds:0x30D4` -- the
# `JOHNWALK.BMP` the engine loads once and hands out again at
# `seg12:0x0792` -- is filled in the same way. The port runs against the real
# archive, so both sides agree about which draws happen at all.
#
# What is compared, per frame: every instruction, as the dispatcher sees it
# and with the operands as the handler receives them; every drawing decision,
# with the arguments the handler worked out; and what the frame left on the
# instance record afterwards.

TTM_REC = 30             # segments the NE does not use, one 64K each
TTM_DATA = 31
TTM_RECOFF = 0x0000      # the 0xD4-byte loaded-TTM record
TTM_TABLE = 0x0400       # its frame table, just after it
TTM_INST = 0x9800        # the instance record, in free DGROUP
TTM_WALK = 0x9000        # the cached JOHNWALK.BMP shapes at ds:0x30D4
TTM_HEAP = 0xA000        # the records seg11:0x042A makes, and shape tables
TTM_TOP = 0xEC00         # ... and where they must stop, short of the stack
# `seg12:0x0281` indexes a shape table by the opcode's frame operand and does
# not bounds-check it: eleven draws in the shipped scripts ask for a frame
# their bitmap does not have, and what the engine reads there is whatever the
# heap holds next. Every table here is padded past the largest index any
# script asks for, so the read lands on a zero and the handler takes its
# no-shape path -- which is the one answer that is not garbage, and is what
# the port does. The `blank` lines in the output are those draws.
TTM_FRAMES = 102         # the largest frame operand in the data, plus one
TTM_W, TTM_H = 640, 480

# The far calls a frame's decisions are made of, and how many words each
# takes. Everything else that leaves segments 1 and 12 is answered with zero
# and not recorded: the pager, the dirty-rectangle bookkeeping at
# `seg11:0x0A85`, the screen and palette loaders, the "could not load that"
# report at `seg5:0x081C`. None of them is a decision the port makes too.
TTM_LEAVES = {
    (7, 0x02EF): ('line', 4),      # seg12:0x0179, A0A4
    (8, 0x1BD8): ('rect', 4),      # seg12:0x0199 and 0x01B6, A104 and A114
    (9, 0x0688): ('circle', 3),    # seg12:0x01E8, A404 and A424
    (9, 0x0D54): ('pixel', 3),     # seg12:0x0233, A002
    (6, 0x17D8): ('sprite', 4),    # seg12:0x02CD, the A5 family
    (8, 0x06CE): ('copy', 4),      # seg12:0x0EE2, B606
    (5, 0x00BA): ('sound', 1),     # seg12:0x0064, C051
}


def ttm_bitmaps():
    """How many frames each `.BMP` in the archive holds.

    The count is `INF:`'s first word, so nothing has to be decompressed --
    and it is the same number the port's `Archive::bitmap` hands back, which
    `verify.py` counts at 2,392 across the archive.
    """
    import dgds
    out = {}
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    for name, body, _ in recs:
        if not name.endswith('.BMP'):
            continue
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == 'INF:':
                out[name] = struct.unpack_from('<H', payload, 0)[0]
                break
    return out


def ttm_scripts():
    """Every shipped `.TTM`, decompressed, in the order both sides walk."""
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    for name, body, _ in recs:
        if not name.endswith('.TTM'):
            continue
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == 'TT3:':
                yield name, dgds.decompress(payload)
                break


class Interpreter:
    """`SCRANTIC.EXE`'s own TTM frame runner, with the rest of it walled off."""

    def __init__(self):
        self.m = fresh()
        self.m.limit = 4_000_000
        self.near = TTM_HEAP
        self.ops = []
        self.draws = []
        m = self.m
        m.hook(8, 0x0619, self._malloc)      # the instance-record allocator
        m.hook(1, 0x229C, self._malloc)
        m.hook(1, 0x22E1, lambda mm: mm.regs.__setitem__('ax', 0))   # free
        m.hook(1, 0x121A, self._memset)
        m.wall({1, 10, 12}, self._outside,
               # What is outside the wall but allowed through. Three of these
               # are arithmetic on their arguments and reach no further: the
               # string-length rule the fetch needs to step over a filename,
               # the generator `2022` draws from, and the file-id lookup that
               # picks which loaded TTM an instance belongs to. The fourth is
               # the frame runner itself, which is the thing being driven.
               allow={(11, 0x04A8), (4, 0x1E86), (11, 0x03E2), (11, 0x074A)})
        m.watch_at(12, 0x0620, self._dispatch)
        # Where a load, a free and the region family finish, so what they
        # settled on can be read out of the record the same way for both of
        # the two paths `F02F` has.
        m.watch_at(12, 0x07F8, self._loaded)
        m.watch_at(12, 0x084D, self._freed)
        m.watch_at(12, 0x0340, self._region)
        m.watch_at(12, 0x0291, self._blank)
        m.write16(m.ds, 0x2562, TTM_W)
        m.write16(m.ds, 0x2564, TTM_H)
        self.bitmaps = ttm_bitmaps()
        self.tables = {}
        # `seg12:0x0792` hands out a cached `JOHNWALK.BMP` rather than
        # loading it again -- `ds:0x27D2` holds the name and `ds:0x30D4` the
        # shapes -- so 36 of the 263 loads in the data never reach the loader
        # at all. Fill it the same way the rest are filled.
        walk = self.bitmaps.get('JOHNWALK.BMP', 0)
        self._table(TTM_WALK, walk)
        m.write16(m.ds, 0x30D4, TTM_WALK)

    def _table(self, at, n):
        """A shape table: `n` non-zero near words -- which is all the sprite
        handler asks of one before it hands the entry to the blitter -- and
        then zeros as far as any script can index."""
        m = self.m
        for i in range(TTM_FRAMES):
            m.write16(m.ds, at + i * 2, at + i * 2 if i < n else 0)
        self.tables[at] = n
        return at

    # ---- what stands in for the rest of the program -----------------------

    def _malloc(self, m):
        n = m.read16(m.ds, m.regs['sp'])
        at = self.near
        self.near = (self.near + n + 3) & ~3
        m.regs['ax'] = at

    def _memset(self, m):
        at = m.read16(m.ds, m.regs['sp'])
        c = m.read16(m.ds, m.regs['sp'] + 2) & 0xFF
        n = m.read16(m.ds, m.regs['sp'] + 4)
        base = m._lin(m.ds, 0)
        m.mem[base + at:base + at + n] = bytes([c]) * n
        m.regs['ax'] = at

    def _outside(self, m, seg, off):
        m.regs['ax'] = 0
        if (seg, off) == (6, 0x17FA):
            return self._load_shapes(m)
        what = TTM_LEAVES.get((seg, off))
        if what is None:
            return
        # Only what a frame draws on the tick it runs. On the ticks after it
        # the engine runs the frame *again* (`seg10:0x1CD6` raises the flag at
        # `ds:0x46DA`), against slots the frame itself may have freed or
        # loaded over -- and this port puts back what the frame drew instead,
        # which is deliberate and not the same. `tests.rs` pins how far apart
        # the two can get: 49 frames in the archive, 48 of them losing
        # sprites they had a tick earlier.
        if m.read16(m.ds, 0x46DA):
            return
        name, argc = what
        a = [_s16(m.read16(m.ds, (m.regs['sp'] + k * 2) & 0xFFFF)) for k in range(argc)]
        fg = m.read8(m.ds, 0x1EAE)
        if name == 'sprite':
            # The slot and the frame are the A-family handler's own third and
            # fourth operands, which is where `seg12:0x025B` reads them; the
            # blitter is handed the shape, not where it came from.
            bp = m.regs['bp']
            slot = m.read16(m.ds, (bp + 0x0C) & 0xFFFF)
            frame = m.read16(m.ds, (bp + 0x0A) & 0xFFFF)
            self.draws.append('sprite %d %d %d %d %d' % (slot, frame, a[1], a[2], a[3]))
        elif name == 'line':
            self.draws.append('line %d %d %d %d %d' % (a[0], a[1], a[2], a[3], fg))
        elif name == 'rect':
            self.draws.append('rect %d %d %d %d %d %d'
                              % (a[0], a[1], a[2], a[3], fg, m.read8(m.ds, 0x1EAC)))
        elif name == 'circle':
            self.draws.append('circle %d %d %d %d %d'
                              % (a[0], a[1], a[2], fg, m.read8(m.ds, 0x1EAC)))
        elif name == 'pixel':
            self.draws.append('pixel %d %d %d' % (a[0], a[1], a[2]))
        elif name == 'copy':
            self.draws.append('copy %d %d %d %d' % (a[0], a[1], a[2], a[3]))
        elif name == 'sound':
            self.draws.append('sound %d' % a[0])

    def _load_shapes(self, m):
        """`seg6:0x17FA`, the shape loader. Answers with a table of the same
        length the real bitmap has, so the sprite handler agrees with the
        port about which draws can happen."""
        at = m.read16(m.ds, m.regs['sp'])
        base = m._lin(m.ds, at)
        name = m.mem[base:m.mem.index(0, base)].decode('latin-1')
        n = self.bitmaps.get(name)
        if n is None:
            return                       # not in the archive: the slot clears
        table = self.near
        self.near += TTM_FRAMES * 2
        if self.near > TTM_TOP:
            raise RuntimeError('the shape tables have run into the stack')
        m.regs['ax'] = self._table(table, n)

    def _slot(self):
        """The shape table the running instance's bitmap slot holds, and how
        long it is.

        Both pointers have to come from where the engine keeps them:
        `ds:0x263E` is the instance whose frame is running and `ds:0x2638` the
        loaded TTM it belongs to, and `seg12:0x0785` reads the slot out of
        *that* record at `+0x0C + slot*2`. The slots are per file.
        """
        m = self.m
        inst = m.read16(m.ds, 0x263E)
        slot = m.read16(m.ds, inst + 0x14) & 0x0F
        rec = (m.read16(m.ds, 0x263A), m.read16(m.ds, 0x2638))
        at = m.read16(rec[0], rec[1] + 0x0C + slot * 2)
        return slot, self.tables.get(at, 0)

    def _loaded(self, m):
        self.draws.append('shapes %d %d' % self._slot())

    def _freed(self, m):
        inst = m.read16(m.ds, 0x263E)
        self.draws.append('shapes %d 0' % (m.read16(m.ds, inst + 0x14) & 0x0F))

    def _blank(self, m):
        """`seg12:0x0291`: the slot has a table, and the frame the opcode
        asked for is not in it."""
        bp = m.regs['bp']
        self.draws.append('blank %d %d' % (m.read16(m.ds, (bp + 0x0C) & 0xFFFF),
                                           m.read16(m.ds, (bp + 0x0A) & 0xFFFF)))

    def _region(self, m):
        """`seg12:0x0340`, reached only by a rectangle that got past the
        guard at `seg12:0x0318`."""
        bp = m.regs['bp']
        r = lambda k: _s16(m.read16(m.ds, (bp + k) & 0xFFFF))
        self.draws.append('region %04X %d %d %d %d'
                          % (m.read16(m.ds, 0x46DC), r(6), r(8), r(0x0A), r(0x0C)))

    # ---- the instruction stream -------------------------------------------

    def _dispatch(self, m):
        """`seg12:0x0620`, where the opcode and its operands are complete and
        no handler has run yet. `ds:0x46DC` is the opcode after the two
        rewrites at `seg12:0x0467`; `[bp-0x26]` is the operand count off its
        low nibble; the operands are at `[bp-4]` downward, already shifted by
        the island's offset; `[bp-0x1A]` is the string, when the count is
        `0x0F`."""
        bp = m.regs['bp']
        op = m.read16(m.ds, 0x46DC)
        count = m.read16(m.ds, (bp - 0x26) & 0xFFFF)
        if count == 0x0F:
            at = m.read16(m.ds, (bp - 0x1A) & 0xFFFF)
            base = m._lin(m.ds, at)
            end = m.mem.index(0, base)
            text = m.mem[base:end].decode('latin-1')
            self.ops.append('op %04X "%s"' % (op, text))
            return
        args = [_s16(m.read16(m.ds, (bp - 4 - k * 2) & 0xFFFF)) for k in range(count)]
        self.ops.append(('op %04X %s' % (op, ' '.join(str(a) for a in args))).rstrip())

    # ---- one file ---------------------------------------------------------

    def load(self, code):
        """Put a script where a loaded TTM lives and let `seg11:0x04D6` build
        its frame table, the same walk that decides what a frame even is."""
        m = self.m
        base = m._lin(TTM_DATA, 0)
        m.mem[base:base + len(code)] = code
        n = sum(1 for i in range(0, len(code) - 1, 2)
                if struct.unpack_from('<H', code, i)[0] == 0x0FF0)
        rec = m._lin(TTM_REC, TTM_RECOFF)
        m.mem[rec:rec + 0xD4] = b'\0' * 0xD4
        struct.pack_into('<HH', m.mem, rec + 0x00, 0, n)
        struct.pack_into('<HH', m.mem, rec + 0x04, 0, TTM_DATA)
        struct.pack_into('<HH', m.mem, rec + 0x08, len(code) - 1, TTM_DATA)
        struct.pack_into('<HH', m.mem, rec + 0xD0, TTM_TABLE, TTM_REC)
        tbl = m._lin(TTM_REC, TTM_TABLE)
        m.mem[tbl:tbl + (n + 1) * 4] = b'\0' * ((n + 1) * 4)
        for off in (0x2634, 0x2638):
            m.write16(m.ds, off, TTM_RECOFF)
            m.write16(m.ds, off + 2, TTM_REC)
        m.write16(m.ds, 0x263C, 0)      # the instance list starts empty
        m.write16(m.ds, 0x263E, 0)
        self.near = TTM_HEAP
        self.tables = {TTM_WALK: self.tables[TTM_WALK]}
        if m.call(11, 0x04D6, [0]) != 1:
            raise RuntimeError('seg11:0x04D6 refused the script')
        # One instance record, reset the way `seg10:0x0B3E` resets one.
        m.mem[m._lin(m.ds, TTM_INST):m._lin(m.ds, TTM_INST) + 0x3D] = b'\0' * 0x3D
        m.write16(m.ds, TTM_INST + 0x0A, 0xFFFF)
        m.call(10, 0x0B3E, [TTM_INST])
        m.write16(m.ds, 0x263E, TTM_INST)
        self.delay = m.read16(m.ds, TTM_INST + 0x29)
        return n

    def _before(self):
        """`seg10:0x1C81`, the step the ADS runner takes in front of every
        frame: the instance's own colours and clip into the globals the
        drawing handlers read, and the two channels the frame reports back
        through -- `ds:0x4644` for "the scene ended" and `ds:0x46E8` for "and
        hold it this long" -- set to their nothing-yet values.

        Run rather than transcribed. It is fifteen instructions of
        straight-line code with no calls in it, and running it means the
        harness cannot be the thing that gets it wrong.
        """
        m = self.m
        m.regs['bx'] = TTM_INST
        m.sregs['cs'] = 10
        m.ip = 0x1C81
        while m.ip != 0x1CBA:
            m.step()

    def frame(self, k):
        """Run frame `k`, the way `seg10:0x1C81` onward runs one."""
        m = self.m
        m.write16(m.ds, TTM_INST + 0x08, k)
        self._before()
        del self.ops[:]
        del self.draws[:]
        m.call(11, 0x074A, [k])
        # seg10:0x1CE6: a frame that asked for no delay leaves the last one.
        asked = m.read16(m.ds, 0x46E8)
        if asked != 0xFFFF:
            self.delay = asked
        r = lambda off: m.read16(m.ds, TTM_INST + off)
        record = ('pal%d bm%d fg%d bg%d clip%d,%d,%d,%d delay%d end%d goto%d'
                  % (r(0x12), r(0x14),
                     m.read8(m.ds, TTM_INST + 0x1E), m.read8(m.ds, TTM_INST + 0x1F),
                     _s16(r(0x21)), _s16(r(0x23)), _s16(r(0x25)), _s16(r(0x27)),
                     self.delay,
                     1 if m.read16(m.ds, 0x4644) else 0,
                     _s16(m.read16(m.ds, 0x4646))))
        return list(self.ops), list(self.draws), record


# ---- a whole tick --------------------------------------------------------

# `seg10:0x1ACB` *is* a tick. It takes no arguments: it reads the current ADS
# context out of `ds:0x2608` and does the three things a tick does.
#
#   `seg10:0x1B12`  the sequence bookkeeping. Each sequence keeps a
#                   null-terminated array of the instance records it plays
#                   (`ctx+0x3D2+sid*2`, built by `seg10:0x040F` from the
#                   script's `2000` and `2005` operands). A sequence that has
#                   just been started -- state 3 -- has every one of them
#                   reset by `seg10:0x0B3E`; otherwise its state is stamped
#                   onto each instance's `+0x2B`.
#   `seg10:0x1B6B`  one pass of every sequence, in id order, through the
#                   interpreter at `seg10:0x1925`.
#   `seg10:0x1C32`  the instance walk: `ctx+0x10` is the head and `+0x3B`
#                   chains it. Each instance selects its own loaded TTM
#                   (`seg10:0x1C71`), has its colours and clip put back
#                   (`0x1C81`), and runs its current frame (`0x1CBA`).
#
# The order matters and it is not the obvious one: the script runs **before**
# the instances, so a scene the pass starts draws its first frame in the same
# tick.
#
# This is the layer neither `seqrun` nor `ttm` reaches. `seqrun` asks what a
# pass does to the scene states with no instances running; `ttm` asks what a
# frame does with nothing else on screen. What is left over is the
# interleaving -- which instance draws first, when a held frame is drawn
# again, and when a scene that has reached its end stops -- and that is what
# this compares.

TICK_REC = 30            # the loaded-TTM records and their frame tables
TICK_DATA = 32           # one 64K segment per TTM file, from here up
TICK_CTX = ADS_CTX       # the ADS context
TICK_SCRIPT = ADS_SCRIPT


def ttm_leaf(name):
    """One `.TTM` record's script, or None."""
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    for rec, body, _ in recs:
        if rec != name:
            continue
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == 'TT3:':
                return dgds.decompress(payload)
    return None


def ads_leaf(name):
    """One `.ADS` record's script and its `RES:` file table."""
    import dgds
    recs = dgds.read_archive(open('original/RESOURCE.001', 'rb').read())
    for rec, body, _ in recs:
        if rec != name:
            continue
        parts = dict((path[-1], payload)
                     for path, payload in dgds.flatten(dgds.parse_chunks(body)))
        return dgds.decompress(parts['SCR:']), dgds.parse_id_table(parts['RES:'])
    raise RuntimeError('no ADS record %s' % name)


class World(Interpreter):
    """Everything a tick needs: an ADS context, its loaded TTMs, and the
    instance records `seg11:0x04D6` makes for them."""

    def __init__(self, record, sequence):
        Interpreter.__init__(self)
        self.rec = 0
        self.acts = []
        m = self.m
        m.limit = 20_000_000
        m.watch_at(10, 0x1C3C, self._visit)
        m.watch_at(12, 0x040C, self._run)
        for off, (label, argc) in ADS_ACTIONS.items():
            m.watch(10, off, self._act(label, argc))
        self._build(record, sequence)

    def _act(self, label, argc):
        def watcher(mm):
            args = [mm.read16(mm.ss, mm.sp + 2 + k * 2) for k in range(argc)]
            self.acts.append('%s %s' % (label, ' '.join(str(a) for a in args)))
        return watcher

    def _visit(self, m):
        """The top of the instance body, where `ds:0x263E` is the one about to
        be looked at."""
        bx = m.read16(m.ds, 0x263E)
        self.here = (m.read16(m.ds, bx), m.read16(m.ds, bx + 2))

    def _run(self, m):
        """`seg12:0x040C` is entered once per live instance per tick -- the
        engine draws a held frame again rather than keeping the picture, and
        `seg10:0x1CD6` is what raises the flag that gates the side effects
        off the second time."""
        bx = m.read16(m.ds, 0x263E)
        self.draws.append('run %d %d %d %d'
                          % (m.read16(m.ds, bx), m.read16(m.ds, bx + 2),
                             m.read16(m.ds, bx + 8), m.read8(m.ds, bx + 0x0E)))

    def _instance(self, ttm, scene):
        m = self.m
        p = m.read16(m.ds, 0x263C)
        while p:
            if m.read16(m.ds, p) == ttm and m.read16(m.ds, p + 2) == scene:
                return p
            p = m.read16(m.ds, p + 0x3B)
        return 0

    def _plays(self, code, start):
        """The instance records one sequence plays, first-appearance order,
        null-terminated: what `seg10:0x040F` builds by walking the script for
        the two opcodes at `ds:0x260A` -- `2000` and `2005` -- and resolving
        each one's first two operands through `seg10:0x0BF4`."""
        m = self.m
        seen, out, p = set(), [], start
        while p + 2 <= len(code):
            op = struct.unpack_from('<H', code, p)[0]
            if op == 0xFFFF and p > start:
                break
            if op in (0x2000, 0x2005):
                ttm, scene = struct.unpack_from('<HH', code, p + 2)
                at = self._instance(ttm, scene)
                if at and at not in seen:
                    seen.add(at)
                    out.append(at)
            p += _ads_len(op)
        at = self.near
        self.near += (len(out) + 1) * 2
        for i, v in enumerate(out):
            m.write16(m.ds, at + i * 2, v)
        m.write16(m.ds, at + len(out) * 2, 0)
        return at

    def _load_ttm(self, ttm_id, code, seg):
        """One loaded-TTM record, chained at the head the way `seg11:0x0304`
        chains one, with the engine's own frame scan run over it."""
        m = self.m
        base = m._lin(seg, 0)
        m.mem[base:base + len(code)] = code
        at, self.rec = self.rec, self.rec + 0x100
        tbl = self.rec
        n = sum(1 for i in range(0, len(code) - 1, 2)
                if struct.unpack_from('<H', code, i)[0] == 0x0FF0)
        self.rec += (n + 1) * 4 + 16
        r = m._lin(TICK_REC, at)
        m.mem[r:r + 0xD4] = b'\0' * 0xD4
        struct.pack_into('<HH', m.mem, r + 0x00, ttm_id, n)
        struct.pack_into('<HH', m.mem, r + 0x04, 0, seg)
        struct.pack_into('<HH', m.mem, r + 0x08, len(code) - 1, seg)
        struct.pack_into('<HH', m.mem, r + 0xD0, tbl, TICK_REC)
        t = m._lin(TICK_REC, tbl)
        m.mem[t:t + (n + 1) * 4] = b'\0' * ((n + 1) * 4)
        struct.pack_into('<HH', m.mem, r + 0xCC,
                         m.read16(m.ds, 0x2634), m.read16(m.ds, 0x2636))
        for off in (0x2634, 0x2638):
            m.write16(m.ds, off, at)
            m.write16(m.ds, off + 2, TICK_REC)
        if m.call(11, 0x04D6, [ttm_id]) != 1:
            raise RuntimeError('seg11:0x04D6 refused %d' % ttm_id)

    def _build(self, record, sequence):
        m = self.m
        code, files = ads_leaf(record)
        self.files = files
        base = m._lin(m.ds, TICK_SCRIPT)
        m.mem[base:base + len(code)] = code
        seqs = ads_layout(code)
        for off in (0x2634, 0x2636, 0x263C, 0x263E):
            m.write16(m.ds, off, 0)
        for n, (fid, name) in enumerate(sorted(files.items())):
            body = ttm_leaf(name)
            if body is not None:
                self._load_ttm(fid, body, TICK_DATA + n)
        m.write16(m.ds, ADS_CURCTX, 0)
        m.write16(m.ds, ADS_CTXPTR, TICK_CTX)
        m.write16(m.ds, ADS_CTXPTR + 2, m.ds)
        m.write16(m.ds, TICK_CTX + 0x02, max(seqs) + 1)
        m.write16(m.ds, TICK_CTX + 0x08, TICK_SCRIPT + len(code))
        m.write16(m.ds, TICK_CTX + 0x0C, m.read16(m.ds, 0x2634))
        m.write16(m.ds, TICK_CTX + 0x0E, m.read16(m.ds, 0x2636))
        m.write16(m.ds, TICK_CTX + 0x10, m.read16(m.ds, 0x263C))
        for sid, (off, _) in seqs.items():
            m.write16(m.ds, TICK_CTX + 0x152 + sid * 4, TICK_SCRIPT + off)
            m.write16(m.ds, TICK_CTX + 0x154 + sid * 4, m.ds)
            # `seg4:0x07CE` starts a sequence by putting it in state 3.
            m.write16(m.ds, TICK_CTX + 0x12 + sid * 2, 3 if sid == sequence else 0)
            m.write16(m.ds, TICK_CTX + 0x292 + sid * 4, 0)
            m.write16(m.ds, TICK_CTX + 0x294 + sid * 4, 0)
            m.write16(m.ds, TICK_CTX + 0x0B2 + sid * 2, 0)
            m.write16(m.ds, TICK_CTX + 0x3D2 + sid * 2, self._plays(code, off))
        m.write16(m.ds, ADS_CURSEQ, sequence)

    def tick(self):
        """One call of `seg10:0x1ACB`, with the clock moved on first the way
        `seg2:0x16D8` moves it -- every deadline on an instance record is
        measured against it."""
        m = self.m
        lo = m.read16(m.ds, 0x2E72) + 1
        m.write16(m.ds, 0x2E72, lo & 0xFFFF)
        if lo > 0xFFFF:
            m.write16(m.ds, 0x2E74, (m.read16(m.ds, 0x2E74) + 1) & 0xFFFF)
        del self.acts[:]
        del self.draws[:]
        self.here = None
        m.call(10, 0x1ACB)
        return list(self.acts), list(self.draws)


# What the query drives, and how far. Every scene the scheduler can pick --
# the same list `tools/sweep.py` plays -- for long enough that each one gets
# past its first few frames, several of them held, and in most cases to its
# end and round again.
# A tick is expensive -- the pass re-reads the whole script and the walk
# visits every instance of every file the script loads -- so the default takes
# the first scene of each of the ten `.ADS` files and runs each far enough to
# get past the first frames, through several held ones and into the second.
# `JC_DEEP=1` runs all sixty-five, four times as far.
def tick_cases():
    """Every scheduler scene, driven for a while, or one per file for less.

    The tick count is the third knob `tools/reach.py` reports on, and
    `JC_TICKS` overrides it: a scene the story never gets round to starting
    inside the window is one whose instructions nothing compared.
    """
    import scenes
    rows = scenes.scenes()
    deep = bool(os.environ.get('JC_DEEP'))
    ticks = int(os.environ.get('JC_TICKS') or (120 if deep else 32))
    if deep:
        return [(name, int(seq), ticks) for name, seq in rows]
    first, out = set(), []
    for name, seq in rows:
        if name not in first:
            first.add(name)
            out.append((name, int(seq), ticks))
    return out


def q_tick():
    out = []
    cases = tick_cases()
    for record, sequence, ticks in cases:
        w = World(record, sequence)
        for n in range(ticks):
            acts, draws = w.tick()
            out.append('%s %d act %s' % (record, n, ' ; '.join(acts)))
            out += ['%s %d %s' % (record, n, line) for line in draws]
    out.append('total %d cases' % len(cases))
    return out


def p_tick():
    lines = []
    cases = tick_cases()
    for record, sequence, ticks in cases:
        lines += port('tick', record, sequence, ticks)
    lines.append('total %d cases' % len(cases))
    return lines


def q_ttm():
    out = []
    files = frames = instrs = draws = 0
    e = Interpreter()
    for name, code in ttm_scripts():
        n = e.load(code)
        files += 1
        out.append('%s frames %d' % (name, n))
        for k in range(n):
            ops, drawn, record = e.frame(k)
            instrs += len(ops)
            draws += len(drawn)
            frames += 1
            out += ['%s %d %s' % (name, k, line) for line in ops + drawn]
            out.append('%s %d = %s' % (name, k, record))
    out.append('total %d files %d frames %d instructions %d draws'
               % (files, frames, instrs, draws))
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
    'holiday': (q_holiday, lambda: port('holiday')),
    'backdrop': (q_backdrop, lambda: port('backdrop')),
    'repeat': (q_repeat, lambda: port('repeat')),
    'island': (q_island, lambda: port('island')),
    'route': (q_route, lambda: port('route', 32)),
    'turn': (q_turn, lambda: port('turn', 8)),
    'blocks': (q_blocks, lambda: port('blocks')),
    'seqpass': (q_seqpass, p_seqpass),
    'seqrun': (q_seqrun, p_seqrun),
    'codec': (q_codec, p_codec),
    'ttm': (q_ttm, lambda: port('ttm')),
    'tick': (q_tick, p_tick),
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
