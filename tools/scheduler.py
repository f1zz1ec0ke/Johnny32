#!/usr/bin/env python3
"""Dump the scene scheduler tables out of SCRANTIC.EXE's data segment.

These are the tables the original engine picks scenes from. They were read off
the code in segment 4:

    seg4:0x0D76   the chooser -- filters the scene table, sums weights,
                  picks one with rand() % total
    seg4:0x0D37   resolves an ADS file id against the file table
    seg4:0x0BC8   the day rollover and story-stage advance

See docs/SCHEDULER.md for what the fields mean.
"""

import argparse
import struct

ADS_TABLE = 0x166E      # {uint16 id; char name[13]; uint16 handle}
SCENE_TABLE = 0x1756    # 17-byte records, zero-terminated
RECORD = 17

ROUTE_MATRIX = 0x0A94   # word at +from*12+to*2 -> route block offset
STEP_POSITIONS = 0x02DE  # 3 bytes per step id
STEP_TABLE = 0x04FB     # in segment 4: stubs returning each step's block


# From the date checks at seg4:0x0560. The background id lands in ds:0x30BC
# and, when non-zero, HOLIDAY.BMP is loaded over the scene.
HOLIDAYS = [
    (10, '29-31', 'Halloween'),
    (12, '23-25', 'Christmas'),
    (12, '29-31', 'New Year'),
    (1, '1', 'New Year'),
    (3, '15-17', "St Patrick's Day"),
]
HOLIDAY_ID = {'Halloween': 1, 'Christmas': 2, 'New Year': 3,
              "St Patrick's Day": 4}


def data_segment(path):
    d = open(path, 'rb').read()
    ne = struct.unpack_from('<I', d, 0x3C)[0]
    nsegs = struct.unpack_from('<H', d, ne + 0x1C)[0]
    segtab = struct.unpack_from('<H', d, ne + 0x22)[0]
    align = 1 << (struct.unpack_from('<H', d, ne + 0x32)[0] or 9)
    for i in range(nsegs):
        sector, length, flags, _ = struct.unpack_from('<HHHH', d, ne + segtab + i * 8)
        if flags & 1:
            return d[sector * align:sector * align + length]
    raise SystemExit('no data segment')


def ads_files(data):
    out = {}
    p = ADS_TABLE
    while True:
        fid = struct.unpack_from('<H', data, p)[0]
        if fid == 0:
            break
        name = data[p + 2:p + 15].split(b'\0')[0].decode('latin1')
        out[fid] = name or '(none)'
        p += RECORD
    return out


def scenes(data):
    out = []
    p = SCENE_TABLE
    while p + RECORD <= len(data):
        rec = data[p:p + RECORD]
        if struct.unpack_from('<H', rec, 0)[0] == 0:
            break
        out.append({
            'at': p,
            'id': struct.unpack_from('<H', rec, 0)[0],
            'weight': rec[2],
            'from': (rec[3], rec[4]),
            'to': (rec[5], rec[6]),
            'cost': rec[7],
            'min': rec[8],
            'max': rec[9],
            'day': rec[10],
            'mask': struct.unpack_from('<I', rec, 11)[0],
            'ads': rec[15],
            'seq': rec[16],
        })
        p += RECORD
    return out


def route_block(data, off):
    """A route block: -position markers, each followed by (step, weight)
    pairs, terminated by -7. The pair (0, 0) marks the target itself."""
    out = []
    p = off
    cur = None
    while p + 2 <= len(data):
        v = struct.unpack_from('<h', data, p)[0]
        if v == -7:
            break
        if v < 0:
            cur = -v
            out.append((cur, []))
            p += 2
            continue
        w = struct.unpack_from('<h', data, p + 2)[0]
        if out:
            out[-1][1].append((v, w))
        p += 4
    return out


def walk_steps(exe_path, data):
    """Step id -> (block offset, waypoint count). The block addresses come
    from a jump table of stubs in segment 4."""
    d = open(exe_path, 'rb').read()
    ne = struct.unpack_from('<I', d, 0x3C)[0]
    nsegs = struct.unpack_from('<H', d, ne + 0x1C)[0]
    segtab = struct.unpack_from('<H', d, ne + 0x22)[0]
    align = 1 << (struct.unpack_from('<H', d, ne + 0x32)[0] or 9)
    code = None
    seen = 0
    for i in range(nsegs):
        sector, length, flags, _ = struct.unpack_from('<HHHH', d, ne + segtab + i * 8)
        if not (flags & 1):
            seen += 1
            if seen == 4:
                code = d[sector * align:sector * align + length]
                break
    steps = {}
    for i in range(34):
        stub = struct.unpack_from('<H', code, STEP_TABLE + i * 2)[0]
        if stub + 3 <= len(code) and code[stub] == 0xB8:
            off = struct.unpack_from('<H', code, stub + 1)[0]
            pts = []
            p = off
            while p + 6 <= len(data):
                frame, x, y = struct.unpack_from('<HHH', data, p)
                if (frame, x, y) == (0, 0, 0):      # all-zero triple ends it
                    break
                pts.append((frame, x, y))
                p += 6
            steps[i + 1] = (off, pts)
    return steps


