#!/usr/bin/env python3
"""Self-check: every record and every compressed chunk in RESOURCE.001.

Regression test for the decoder. A compressed chunk declares its own
uncompressed size, so decoding to exactly that size is a real check, not a
tautology -- the block-reading LZW behaviour is what makes the large files pass.
"""

import os
import struct
import re
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

# The opcode census, against the lists the port carries.
#
# Several readings of the disassembly differ only where the data never goes --
# the general pre-shift, the string-length exception, which of `4000` and
# `4010` is the front of the draw order. The port writes down what the data
# actually contains and leans on it; this is what stops that going stale.

def _ads_length(op):
    if op in (0x2000, 0x2005): return 10
    if op in (0x2010, 0x2015, 0x2020, 0x4000, 0x4010): return 8
    if (0x1010 <= op <= 0x1070 or 0x1310 <= op <= 0x1370) and op & 0xF == 0: return 6
    if op in (0xF010, 0xF200, 0xF210, 0x1080, 0x1380, 0x1390, 0x3020): return 4
    if op in (0x13A0, 0x13A1, 0x13B0, 0x13B1, 0x13C0, 0x13C1): return 4
    return 2


def _census():
    ttm_used, ads_used = set(), set()
    for name, body, _ in records:
        want = 'TT3:' if name.endswith('.TTM') else ('SCR:' if name.endswith('.ADS') else None)
        if want is None:
            continue
        code = None
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == want:
                try:
                    code = dgds.decompress(payload)
                except Exception:
                    code = payload
        if not code:
            continue
        p = 0
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            if want == 'TT3:':
                n = op & 0x0F
                size = 2 + n * 2
                if n == 0x0F:
                    e = code.find(b'\x00', p + 2)
                    if e < 0:
                        break
                    size = (e + 1) - p
                    size += size & 1
                ttm_used.add(op)
            else:
                size = _ads_length(op)
                if not (0 < op <= 0xFF):
                    ads_used.add(op)
            p += size
    return ttm_used, ads_used


def _res_table(payload):
    """A count, then that many (id word, NUL-terminated name) pairs."""
    out = {}
    if len(payload) < 2:
        return out
    i = 2
    for _ in range(int.from_bytes(payload[0:2], 'little')):
        if i + 2 > len(payload):
            break
        rid = int.from_bytes(payload[i:i + 2], 'little')
        i += 2
        end = payload.find(b'\0', i)
        if end < 0:
            break
        out[rid] = payload[i:end].decode('latin-1')
        i = end + 1
    return out


def _leaf(body, want):
    code = None
    for path, payload in dgds.flatten(dgds.parse_chunks(body)):
        if path[-1] == want:
            try:
                code = dgds.decompress(payload)
            except Exception:
                code = payload
    return code


def _ttm_scenes(code):
    """The scene numbers a TTM defines: what `seg11:0x074A` will accept."""
    scenes, p = set(), 0
    while p + 2 <= len(code):
        op = int.from_bytes(code[p:p + 2], 'little')
        n = op & 0x0F
        if n == 0x0F:
            e = code.find(b'\x00', p + 2)
            if e < 0:
                break
            size = (e + 1) - p
            p += size + (size & 1)
            continue
        if op in (0x1101, 0x1111):
            scenes.add(int.from_bytes(code[p + 2:p + 4], 'little'))
        p += 2 + n * 2
    return scenes


def _headers():
    """Every word <= 0xFF in a script is a sequence header, and every sequence
    starts where the port thinks it does.

    `seg10:0x0C30` scans forward from a sequence's stored pointer for an
    instruction whose opcode word is `0005` and resumes just after it,
    stopping at the `FFFF` that closes the sequence -- so a body holding one
    would start somewhere the port's "just after the header word" does not.
    An `0005` is also a word <= 0xFF, which is exactly what marks a header, so
    it would be read as a second sequence 5 rather than as anything unusual.

    Both readings survive only because every such word in the shipped scripts
    follows an `FFFF`.
    """
    bad, headers = [], 0
    for name, code in _ads_scripts():
        after_end, seen = True, set()
        p = 0
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            if 0 < op <= 0xFF:
                headers += 1
                if not after_end:
                    bad.append('%s @%04X: %04X is inside a sequence body' % (name, p, op))
                if op in seen:
                    bad.append('%s @%04X: sequence %d is declared twice' % (name, p, op))
                seen.add(op)
            after_end = op == 0xFFFF
            p += _ads_length(op)
    return headers, bad


def _ads_scripts():
    for name, body, _ in records:
        if not name.endswith('.ADS'):
            continue
        code = _leaf(body, 'SCR:')
        if code:
            yield name, code


