"""Reader for the Dynamix DGDS resource format used by Johnny Castaway.

Covers RESOURCE.MAP / RESOURCE.001 as shipped with the 1992 Sierra/Dynamix
screensaver "Johnny Castaway" (internally SCRANTIC / "Screen Antics").

Everything here was derived from the shipped data plus a disassembly of the
original 16-bit SCRANTIC.EXE; see docs/FILE_FORMATS.md for the write-up.
"""

import struct
import zlib

# ---------------------------------------------------------------- containers

def read_archive(data):
    """Walk RESOURCE.001. Each record is [name:13][size:uint32][body:size]."""
    off = 0
    files = []
    while off + 17 <= len(data):
        name = data[off:off + 13].split(b'\0')[0].decode('latin1')
        size = struct.unpack_from('<I', data, off + 13)[0]
        if not name or off + 17 + size > len(data):
            break
        files.append((name, data[off + 17:off + 17 + size], off))
        off += 17 + size
    return files


def read_map(data):
    """RESOURCE.MAP: [magic:6][archive name:13][count:uint16][(hash,offset)*]."""
    archive = data[6:19].split(b'\0')[0].decode('latin1')
    count = struct.unpack_from('<H', data, 19)[0]
    entries = [struct.unpack_from('<II', data, 21 + i * 8) for i in range(count)]
    return archive, entries


def parse_chunks(body):
    """Chunk tree: [tag:4][size:uint32][payload].

    The top bit of the size field marks a container whose payload is itself a
    chunk list; otherwise the payload is a leaf.
    """
    tree = []
    p = 0
    while p + 8 <= len(body):
        tag = body[p:p + 4]
        if not all(32 <= c < 127 for c in tag):
            break
        raw = struct.unpack_from('<I', body, p + 4)[0]
        container = bool(raw & 0x80000000)
        size = raw & 0x7FFFFFFF
        payload = body[p + 8:p + 8 + size]
        tree.append((tag.decode('latin1'), payload, container,
                     parse_chunks(payload) if container else None))
        p += 8 + size
    return tree


def flatten(tree, path=()):
    """Yield (path_tuple, payload) for every leaf chunk."""
    for tag, payload, container, sub in tree:
        if container:
            yield from flatten(sub, path + (tag,))
        else:
            yield path + (tag,), payload


# Leaf chunks that carry a [type:uint8][uncompressed size:uint32] header.
# VGA: is context-dependent: inside a .BMP it is compressed, inside a .PAL the
# payload is the raw palette. See is_compressed().
COMPRESSED_TAGS = {'BIN:', 'SCR:'}


def is_compressed(record_name, tag):
    """Does this leaf carry a compression header, given the record it is in?"""
    if tag in COMPRESSED_TAGS:
        return True
    if tag == 'VGA:':
        return record_name.upper().endswith('.BMP')
    return False

# ------------------------------------------------------------- decompression

def lzw_decompress(data, usize):
    """Port of the LZW decoder at seg6:0x0D63 / 0x0FC1 in SCRANTIC.EXE.

    Unix-compress lineage: LSB-first codes of 9..12 bits, 256 is CLEAR, and the
    input is consumed in blocks of `codesize` bytes (eight codes per block). A
    code-width increase or a CLEAR discards the rest of the current block and
    starts a fresh one -- miss that and large files silently decode short.
    """
    prefix = [0] * 4096
    suffix = list(range(256)) + [0] * (4096 - 256)
    nextcode = 257
    reset_pending = False
    codesize = 9
    maxcode = (1 << 9) - 1
    buf = b''
    bitoff = 0
    availbits = 0
    src = 0
    out = bytearray()

    def getcode():
        nonlocal codesize, maxcode, reset_pending, buf, bitoff, availbits, src
        if reset_pending or bitoff >= availbits or nextcode > maxcode:
            if nextcode > maxcode:
                codesize += 1
                # note: at 12 bits the original stores 0x1000, not 0x0FFF,
                # which is what freezes the table instead of widening again.
                maxcode = 0x1000 if codesize == 12 else (1 << codesize) - 1
            if reset_pending:
                codesize = 9
                maxcode = (1 << 9) - 1
                reset_pending = False
            n = min(codesize, len(data) - src)
            if n <= 0:
                return -1
            buf = data[src:src + n]
            src += n
            bitoff = 0
            availbits = n * 8 - (codesize - 1)
        code = 0
        for k in range(codesize):
            bi = bitoff + k
            if (bi >> 3) < len(buf):
                code |= ((buf[bi >> 3] >> (bi & 7)) & 1) << k
        bitoff += codesize
        return code

    code = getcode()
    if code < 0:
        return b''
    oldcode = code
    finchar = code
    out.append(finchar & 0xFF)

    while len(out) < usize:
        code = getcode()
        if code < 0:
            break
        if code == 0x100:                       # CLEAR
            for i in range(256):
                prefix[i] = 0
            reset_pending = True
            nextcode = 0x100                    # yes, 256 -- not 257
            code = getcode()
            if code < 0:
                break
        incode = code
        stack = []
        if code >= nextcode:                    # KwKwK
            stack.append(finchar)
            code = oldcode
        while code >= 256:
            stack.append(suffix[code])
            code = prefix[code]
        finchar = suffix[code]
        stack.append(finchar)
        out.extend(bytes(reversed(stack)))
        if nextcode < 0x1000:
            prefix[nextcode] = oldcode
            suffix[nextcode] = finchar
            nextcode += 1
        oldcode = incode
    return bytes(out[:usize])


