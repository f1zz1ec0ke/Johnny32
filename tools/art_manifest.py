#!/usr/bin/env python3
"""What an artist would have to redraw, and in what order.

`export_assets.py` already writes every frame out as a PNG. This says which of
them are worth the effort: how often each bitmap is actually put on screen by
the scripts, what it is used for, and how big it is.

    python3 tools/art_manifest.py                 # the summary
    python3 tools/art_manifest.py --csv art.csv   # one row per bitmap

The counts are static -- how many draw instructions name each bitmap across all
41 scripts -- not how often they run. A frame inside a loop is on screen far
longer than its one instruction suggests, so read the order as a guide rather
than a measurement.
"""

import argparse
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dgds

LOAD_BITMAP = 0xF02F
SELECT_SLOT = 0x1051
SPRITE = (0xA504, 0xA514, 0xA524, 0xA534)


def instructions(code):
    """Walk a TTM script. The low nibble is the operand count; 0x0F is a
    NUL-terminated string padded to a word."""
    p = 0
    while p + 2 <= len(code):
        op = int.from_bytes(code[p:p + 2], 'little')
        n = op & 0x0F
        size, text = 2 + n * 2, None
        if n == 0x0F:
            end = code.find(b'\x00', p + 2)
            if end < 0:
                break
            text = code[p + 2:end].decode('latin1')
            size = (end + 1) - p
            size += size & 1
        yield op, code[p + 2:p + size], text
        p += size


def leaf(body, tag):
    for path, payload in dgds.flatten(dgds.parse_chunks(body)):
        if path[-1] == tag:
            try:
                return dgds.decompress(payload)
            except Exception:                       # noqa: BLE001 - stored raw
                return payload
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('-d', '--datadir', default='original')
    ap.add_argument('--csv', help='write one row per bitmap here')
    ap.add_argument('-n', '--top', type=int, default=25)
    args = ap.parse_args()

    data = open(os.path.join(args.datadir, 'RESOURCE.001'), 'rb').read()
    records = [(n, b) for n, b, _ in dgds.read_archive(data)]
    bodies = dict(records)

    frames = {}
    for name, body in records:
        if name.endswith('.BMP'):
            try:
                frames[name] = [(w, h) for w, h, _ in dgds.read_bitmap(body)]
            except Exception:                       # noqa: BLE001
                pass

    # bitmap -> how many draw instructions name it, and which TTM files do.
    draws = collections.Counter()
    used_by = collections.defaultdict(set)
    for name, body in records:
        if not name.endswith('.TTM'):
            continue
        code = leaf(body, 'TT3:')
        if not code:
            continue
        slot, slots = 0, {}
        for op, operands, text in instructions(code):
            if op == SELECT_SLOT:
                slot = int.from_bytes(operands[0:2], 'little')
            elif op == LOAD_BITMAP and text:
                slots[slot] = text
            elif op in SPRITE and len(operands) >= 8:
                who = slots.get(int.from_bytes(operands[6:8], 'little'))
                if who:
                    draws[who] += 1
                    used_by[who].add(name)

    # TTM file -> the named sequences that play it, through each ADS `RES:`.
    plays = collections.defaultdict(set)
    for name, body in records:
        if not name.endswith('.ADS'):
            continue
        res = leaf(body, 'RES:')
        tag = leaf(body, 'TAG:')
        files = dict(entries(res))
        labels = dict(entries(tag))
        code = leaf(body, 'SCR:')
        if not code:
            continue
        sequence = None
        p = 0
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            n = ads_length(op)
            if 0 < op <= 0xFF:
                sequence = op
            elif op in (0x2000, 0x2005) and sequence is not None:
                ttm = files.get(int.from_bytes(code[p + 2:p + 4], 'little'))
                if ttm:
                    plays[ttm].add('%s %s' % (name.split('.')[0],
                                              labels.get(sequence, '#%d' % sequence)))
            p += n

    rows = []
    for name, fs in frames.items():
        where = set()
        for ttm in used_by.get(name, ()):
            where |= plays.get(ttm, set())
        rows.append({
            'bitmap': name,
            'frames': len(fs),
            'draws': draws.get(name, 0),
            'pixels': sum(w * h for w, h in fs),
            'biggest': '%dx%d' % max(fs, key=lambda wh: wh[0] * wh[1]) if fs else '-',
            'used_in': sorted(where),
        })
    rows.sort(key=lambda r: (-r['draws'], r['bitmap']))

    total = sum(r['draws'] for r in rows) or 1
    print('%-14s %6s %6s %8s  %s' % ('bitmap', 'frames', 'draws', 'biggest', 'seen in'))
    run = 0
    for r in rows[:args.top]:
        run += r['draws']
        seen = ', '.join(r['used_in'][:2]) or '-'
        if len(r['used_in']) > 2:
            seen += ' +%d more' % (len(r['used_in']) - 2)
        print('%-14s %6d %6d %8s  %s' % (r['bitmap'], r['frames'], r['draws'],
                                         r['biggest'], seen[:52]))
    print()
    print('%d bitmaps, %d frames, %.1fM pixels of artwork in all'
          % (len(rows), sum(r['frames'] for r in rows),
             sum(r['pixels'] for r in rows) / 1e6))
    for k in (5, 10, 20, 30, 50):
        if k <= len(rows):
            share = 100 * sum(r['draws'] for r in rows[:k]) / total
            nf = sum(r['frames'] for r in rows[:k])
            print('  the busiest %-3d bitmaps are %4d frames and %4.1f%% of every draw'
                  % (k, nf, share))

    if args.csv:
        with open(args.csv, 'w') as fh:
            fh.write('bitmap,frames,draws,pixels,biggest,used_in\n')
            for r in rows:
                fh.write('%s,%d,%d,%d,%s,"%s"\n'
                         % (r['bitmap'], r['frames'], r['draws'], r['pixels'],
                            r['biggest'], '; '.join(r['used_in'])))
        print('\nwrote %s' % args.csv)


def entries(chunk):
    """A `RES:`/`TAG:` table: uint16 count, then id + NUL-terminated name."""
    if not chunk:
        return
    p = 2
    while p + 3 <= len(chunk):
        ident = int.from_bytes(chunk[p:p + 2], 'little')
        end = chunk.find(b'\x00', p + 2)
        if end < 0:
            return
        yield ident, chunk[p + 2:end].decode('latin1')
        p = end + 1


def ads_length(opcode):
    """From `seg10:0x0303`; see `johnny/src/ads.rs`."""
    if opcode in (0x2000, 0x2005):
        return 10
    if opcode in (0x2010, 0x2015, 0x2020, 0x4000, 0x4010):
        return 8
    if (0x1010 <= opcode <= 0x1070 or 0x1310 <= opcode <= 0x1370) and opcode & 0xF == 0:
        return 6
    if opcode in (0xF010, 0xF200, 0xF210, 0x1080, 0x1380, 0x1390, 0x3020):
        return 4
    if opcode in (0x13A0, 0x13A1, 0x13B0, 0x13B1, 0x13C0, 0x13C1):
        return 4
    return 2


if __name__ == '__main__':
    main()
