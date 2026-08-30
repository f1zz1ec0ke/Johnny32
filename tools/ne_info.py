#!/usr/bin/env python3
"""Dump and disassemble the 16-bit New Executable (SCRANTIC.EXE).

    python3 tools/ne_info.py original/scrantic.exe
    python3 tools/ne_info.py original/scrantic.exe --strings
    python3 tools/ne_info.py original/scrantic.exe --disasm 6 --start 0xfc1 --end 0x10d0

Disassembly needs `capstone` (pip install capstone).
"""

import argparse
import re
import struct


def parse(data):
    ne = struct.unpack_from('<I', data, 0x3C)[0]
    if data[ne:ne + 2] != b'NE':
        raise SystemExit('not a New Executable')
    info = {}
    info['ne'] = ne
    info['linker'] = (data[ne + 2], data[ne + 3])
    info['flags'] = struct.unpack_from('<H', data, ne + 0x0C)[0]
    info['heap'] = struct.unpack_from('<H', data, ne + 0x10)[0]
    info['stack'] = struct.unpack_from('<H', data, ne + 0x12)[0]
    info['csip'] = struct.unpack_from('<I', data, ne + 0x14)[0]
    info['sssp'] = struct.unpack_from('<I', data, ne + 0x18)[0]
    info['nsegs'] = struct.unpack_from('<H', data, ne + 0x1C)[0]
    info['nmods'] = struct.unpack_from('<H', data, ne + 0x1E)[0]
    info['segtab'] = struct.unpack_from('<H', data, ne + 0x22)[0]
    info['resnames'] = struct.unpack_from('<H', data, ne + 0x26)[0]
    info['modrefs'] = struct.unpack_from('<H', data, ne + 0x28)[0]
    info['imports'] = struct.unpack_from('<H', data, ne + 0x2A)[0]
    info['nonresnames'] = struct.unpack_from('<I', data, ne + 0x2C)[0]
    info['align'] = 1 << (struct.unpack_from('<H', data, ne + 0x32)[0] or 9)
    segs = []
    for i in range(info['nsegs']):
        o = ne + info['segtab'] + i * 8
        sector, length, flags, minalloc = struct.unpack_from('<HHHH', data, o)
        segs.append({'index': i + 1, 'offset': sector * info['align'],
                     'length': length, 'flags': flags,
                     'kind': 'DATA' if flags & 1 else 'CODE'})
    info['segments'] = segs
    return info


def imported_names(data, info):
    """Module reference table -> list of module names, in ordinal order."""
    ne = info['ne']
    mods = []
    for i in range(info['nmods']):
        nameoff = struct.unpack_from('<H', data, ne + info['modrefs'] + i * 2)[0]
        p = ne + info['imports'] + nameoff
        mods.append(data[p + 1:p + 1 + data[p]].decode('latin1'))
    return mods


# What a fixup patches, from the record's first byte.
ADDR_LOBYTE = 0
ADDR_SEGMENT = 2
ADDR_FAR = 3        # segment:offset, four bytes
ADDR_OFFSET = 5     # two bytes


def relocation_records(data, info, seg):
    """Per-segment relocation records, in full.

    A segment with flag 0x100 stores, right after its data, a uint16 count and
    that many 8-byte records. Borland leaves the instruction operand as a
    placeholder (0000:FFFF for a far call) and patches it at load time, so
    without these every imported call disassembles identically and uselessly.

    Each record carries what it patches (`addr`, one of the `ADDR_*` above)
    and whether it is additive. A record that is **not** additive names only
    the *first* site: the placeholder there holds the offset of the next site
    with the same target, and so on until 0xFFFF. Following that chain is not
    optional -- a target called from a dozen places has one record and eleven
    links, and an interpreter that patches only the record runs the other
    eleven calls straight into 0000:FFFF.
    """
    if not (seg['flags'] & 0x100):
        return []
    mods = imported_names(data, info)
    base = seg['offset'] + seg['length']
    if base + 2 > len(data):
        return []
    count = struct.unpack_from('<H', data, base)[0]
    out = []
    for i in range(count):
        o = base + 2 + i * 8
        if o + 8 > len(data):
            break
        addr_type = data[o]
        rel_type = data[o + 1]
        offset = struct.unpack_from('<H', data, o + 2)[0]
        a, b = struct.unpack_from('<HH', data, o + 4)
        kind = rel_type & 3
        if kind == 0:                                   # internal reference
            segno = a & 0xFF
            desc = 'seg%d:%#06x' % (segno, b) if segno != 0xFF else 'movable#%d' % b
        elif kind == 1:                                 # by ordinal
            mod = mods[a - 1] if 0 < a <= len(mods) else '?%d' % a
            desc = '%s.%d' % (mod, b)
        elif kind == 2:                                 # by name
            mod = mods[a - 1] if 0 < a <= len(mods) else '?%d' % a
            p = info['ne'] + info['imports'] + b
            desc = '%s.%s' % (mod, data[p + 1:p + 1 + data[p]].decode('latin1'))
        else:
            desc = 'osfixup'
        out.append({'offset': offset, 'desc': desc, 'addr': addr_type & 0x7F,
                    'additive': bool(rel_type & 4)})
    return out


