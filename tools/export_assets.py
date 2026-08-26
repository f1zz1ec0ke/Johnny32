#!/usr/bin/env python3
"""Export everything in RESOURCE.001 to PNGs and readable script dumps."""

import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dgds


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('-d', '--datadir', default='original',
                    help='directory holding RESOURCE.001 (default: original)')
    ap.add_argument('-o', '--outdir', default='assets')
    ap.add_argument('--sheets', action='store_true',
                    help='also write one contact sheet per multi-frame bitmap')
    args = ap.parse_args()

    data = open(os.path.join(args.datadir, 'RESOURCE.001'), 'rb').read()
    records = dict((name, body) for name, body, _ in dgds.read_archive(data))
    palette = dgds.read_palette(records['JOHNCAST.PAL'])

    for sub in ('backgrounds', 'sprites', 'sheets', 'scripts'):
        os.makedirs(os.path.join(args.outdir, sub), exist_ok=True)

    counts = {'backgrounds': 0, 'sprites': 0, 'scripts': 0}

    for name, body in records.items():
        stem = os.path.splitext(name)[0]
        ext = os.path.splitext(name)[1].upper()

        if ext == '.SCR':
            w, h, px = dgds.read_screen(body)
            dgds.write_png(os.path.join(args.outdir, 'backgrounds', stem + '.png'),
                           w, h, px, palette)
            counts['backgrounds'] += 1

        elif ext == '.BMP':
            frames = dgds.read_bitmap(body)
            for i, (w, h, px) in enumerate(frames):
                out = os.path.join(args.outdir, 'sprites', '%s_%02d.png' % (stem, i))
                dgds.write_png(out, w, h, px, palette, transparent=0)
            counts['sprites'] += len(frames)
            if args.sheets and len(frames) > 1:
                write_sheet(os.path.join(args.outdir, 'sheets', stem + '.png'),
                            frames, palette)

        elif ext in ('.TTM', '.ADS'):
            dump_script(os.path.join(args.outdir, 'scripts', stem + ext.lower() + '.txt'),
                        name, body)
            counts['scripts'] += 1

    print('backgrounds : %d' % counts['backgrounds'])
    print('sprite frames: %d' % counts['sprites'])
    print('scripts     : %d' % counts['scripts'])
    print('\nwrote %s/' % args.outdir)


def write_sheet(path, frames, palette):
    cols = min(len(frames), 8)
    rows = (len(frames) + cols - 1) // cols
    cw = max(f[0] for f in frames) + 2
    ch = max(f[1] for f in frames) + 2
    W, H = cw * cols, ch * rows
    canvas = bytearray(W * H)
    for i, (w, h, px) in enumerate(frames):
        cx = (i % cols) * cw + 1
        cy = (i // cols) * ch + 1
        for y in range(h):
            canvas[(cy + y) * W + cx:(cy + y) * W + cx + w] = px[y * w:(y + 1) * w]
    dgds.write_png(path, W, H, bytes(canvas), palette, transparent=0)


def dump_script(path, name, body):
    chunks = dict((p[-1], payload) for p, payload in dgds.flatten(dgds.parse_chunks(body)))
    lines = ['# %s' % name, '']
    if 'VER:' in chunks:
        lines.append('version: %s' % chunks['VER:'].split(b'\0')[0].decode('latin1'))
    if 'PAG:' in chunks:
        lines.append('pages:   %d' % int.from_bytes(chunks['PAG:'][:2], 'little'))
    if 'RES:' in chunks:
        lines.append('')
        lines.append('resources:')
        for k, v in sorted(dgds.parse_id_table(chunks['RES:']).items()):
            lines.append('  %3d  %s' % (k, v))
    tags = dgds.parse_id_table(chunks['TAG:']) if 'TAG:' in chunks else {}
    if tags:
        lines.append('')
        lines.append('scene labels:')
        for k, v in sorted(tags.items()):
            lines.append('  %3d  %s' % (k, v))
    code = chunks.get('TT3:') or chunks.get('SCR:')
    if code:
        lines.append('')
        lines.append('bytecode (%d bytes) -- best effort, see docs/ANALYSIS.md:' % len(code))
        reached = 0
        for off, op, argv, text in dgds.disasm(code):
            if text is not None:
                lines.append('  %04X  %04X  %r' % (off, op, text))
            else:
                lines.append('  %04X  %04X  %s' % (off, op, ' '.join(str(a) for a in argv)))
            reached = off
        if reached < len(code) - 4:
            lines.append('  ---- decoding stopped at %04X of %04X ----' % (reached, len(code)))
    with open(path, 'w') as fh:
        fh.write('\n'.join(lines) + '\n')


if __name__ == '__main__':
    main()
