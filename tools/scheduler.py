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


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('exe', nargs='?', default='original/scrantic.exe')
    args = ap.parse_args()

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
