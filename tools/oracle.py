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


def _seqpass_cases():
    """Every (script, sequence, case) the comparison walks, and its states.

    One list, used by both sides, so the only thing being compared is what
    each does with the same starting position.
    """
    for name, code in ads_scripts():
        layout = ads_layout(code)
        for seq in sorted(layout):
            _, pairs = layout[seq]
            cases = [{}]
            for pair in pairs[:6]:
                for state in (4, 1, 5):
                    cases.append({pair: (1, state)})
            for n, states in enumerate(cases):
                yield name, code, layout, seq, n, states


def q_seqpass():
    """One pass of every shipped sequence, in every state that matters.

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
    for name, code, layout, seq, n, states in _seqpass_cases():
        got = ads_pass(fresh(), code, layout, seq, states, layout[seq][1])
        out.append('seqpass %s %d %d %s' % (name, seq, n, ' ; '.join(got)))
    return out


def p_seqpass():
    out = []
    for name, _code, layout, seq, n, states in _seqpass_cases():
        arg = ','.join('%d:%d:%d:%d' % (t, s, p, st)
                       for (t, s), (p, st) in sorted(states.items())) or '-'
        pairs = ','.join('%d:%d' % pair for pair in layout[seq][1]) or '-'
        mine = port('seqpass', name, seq, arg, 0, pairs)
        out.append('seqpass %s %d %d %s' % (name, seq, n, ' ; '.join(mine)))
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
    'blocks': (q_blocks, lambda: port('blocks')),
    'seqpass': (q_seqpass, p_seqpass),
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
