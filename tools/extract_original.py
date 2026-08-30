#!/usr/bin/env python3
"""Recover the original 1992 Dynamix files from the screensaverplanet installer.

    johnnycastaway.exe            NSIS installer, 32-bit
      -> Screen Antics.scr        UPX-packed 32-bit Delphi launcher
           -> scrantic.exe        the original 16-bit NE screensaver
           -> RESOURCE.001        the original DGDS data archive
           -> RESOURCE.MAP        the original archive index

The Delphi layer carries the three originals as RCDATA resources and does
nothing at runtime but drop them next to itself and ShellExecute the 16-bit
binary -- which is why the whole thing dies on 64-bit Windows.

Needs `7z` (NSIS) and `upx` (the packed launcher) on PATH.
"""

import argparse
import os
import shutil
import struct
import subprocess
import sys
import tempfile

RESOURCES = {
    'SCREENANTICSEXECUTABLE': 'scrantic.exe',
    'SCREENANTICSDATAFILE': 'RESOURCE.001',
    'SCREENANTICSMAPFILE': 'RESOURCE.MAP',
}


def _sections(data, opt, optsize, nsections):
    secs = []
    base = opt + optsize
    for i in range(nsections):
        o = base + i * 40
        name = data[o:o + 8].rstrip(b'\0').decode('latin1')
        vsize, vaddr, rsize, raddr = struct.unpack_from('<IIII', data, o + 8)
        secs.append((name, vsize, vaddr, rsize, raddr))
    return secs


def extract_rcdata(pe_path):
    """Pull the named RCDATA resources out of an unpacked PE32 image."""
    data = open(pe_path, 'rb').read()
    e_lfanew = struct.unpack_from('<I', data, 0x3C)[0]
    if data[e_lfanew:e_lfanew + 4] != b'PE\0\0':
        raise SystemExit('%s is not a PE image' % pe_path)
    nsections = struct.unpack_from('<H', data, e_lfanew + 6)[0]
    optsize = struct.unpack_from('<H', data, e_lfanew + 20)[0]
    opt = e_lfanew + 24
    secs = _sections(data, opt, optsize, nsections)

    def rva_to_off(rva):
        for _, vsize, vaddr, rsize, raddr in secs:
            if vaddr <= rva < vaddr + max(vsize, rsize):
                return raddr + (rva - vaddr)
        raise ValueError('unmapped RVA %#x' % rva)

    rsrc_rva = struct.unpack_from('<I', data, opt + 96 + 2 * 8)[0]
    base = rva_to_off(rsrc_rva)
    found = {}

    def walk(off, level, path):
        nnamed, nid = struct.unpack_from('<HH', data, off + 12)
        for i in range(nnamed + nid):
            name_val, off_val = struct.unpack_from('<II', data, off + 16 + i * 8)
            if name_val & 0x80000000:
                so = base + (name_val & 0x7FFFFFFF)
                ln = struct.unpack_from('<H', data, so)[0]
                name = data[so + 2:so + 2 + ln * 2].decode('utf-16le')
            else:
                name = str(name_val)
            if off_val & 0x80000000:
                walk(base + (off_val & 0x7FFFFFFF), level + 1, path + [name])
            else:
                do = base + off_val
                drva, dsize = struct.unpack_from('<II', data, do)
                if len(path) >= 2 and path[1] in RESOURCES:
                    o = rva_to_off(drva)
                    found[path[1]] = data[o:o + dsize]

    walk(base, 0, [])
    return found


def need(tool):
    if shutil.which(tool) is None:
        raise SystemExit('%s not found on PATH (needed to unpack the installer)' % tool)


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('installer', nargs='?', default='johnnycastaway.exe')
    ap.add_argument('-o', '--outdir', default='original')
    args = ap.parse_args()

    need('7z')
    need('upx')
    os.makedirs(args.outdir, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run(['7z', 'x', '-y', '-o' + tmp, args.installer],
                       check=True, stdout=subprocess.DEVNULL)
        scr = os.path.join(tmp, 'Screen Antics.scr')
        if not os.path.exists(scr):
            raise SystemExit('no "Screen Antics.scr" inside %s' % args.installer)
        subprocess.run(['upx', '-d', '-q', scr], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        blobs = extract_rcdata(scr)

    missing = set(RESOURCES) - set(blobs)
    if missing:
        raise SystemExit('missing resources: %s' % ', '.join(sorted(missing)))

    for res, filename in RESOURCES.items():
        path = os.path.join(args.outdir, filename)
        with open(path, 'wb') as fh:
            fh.write(blobs[res])
        print('%-14s %9d bytes' % (filename, len(blobs[res])))
    print('\nwrote %s/' % args.outdir)


if __name__ == '__main__':
    main()
