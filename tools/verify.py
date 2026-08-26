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

print('archive      %s, %d records, %d bytes, fully consumed'
      % (archive, len(records), len(data)))
print('compressed   %d chunks' % chunks)
print('sprites      %d frames' % frames)

if failures:
    print('\nFAILURES (%d):' % len(failures))
    for f in failures[:40]:
        print('  ' + f)
    sys.exit(1)
print('\nall good')