def _frames():
    """Every scene marker is the first instruction of its frame.

    `seg12:0x13AA` resolves a scene number by walking the frame table and
    testing only the *first* opcode of each frame, so a marker anywhere else
    names nothing -- while the port, which searches the decoded instructions,
    would find it wherever it sat. That is the check.

    The count of files whose frame 0 is itself a named scene comes back with
    it. The port runs frame 0 once when a file loads, for the palette and
    backdrop in it, and in `WOULDBE.TTM` -- the only one -- that frame is
    scene 3. It loads the same palette and the same backdrop as the other
    forty and draws nothing, so running it is the same either way; the number
    is here so that stays true rather than stays assumed.
    """
    late, first, markers = [], [], 0
    for name, body, _ in records:
        if not name.endswith('.TTM'):
            continue
        code = _leaf(body, 'TT3:')
        if not code:
            continue
        p, at_start = 0, True
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            n = op & 0x0F
            if n == 0x0F:
                e = code.find(b'\x00', p + 2)
                if e < 0:
                    break
                size = (e + 1) - p
                p += size + (size & 1)
                at_start = False
                continue
            if op in (0x1101, 0x1111):
                markers += 1
                if not at_start:
                    late.append('%s @%04X is a marker inside a frame' % (name, p))
                if p == 0:
                    first.append(name)
            at_start = op == 0x0FF0
            p += 2 + n * 2
    return markers, late, first


def _palettes():
    """There is one palette in the archive and every script loads that one.

    The engine keeps a palette per slot: `F05F` loads into the slot `1061`
    last selected (`seg12:0x099F`) and records the pointer, and it is `1061`
    that *applies* one (`seg12:0x098A`, when the display is 16 colours). The
    port has no slots and applies at load. The two can only differ if a script
    ever loads a second palette or reaches back for an earlier one, so this
    counts what the data actually holds.
    """
    in_archive = [n for n, _, _ in records if n.endswith('.PAL')]
    loaded, files = set(), 0
    for name, body, _ in records:
        if not name.endswith('.TTM'):
            continue
        code = _leaf(body, 'TT3:')
        if not code:
            continue
        files += 1
        p, seen = 0, 0
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            n = op & 0x0F
            if n == 0x0F:
                e = code.find(b'\x00', p + 2)
                if e < 0:
                    break
                if op == 0xF05F:
                    loaded.add(code[p + 2:e].decode('latin-1'))
                    seen += 1
                size = (e + 1) - p
                p += size + (size & 1)
                continue
            p += 2 + n * 2
        if seen != 1:
            loaded.add('%s loads %d palettes' % (name, seen))
    bad = []
    if len(in_archive) != 1:
        bad.append('the archive holds %d palettes: %s' % (len(in_archive), in_archive))
    if loaded != set(in_archive):
        bad.append('the scripts load %s' % sorted(loaded))
    return in_archive, files, bad


def _references():
    """Does every scene the scripts name actually exist?

    `seg11:0x074A` refuses a scene index outside the TTM's scene table or
    whose entry is null, and `seg10:0x1DBA` puts that instance straight into
    state 4 -- one pass in which the block waiting on it fires, then idle.
    The port has that path too, but it is a path neither side should ever
    take, and the difference between "never taken" and "taken and handled
    differently" is worth keeping true rather than remembering. The same
    goes for a `1201` goto naming a scene its own script does not define.
    """
    bodies = dict((n, b) for n, b, _ in records)
    scenes_of, refs, bad = {}, 0, []

    def scenes(name):
        if name not in scenes_of:
            body = bodies.get(name)
            code = _leaf(body, 'TT3:') if body is not None else None
            scenes_of[name] = _ttm_scenes(code) if code else None
        return scenes_of[name]

    for name, body, _ in records:
        if name.endswith('.TTM'):
            code = _leaf(body, 'TT3:')
            if not code:
                continue
            have, p = _ttm_scenes(code), 0
            while p + 2 <= len(code):
                op = int.from_bytes(code[p:p + 2], 'little')
                n = op & 0x0F
                if n == 0x0F:
                    e = code.find(b'\x00', p + 2)
                    if e < 0:
                        break
                    size = (e + 1) - p
                    p += size + (size & 1)
                    continue
                if op == 0x1201:
                    refs += 1
                    target = int.from_bytes(code[p + 2:p + 4], 'little')
                    if target not in have:
                        bad.append('%s @%04X gotos scene %d, which it does not define'
                                   % (name, p, target))
                p += 2 + n * 2
            continue
        if not name.endswith('.ADS'):
            continue
        code = _leaf(body, 'SCR:')
        res = None
        for path, payload in dgds.flatten(dgds.parse_chunks(body)):
            if path[-1] == 'RES:':
                res = _res_table(payload)
        if not code or res is None:
            continue
        p = 0
        while p + 2 <= len(code):
            op = int.from_bytes(code[p:p + 2], 'little')
            size = _ads_length(op)
            if op in (0x2000, 0x2005, 0x2010, 0x2015, 0x2020) and p + 6 <= len(code):
                refs += 1
                ttm = int.from_bytes(code[p + 2:p + 4], 'little')
                scene = int.from_bytes(code[p + 4:p + 6], 'little')
                fname = res.get(ttm)
                if fname is None:
                    bad.append('%s plays TTM id %d, which its RES: table has not got'
                               % (name, ttm))
                elif scenes(fname) is None:
                    bad.append('%s plays %s, which is not in the archive' % (name, fname))
                elif scene not in scenes(fname):
                    bad.append('%s plays %s scene %d, which that TTM does not define'
                               % (name, fname, scene))
            p += size
    return refs, bad


