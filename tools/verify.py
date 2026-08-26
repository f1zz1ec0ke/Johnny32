#!/usr/bin/env python3
"""Self-check: every record and every compressed chunk in RESOURCE.001.

Regression test for the decoder. A compressed chunk declares its own
uncompressed size, so decoding to exactly that size is a real check, not a
tautology -- the block-reading LZW behaviour is what makes the large files pass.
"""

import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dgds
import scheduler

datadir = sys.argv[1] if len(sys.argv) > 1 else 'original'
data = open(os.path.join(datadir, 'RESOURCE.001'), 'rb').read()
mapdata = open(os.path.join(datadir, 'RESOURCE.MAP'), 'rb').read()

records = dgds.read_archive(data)
consumed = sum(17 + len(body) for _, body, _ in records)
archive, entries = dgds.read_map(mapdata)

failures = []

if consumed != len(data):
    failures.append('archive walk consumed %d of %d bytes' % (consumed, len(data)))
if len(entries) != len(records):
    failures.append('map lists %d entries, archive holds %d'
                    % (len(entries), len(records)))
for (name, _, off), (_, mapoff) in zip(records, entries):
    if off != mapoff:
        failures.append('%s at %d but map says %d' % (name, off, mapoff))

chunks = 0
for name, body, _ in records:
    for path, payload in dgds.flatten(dgds.parse_chunks(body)):
        if not dgds.is_compressed(name, path[-1]):
            continue
        chunks += 1
        usize = struct.unpack_from('<I', payload, 1)[0]
        try:
            out = dgds.decompress(payload)
        except Exception as exc:                     # noqa: BLE001
            failures.append('%s %s raised %s' % (name, path[-1], exc))
            continue
        if len(out) != usize:
            failures.append('%s %s decoded %d, expected %d'
                            % (name, path[-1], len(out), usize))

frames = 0
for name, body, _ in records:
    if name.upper().endswith('.BMP'):
        try:
            frames += len(dgds.read_bitmap(body))
        except Exception as exc:                     # noqa: BLE001
            failures.append('%s failed to decode: %s' % (name, exc))
    elif name.upper().endswith('.SCR'):
        try:
            w, h, px = dgds.read_screen(body)
            if len(px) != w * h:
                failures.append('%s gave %d pixels for %dx%d' % (name, len(px), w, h))
        except Exception as exc:                     # noqa: BLE001
            failures.append('%s failed to decode: %s' % (name, exc))

ttm_ok = ttm_total = 0
ads_ok = ads_total = 0
for name, body, _ in records:
    leaves = dict((path[-1], payload) for path, payload in dgds.flatten(dgds.parse_chunks(body)))
    if 'TT3:' in leaves:
        ttm_total += 1
        code = dgds.decompress(leaves['TT3:'])
        ops = dgds.disasm(code, dgds.TTM_OPCODES)
        consumed = 0
        if ops:
            off, op, argv, text = ops[-1]
            consumed = off + 2 + (len(text) + 1 + ((off + len(text) + 3) & 1)
                                  if text is not None else len(argv) * 2)
        if consumed >= len(code) - 2:
            ttm_ok += 1
        else:
            failures.append('%s disassembly stopped at %d of %d'
                            % (name, consumed, len(code)))
    elif 'SCR:' in leaves and name.upper().endswith('.ADS'):
        ads_total += 1
        code = dgds.decompress(leaves['SCR:'])
        ops = dgds.disasm_ads(code)
        consumed = 0
        if ops:
            off, op, argv = ops[-1]
            consumed = off + dgds.ADS_LENGTHS.get(op, 2)
        if consumed == len(code):
            ads_ok += 1
        else:
            failures.append('%s consumed %d of %d bytes'
                            % (name, consumed, len(code)))

# The scheduler tables live in the executable, not the archive, so only check
# them when scrantic.exe has been extracted alongside the data.
sched_note = 'skipped (no scrantic.exe)'
exe = os.path.join(datadir, 'scrantic.exe')
if os.path.exists(exe):
    seg = scheduler.data_segment(exe)
    files = scheduler.ads_files(seg)
    rows = scheduler.scenes(seg)
    archive_names = set(n.upper() for n, _, _ in records)
    for fid, fname in files.items():
        if fname != '(none)' and fname.upper() not in archive_names:
            failures.append('scheduler references missing ADS file %s' % fname)
    enabled = [r for r in rows if r['seq']]
    for r in enabled:
        if r['ads'] not in files and r['ads'] != 0xFF:
            failures.append('scene at %#06x references unknown ADS id %d'
                            % (r['at'], r['ads']))
    days = sorted(set(r['day'] for r in rows if r['day']))
    if days != list(range(1, 12)):
        failures.append('story days are %s, expected 1..11' % days)
    # Route weight lists should sum to 100, because the picker draws
    # rand() % 100. One block in the shipped data does not -- see below.
    odd_routes = []
    for f in range(1, 7):
        for t in range(1, 7):
            if f == t:
                continue
            off = struct.unpack_from('<H', seg, scheduler.ROUTE_MATRIX
                                     + f * 12 + t * 2)[0]
            if not off:
                continue
            for pos, pairs in scheduler.route_block(seg, off):
                total = sum(w for _, w in pairs)
                if pairs and total not in (0, 100):
                    odd_routes.append((f, t, pos, total))
    if odd_routes != [(1, 5, 1, 150)]:
        failures.append('route weight totals changed: expected only the known '
                        '1->5 quirk, got %s' % odd_routes)
    steps = scheduler.walk_steps(exe, seg)
    sched_note = ('%d ADS files, %d scenes (%d enabled), story days %d-%d, '
                  '%d walk paths (route 1->5 sums to 150 -- original bug)'
                  % (len(files) - 1, len(rows), len(enabled), days[0], days[-1],
                     len(steps)))

print('archive      %s, %d records, %d bytes, fully consumed'
      % (archive, len(records), len(data)))
print('compressed   %d chunks' % chunks)
print('sprites      %d frames' % frames)
print('TTM scripts  %d/%d disassemble end to end' % (ttm_ok, ttm_total))
print('ADS scripts  %d/%d consume exactly' % (ads_ok, ads_total))
print('scheduler    %s' % sched_note)

if failures:
    print('\nFAILURES (%d):' % len(failures))
    for f in failures[:40]:
        print('  ' + f)
    sys.exit(1)
print('\nall good')
