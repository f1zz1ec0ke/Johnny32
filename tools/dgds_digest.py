#!/usr/bin/env python3
"""Print a canonical digest of everything decoded out of RESOURCE.001.

The Rust port's `jcdump` prints byte-identical lines, so diffing the two
answers "does the port decode this correctly" for every chunk and every sprite
frame rather than for a sample. See johnny/src/bin/jcdump.rs.
"""

import os
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dgds


def crc(b):
    return '%08x' % (zlib.crc32(b) & 0xFFFFFFFF)


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else 'original/RESOURCE.001'
    data = open(path, 'rb').read()
    records = dgds.read_archive(data)
    failed = False

    print('records\t%d' % len(records))
    for name, body, _ in records:
        print('record\t%s\t%d' % (name, len(body)))

        for path_tags, payload in dgds.flatten(dgds.parse_chunks(body)):
            tag = path_tags[-1]
            if dgds.is_compressed(name, tag):
                try:
                    out = dgds.decompress(payload)
                    print('  chunk\t%s\t%d\t%s' % (tag, len(out), crc(out)))
                except Exception as exc:                     # noqa: BLE001
                    print('  chunk\t%s\tERROR\t%s' % (tag, exc))
                    failed = True
            else:
                print('  raw\t%s\t%d\t%s' % (tag, len(payload), crc(payload)))

        upper = name.upper()
        if upper.endswith('.BMP'):
            try:
                for i, (w, h, px) in enumerate(dgds.read_bitmap(body)):
                    print('  frame\t%d\t%dx%d\t%s' % (i, w, h, crc(px)))
            except Exception as exc:                         # noqa: BLE001
                print('  frame\tERROR\t%s' % exc)
                failed = True
        elif upper.endswith('.SCR'):
            try:
                w, h, px = dgds.read_screen(body)
                print('  screen\t%dx%d\t%s' % (w, h, crc(px)))
            except Exception as exc:                         # noqa: BLE001
                print('  screen\tERROR\t%s' % exc)
                failed = True
        elif upper.endswith('.TTM'):
            chunks = dict((q[-1], pl) for q, pl in
                          dgds.flatten(dgds.parse_chunks(body)))
            if 'TT3:' not in chunks:
                print('  ttm\tERROR\tno TT3: chunk')
                failed = True
            else:
                code = dgds.decompress(chunks['TT3:'])
                ops = dgds.disasm(code, dgds.TTM_OPCODES)
                consumed = 0
                if ops:
                    off, op, argv, text = ops[-1]
                    if text is not None:
                        consumed = off + 2 + len(text) + 1
                        if consumed & 1:
                            consumed += 1
                    else:
                        consumed = off + 2 + len(argv) * 2
                print('  ttm\t%d\t%d\t%d' % (len(ops), consumed, len(code)))
        elif upper.endswith('.ADS'):
            chunks = dict((q[-1], pl) for q, pl in
                          dgds.flatten(dgds.parse_chunks(body)))
            if 'SCR:' not in chunks:
                print('  ads\tERROR\tno SCR: chunk')
                failed = True
            else:
                code = dgds.decompress(chunks['SCR:'])
                ops = dgds.disasm_ads(code)
                consumed = 0
                if ops:
                    off, op, argv = ops[-1]
                    consumed = off + dgds.ADS_LENGTHS.get(op, 2)
                print('  ads\t%d\t%d\t%d' % (len(ops), consumed, len(code)))
        elif upper.endswith('.PAL'):
            try:
                pal = dgds.read_palette(body)
                flat = bytes(v for entry in pal for v in entry)
                print('  palette\t%d\t%s' % (len(flat), crc(flat)))
            except Exception as exc:                         # noqa: BLE001
                print('  palette\tERROR\t%s' % exc)
                failed = True

    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