def print_walking(exe_path, data):
    print('route matrix at ds:%#06x -- word at +from*12+to*2' % ROUTE_MATRIX)
    print('        to:' + ''.join('%8d' % t for t in range(1, 7)))
    for f in range(1, 7):
        row = [struct.unpack_from('<H', data, ROUTE_MATRIX + f * 12 + t * 2)[0]
               for t in range(1, 7)]
        print('  from %d:  ' % f + ''.join('%8s' % ('-' if v == 0 else hex(v))
                                           for v in row))
    print()
    off = struct.unpack_from('<H', data, ROUTE_MATRIX + 1 * 12 + 2 * 2)[0]
    print('example route block, from 1 to 2, at %#06x:' % off)
    for pos, pairs in route_block(data, off):
        if pairs == [(0, 0)]:
            print('  at position %d: already there' % pos)
        else:
            total = sum(w for _, w in pairs)
            print('  at position %d: %s   (weights total %d)'
                  % (pos, ', '.join('step %d w%d' % (s, w) for s, w in pairs), total))
    print()
    steps = walk_steps(exe_path, data)
    print('%d walk steps. Each is a list of (frame, x, y) waypoints ending in an'
          % len(steps))
    print('all-zero triple; bit 15 of the frame mirrors the sprite horizontally.')
    for sid in sorted(steps):
        o, pts = steps[sid]
        a = data[STEP_POSITIONS + sid * 3:STEP_POSITIONS + sid * 3 + 3]
        flipped = sum(1 for f, _, _ in pts if f & 0x8000)
        head = ' '.join('(%d,%d)' % (x, y) for _, x, y in pts[:3])
        print('  step %2d  %#06x  %2d pts  pos %d->%d  %s%s%s'
              % (sid, o, len(pts), a[0], a[2], head,
                 ' ...' if len(pts) > 3 else '',
                 '  [mirrored]' if flipped == len(pts) and pts else ''))


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('exe', nargs='?', default='original/scrantic.exe')
    ap.add_argument('--walk', action='store_true',
                    help='dump the walk routes and step paths instead')
    args = ap.parse_args()

    if args.walk:
        print_walking(args.exe, data_segment(args.exe))
        return

    data = data_segment(args.exe)
    files = ads_files(data)

    print('ADS files (id -> name):')
    for fid, name in sorted(files.items()):
        print('  %5d  %s' % (fid, name))
    print()

    rows = scenes(data)
    print('scene table: %d entries at %#06x' % (len(rows), SCENE_TABLE))
    print()
    print('  #   id  wt  from    to    cost  window   day  categories  play')
    print('  --- --- --- ------- ----- ----- -------- ---- ----------- ----------------')
    for i, r in enumerate(rows):
        cats = [str(b) for b in range(8) if r['mask'] & (1 << b)]
        target = ('%s seq %d' % (files.get(r['ads'], '#%d' % r['ads']), r['seq'])
                  if r['seq'] else '(disabled)')
        print('  %3d %3d %3d (%d,%d) -> (%d,%d) %4d  %3d-%-3d  %3d  %-11s %s'
              % (i, r['id'], r['weight'], r['from'][0], r['from'][1],
                 r['to'][0], r['to'][1], r['cost'], r['min'], r['max'],
                 r['day'], ','.join(cats) or '-', target))

    print()
    print('story arc (day -> scene), from the day field:')
    by_day = {}
    for r in rows:
        if r['day']:
            by_day.setdefault(r['day'], []).append(
                '%s seq %d' % (files.get(r['ads'], '#%d' % r['ads']), r['seq']))
    for day in sorted(by_day):
        print('  day %2d  %s' % (day, ', '.join(by_day[day])))
    print()
    print('holidays (seg4:0x0560), by calendar date:')
    for month, days, which in HOLIDAYS:
        print('  %-22s month %2d, day %s -> background %d'
              % (which, month, days, HOLIDAY_ID[which]))
    print()
    print('time of day: (elapsed since StartTime) reduced modulo 16.')
    print('  phases 0-11 are day  -> one of OCEAN00/01/02.SCR at random')
    print('  phases 12-15 are night -> NIGHT.SCR')
    print('  scene windows in the table above are in these units.')
    print()
    moves = [r for r in rows if r['from'] != r['to']]
    print('%d of %d entries move Johnny between positions' % (len(moves), len(rows)))
    days = sorted(set(r['day'] for r in rows if r['day']))
    print('story-stage entries use days: %s' % (days or 'none'))
    print('positions referenced: %s'
          % sorted(set([r['from'] for r in rows] + [r['to'] for r in rows])))


if __name__ == '__main__':
    main()