def relocations(data, info, seg):
    """Fixup sites -> description, chains followed. For the disassembler."""
    body = data[seg['offset']:seg['offset'] + seg['length']]
    out = {}
    for rec in relocation_records(data, info, seg):
        for off in fixup_sites(body, rec):
            out[off] = rec['desc']
    return out


def fixup_sites(body, rec):
    """Every offset one relocation record patches, following its chain."""
    off = rec['offset']
    if rec['additive']:
        return [off]
    sites, seen = [], set()
    while 0 <= off < len(body) - 1 and off not in seen:
        seen.add(off)
        sites.append(off)
        nxt = struct.unpack_from('<H', body, off)[0]
        if nxt == 0xFFFF:
            break
        # A far-address chain links through the offset half only.
        off = nxt
    return sites


def names_at(data, off):
    out = []
    p = off
    while p < len(data) and data[p]:
        ln = data[p]
        name = data[p + 1:p + 1 + ln].decode('latin1')
        ordinal = struct.unpack_from('<H', data, p + 1 + ln)[0]
        out.append((name, ordinal))
        p += 1 + ln + 2
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('exe')
    ap.add_argument('--strings', action='store_true',
                    help='dump printable strings from the data segment')
    ap.add_argument('--disasm', type=int, metavar='SEG',
                    help='disassemble a code segment (1-based)')
    ap.add_argument('--start', type=lambda s: int(s, 0), default=0)
    ap.add_argument('--end', type=lambda s: int(s, 0), default=None)
    args = ap.parse_args()

    data = open(args.exe, 'rb').read()
    info = parse(data)
    ne = info['ne']

    if args.disasm:
        try:
            from capstone import Cs, CS_ARCH_X86, CS_MODE_16
        except ImportError:
            raise SystemExit('capstone not installed: pip install capstone')
        seg = info['segments'][args.disasm - 1]
        body = data[seg['offset']:seg['offset'] + seg['length']]
        end = args.end if args.end is not None else len(body)
        fixups = relocations(data, info, seg)
        md = Cs(CS_ARCH_X86, CS_MODE_16)
        for ins in md.disasm(body[args.start:end], args.start):
            note = ''
            for k in range(ins.size):
                if ins.address + k in fixups:
                    note = '   ; ' + fixups[ins.address + k]
                    break
            print('%04x  %-14s %s %s%s' % (ins.address, ins.bytes.hex(),
                                           ins.mnemonic, ins.op_str, note))
        return

    if args.strings:
        seg = next(s for s in info['segments'] if s['kind'] == 'DATA')
        body = data[seg['offset']:seg['offset'] + seg['length']]
        for m in re.finditer(rb'[\x20-\x7e]{4,}', body):
            print('%04x  %s' % (m.start(), m.group().decode('latin1')))
        return

    print('linker        %d.%d' % info['linker'])
    print('flags         %#06x' % info['flags'])
    print('CS:IP         %#010x' % info['csip'])
    print('SS:SP         %#010x' % info['sssp'])
    print('heap / stack  %d / %d' % (info['heap'], info['stack']))
    print()
    total = 0
    print('segments:')
    for s in info['segments']:
        print('  %2d  %s  file %#08x  %6d bytes  flags %#06x'
              % (s['index'], s['kind'], s['offset'], s['length'], s['flags']))
        if s['kind'] == 'CODE':
            total += s['length']
    print('  total code: %d bytes' % total)
    print()
    print('imports:      %s' % ', '.join(imported_names(data, info)))
    print('resident:     %s' % ', '.join(
        '%s@%d' % n for n in names_at(data, ne + info['resnames'])))
    print('non-resident: %s' % ', '.join(
        '%s@%d' % n for n in names_at(data, info['nonresnames'])))


if __name__ == '__main__':
    main()