def rle_decompress(data, usize):
    out = bytearray()
    p = 0
    while p < len(data) and len(out) < usize:
        n = data[p]
        p += 1
        if n & 0x80:
            n &= 0x7F
            out += data[p:p + n]
            p += n
        else:
            if p >= len(data):
                break
            out += bytes([data[p]]) * n
            p += 1
    return bytes(out[:usize])


def decompress(payload):
    """Decode a leaf whose payload is [type:uint8][uncompressed size:uint32]."""
    ctype = payload[0]
    usize = struct.unpack_from('<I', payload, 1)[0]
    body = payload[5:]
    if ctype == 0:
        return body[:usize]
    if ctype == 1:
        return rle_decompress(body, usize)
    if ctype == 2:
        return lzw_decompress(body, usize)
    raise ValueError('unknown compression type %d' % ctype)


# -------------------------------------------------------------------- images

def unpack4(raw, w, h):
    """4bpp, two pixels per byte, high nibble first, rows padded to a byte."""
    stride = (w + 1) // 2
    px = bytearray(w * h)
    for y in range(h):
        ro = y * stride
        row = y * w
        for x in range(w):
            b = raw[ro + (x >> 1)]
            px[row + x] = (b >> 4) if (x & 1) == 0 else (b & 0x0F)
    return bytes(px)


def read_palette(pal_body):
    """JOHNCAST.PAL -> 256 RGB triples. Stored as 6-bit VGA values."""
    for path, payload in flatten(parse_chunks(pal_body)):
        if path[-1] == 'VGA:':
            return [(min(255, payload[i * 3] * 255 // 63),
                     min(255, payload[i * 3 + 1] * 255 // 63),
                     min(255, payload[i * 3 + 2] * 255 // 63))
                    for i in range(len(payload) // 3)]
    raise ValueError('no VGA: chunk in palette')


def read_bitmap(body):
    """A .BMP record -> list of (w, h, indices). Index 0 is transparent."""
    chunks = dict((path[-1], payload) for path, payload in flatten(parse_chunks(body)))
    inf = chunks['INF:']
    n = struct.unpack_from('<H', inf, 0)[0]
    widths = [struct.unpack_from('<H', inf, 2 + i * 2)[0] for i in range(n)]
    heights = [struct.unpack_from('<H', inf, 2 + n * 2 + i * 2)[0] for i in range(n)]
    raw = decompress(chunks['BIN:'])
    frames = []
    off = 0
    for w, h in zip(widths, heights):
        size = ((w + 1) // 2) * h
        frames.append((w, h, unpack4(raw[off:off + size], w, h)))
        off += size
    return frames


def read_screen(body):
    """A .SCR record -> (w, h, indices). Full-screen background, 640x350."""
    chunks = dict((path[-1], payload) for path, payload in flatten(parse_chunks(body)))
    w, h = struct.unpack('<HH', chunks['DIM:'][:4])
    return w, h, unpack4(decompress(chunks['BIN:']), w, h)


# --------------------------------------------------------------- script data

def parse_id_table(payload):
    """TAG:/RES: chunks -- [count:uint16] then count * [id:uint16][cstring]."""
    n = struct.unpack_from('<H', payload, 0)[0]
    p = 2
    out = {}
    for _ in range(n):
        if p + 2 > len(payload):
            break
        idx = struct.unpack_from('<H', payload, p)[0]
        p += 2
        end = payload.index(b'\0', p)
        out[idx] = payload[p:end].decode('latin1')
        p = end + 1
    return out


def disasm(code):
    """TTM TT3: / ADS SCR: bytecode -> [(offset, opcode, args, string)].

    Opcodes are 16-bit; the low nibble is the operand count, 0x0F meaning a
    single NUL-terminated string padded to a word boundary.

    INCOMPLETE. This decodes the opening of every script correctly -- resource
    loads, sprite draws, end-of-frame -- but loses sync partway into most of
    them, so some opcodes must carry an operand count that is not the low
    nibble. Decoding stops at the first inconsistency rather than emitting
    garbage; the caller gets however much was understood. See docs/ANALYSIS.md.
    """
    ops = []
    p = 0
    while p + 2 <= len(code):
        w = struct.unpack_from('<H', code, p)[0]
        count = w & 0x0F
        op = w & 0xFFF0
        if count == 0x0F:
            end = code.find(b'\0', p + 2)
            if end < 0:
                break
            ops.append((p, op, [], code[p + 2:end].decode('latin1')))
            p = end + 1
            if p & 1:
                p += 1
        else:
            if p + 2 + count * 2 > len(code):
                break
            args = [struct.unpack_from('<h', code, p + 2 + i * 2)[0]
                    for i in range(count)]
            ops.append((p, op, args, None))
            p += 2 + count * 2
    return ops


# ----------------------------------------------------------------- png output

def write_png(path, w, h, indices, palette, transparent=None):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += indices[y * w:(y + 1) * w]

    def chunk(tag, payload):
        body = tag + payload
        return (struct.pack('>I', len(payload)) + body
                + struct.pack('>I', zlib.crc32(body) & 0xFFFFFFFF))

    png = b'\x89PNG\r\n\x1a\n'
    png += chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 3, 0, 0, 0))
    png += chunk(b'PLTE', b''.join(bytes(c) for c in palette))
    if transparent is not None:
        alpha = bytearray([255] * len(palette))
        alpha[transparent] = 0
        png += chunk(b'tRNS', bytes(alpha))
    png += chunk(b'IDAT', zlib.compress(bytes(raw), 9))
    png += chunk(b'IEND', b'')
    with open(path, 'wb') as fh:
        fh.write(png)