def _declared(path, const):
    text = open(path).read()
    body = text[text.index(const):]
    body = body[body.index('['):body.index('];')]
    return set(int(v, 16) for v in re.findall(r'0x([0-9A-Fa-f]{4})', body))


_ttm_used, _ads_used = _census()
for _path, _found, _label in [
        ('johnny/src/ttm.rs', _ttm_used, 'TTM'),
        ('johnny/src/ads.rs', _ads_used, 'ADS')]:
    try:
        _said = _declared(_path, 'pub const USED_IN_DATA')
    except Exception as _e:
        failures.append('%s census: could not read %s (%s)' % (_label, _path, _e))
        continue
    if _said != _found:
        failures.append(
            '%s census: %s declares %s the archive does not use, and misses %s'
            % (_label, _path,
               sorted('%04X' % v for v in _said - _found) or 'nothing',
               sorted('%04X' % v for v in _found - _said) or 'nothing'))
census_note = ('%d TTM and %d ADS opcodes, matching what the port declares'
               % (len(_ttm_used), len(_ads_used)))

# `tables.rs` is generated. This checks the file in the repository is still
# the file the generator would write -- a hand edit, a stale regeneration, a
# field quietly reordered. Everything the scheduler runs on is in there: the
# eighty scene rows, the route matrix, the thirty-four walk paths and the
# standing poses.

def _generated_tables():
    import subprocess
    out = subprocess.run([sys.executable, 'tools/gen_tables.py'],
                         capture_output=True, text=True)
    if out.returncode != 0:
        return None, out.stderr.strip().splitlines()[-1:] or ['gen_tables.py failed']
    return out.stdout, None


_fresh, _why = _generated_tables()
if _fresh is None:
    failures.append('tables: could not regenerate (%s)' % '; '.join(_why))
    tables_note = 'not checked'
else:
    _have = open('johnny/src/tables.rs').read()
    if _fresh != _have:
        a, b = _fresh.splitlines(), _have.splitlines()
        where = next((n for n, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
        failures.append('tables: johnny/src/tables.rs is not what gen_tables.py '
                        'writes; first difference at line %d' % (where + 1))
        tables_note = 'STALE'
    else:
        tables_note = '%d lines, byte for byte what gen_tables.py writes' % len(
            _fresh.splitlines())

_headers, _bad_headers = _headers()
if _bad_headers:
    failures.extend('sequences: ' + line for line in _bad_headers[:10])
    headers_note = '%d headers, %d of them wrong' % (_headers, len(_bad_headers))
else:
    headers_note = '%d sequence headers, every one after an FFFF' % _headers

_pals, _pal_files, _bad_pals = _palettes()
if _bad_pals:
    failures.extend('palettes: ' + line for line in _bad_pals[:5])
    pal_note = 'not the one palette this port assumes'
else:
    pal_note = ('%s, and each of the %d scripts loads it once and nothing else'
                % (_pals[0], _pal_files))

_markers, _bad_frames, _headless = _frames()
if _bad_frames:
    failures.extend('frames: ' + line for line in _bad_frames[:10])
    frames_note = '%d scene markers, %d misplaced' % (_markers, len(_bad_frames))
else:
    frames_note = ('%d scene markers, every one starts its frame (%s '
                   'has no unnamed header frame)'
                   % (_markers, ', '.join(_headless) if _headless else 'none'))

_refs, _bad_refs = _references()
if _bad_refs:
    failures.extend('references: ' + line for line in _bad_refs[:10])
    refs_note = '%d scene references, %d of them dangling' % (_refs, len(_bad_refs))
else:
    refs_note = '%d scene references, every one resolves' % _refs

print('archive      %s, %d records, %d bytes, fully consumed'
      % (archive, len(records), len(data)))
print('compressed   %d chunks' % chunks)
print('sprites      %d frames' % frames)
print('TTM scripts  %d/%d disassemble end to end' % (ttm_ok, ttm_total))
print('ADS scripts  %d/%d consume exactly' % (ads_ok, ads_total))
print('scheduler    %s' % sched_note)
print('census       %s' % census_note)
print('tables       %s' % tables_note)
print('references   %s' % refs_note)
print('frames       %s' % frames_note)
print('sequences    %s' % headers_note)
print('palette      %s' % pal_note)

if failures:
    print('\nFAILURES (%d):' % len(failures))
    for f in failures[:40]:
        print('  ' + f)
    sys.exit(1)
print('\nall good')
