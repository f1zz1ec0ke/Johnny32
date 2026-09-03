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
        if p + 8 + size > len(body):
            # Not a chunk. Slicing past the end would silently invent a
            # payload -- FILES.VIN is plain text whose first four bytes happen
            # to look like a tag, and that is exactly how it used to sneak
            # through as an `intr` chunk.
            break
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
# payload is the raw palette. See is_compressed(). TT3: is RLE-compressed --
# easy to miss, because RLE literal runs leave most of the bytecode readable.
COMPRESSED_TAGS = {'BIN:', 'SCR:', 'TT3:'}


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
    """Bit 7 set means repeat the next byte (n & 0x7F) times; clear means a
    literal run of n bytes. Getting this backwards still decodes small streams
    almost right, which is a good way to lose a day."""
    out = bytearray()
    p = 0
    while p < len(data) and len(out) < usize:
        n = data[p]
        p += 1
        if n & 0x80:
            if p >= len(data):
                break
            out += bytes([data[p]]) * (n & 0x7F)
            p += 1
        else:
            out += data[p:p + n]
            p += n
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


# TTM opcodes, read out of the interpreter in SCRANTIC.EXE segment 12. The
# interpreter looks each one up in a table and jumps to a parallel handler
# table; anything not found goes to an ignore path.
#   seg12:0x12BD  47 general opcodes   (handlers at 0x131B)
#   seg12:0x03BC + 0x1900   the 0xAxxx drawing opcodes
#   seg12:0x00FC            the 0xCxxx opcodes
# 0x0FF0 is special-cased before the lookup and ends the frame; 0x1301/0x1311
# are rewritten to 0xC051/0xC061 on the way in.
TTM_GENERAL = [
    0x0010, 0x0020, 0x0070, 0x0080, 0x0090, 0x00C0, 0x00E0, 0x0110,
    0x0400, 0x0500, 0x0510, 0x1011, 0x1021, 0x1031, 0x1041, 0x1051,
    0x1061, 0x1071, 0x1101, 0x1111, 0x1121, 0x1201, 0x2002, 0x2012,
    0x2022, 0x2032, 0x2302, 0x2312, 0x2322, 0x2402, 0x3003, 0x3103,
    0x4004, 0x4114, 0x4124, 0x4136, 0x4204, 0x4214, 0x5005, 0xB000,
    0xB013, 0xB606, 0xF01F, 0xF02F, 0xF04F, 0xF05F, 0xF08F,
]
TTM_DRAW = [
    0xA002, 0xA014, 0xA024, 0xA034, 0xA044, 0xA054, 0xA064, 0xA094,
    0xA0A4, 0xA0B5, 0xA104, 0xA114, 0xA404, 0xA424, 0xA504, 0xA506,
    0xA514, 0xA516, 0xA524, 0xA526, 0xA534, 0xA536, 0xA5A7, 0xA601,
    0xA704, 0xAF02, 0xAF1F, 0xAF2F,
]
TTM_C = [
    0xC01F, 0xC02F, 0xC031, 0xC041, 0xC051, 0xC061, 0xC071, 0xC081,
    0xC091, 0xC0A1, 0xC0B2, 0xC0C2, 0xC0D3, 0xC0E3, 0xC0F4, 0xC102,
    0xCF01, 0xCF11,
]
TTM_OPCODES = set(TTM_GENERAL) | set(TTM_DRAW) | set(TTM_C) | {0x0FF0, 0x1301, 0x1311}

# ADS opcodes, from the table at SCRANTIC.EXE seg10:0x1875 (handlers at 0x18CD).
ADS_OPCODES = [
    0xF000, 0xF010, 0xF200, 0xF210, 0xFFFF, 0x0001, 0x0005, 0x1010,
    0x1020, 0x1030, 0x1040, 0x1050, 0x1060, 0x1070, 0x1080, 0x1310,
    0x1320, 0x1330, 0x1340, 0x1350, 0x1360, 0x1370, 0x1380, 0x1390,
    0x13A0, 0x13A1, 0x13B0, 0x13B1, 0x13C0, 0x13C1, 0x1420, 0x1430,
    0x1500, 0x1510, 0x1520, 0x2000, 0x2005, 0x2010, 0x2015, 0x2020,
    0x3010, 0x3020, 0x4000, 0x4010,
]

# ADS instruction lengths, from the length function at seg10:0x0303. It looks
# the opcode up in a 35-entry table at seg10:0x0383 (handlers at 0x03C9) whose
# handlers do nothing but add a constant to the script pointer. Anything not
# listed advances 2 -- which covers the 0x000N sequence labels, the block
# delimiters and the terminators. Every ADS script consumes exactly under this.
ADS_LENGTHS = {
    0xF000: 2, 0xF010: 4, 0xF200: 4, 0xF210: 4,
    0x1010: 6, 0x1020: 6, 0x1030: 6, 0x1040: 6, 0x1050: 6, 0x1060: 6,
    0x1070: 6, 0x1080: 4,
    0x1310: 6, 0x1320: 6, 0x1330: 6, 0x1340: 6, 0x1350: 6, 0x1360: 6,
    0x1370: 6, 0x1380: 4, 0x1390: 4,
    0x13A0: 4, 0x13A1: 4, 0x13B0: 4, 0x13B1: 4, 0x13C0: 4, 0x13C1: 4,
    0x2000: 10, 0x2005: 10, 0x2010: 8, 0x2015: 8, 0x2020: 8,
    0x3020: 4, 0x4000: 8, 0x4010: 8,
}

# Names for the script dumps. The `10x0` family waits at its guard and the
# `13x0` family is an ordinary rule; see docs/FILE_FORMATS.md.
ADS_NAMES = {
    0x2000: 'PLAY_SCENE',      # (ttm, scene, flags, weight)
    0x2005: 'PLAY_SCENE',      # same shape; weight is operand 4
    0x2010: 'STOP_SCENE',
    0x2015: 'FINISH_SCENE',
    0x4000: 'TO_FRONT',
    0x4010: 'TO_BACK',
    0x3010: 'RANDOM_BEGIN',    # weighted pick of one entry, up to RANDOM_END
    0x30FF: 'RANDOM_END',
    0x3020: 'RANDOM_SKIP',     # a weighted "play nothing" choice
    0x1500: 'SKIP_STEP',
    0x1510: 'STEP',
    0x1520: 'STEP_END',
    0x1420: 'AND',
    0x1430: 'OR',
    0xF000: 'RUN_SEQUENCE',
    0xF200: 'RUN_SEQUENCE',
    0xF210: 'RUN_SEQUENCE',
    0xF010: 'STOP_SEQUENCE',
    0xFFFF: 'END_SEQUENCE',
    0x1010: 'WAIT_FINISHED',     0x1310: 'IF_FINISHED',
    0x1020: 'WAIT_NOT_FINISHED', 0x1320: 'IF_NOT_FINISHED',
    0x1030: 'WAIT_NOT_STARTED',  0x1330: 'IF_NOT_STARTED',
    0x1040: 'WAIT_STARTED',      0x1340: 'IF_STARTED',
    0x1050: 'WAIT_JUST_ENDED',   0x1350: 'IF_JUST_ENDED',
    0x1060: 'WAIT_NOT_QUEUED',   0x1360: 'IF_NOT_QUEUED',
    0x1070: 'WAIT_QUEUED',       0x1370: 'IF_QUEUED',
    0x1080: 'WAIT_COUNT_LE',     0x1380: 'IF_COUNT_LE',
    0x1390: 'IF_COUNT_GE',
    0x13A0: 'IF_VAR_LE', 0x13A1: 'IF_VAR_LE',
    0x13B0: 'IF_VAR_GE', 0x13B1: 'IF_VAR_GE',
    0x13C0: 'IF_VAR_EQ', 0x13C1: 'IF_VAR_EQ',
}


def disasm_ads(code):
    """ADS SCR: bytecode -> [(offset, opcode, operands)].

    ADS does not use the TTM rule -- its operand counts are not the low nibble.
    Each handler advances the script pointer itself, and the lengths come from
    ADS_LENGTHS. All ten ADS scripts consume exactly under this.
    """
    ops = []
    p = 0
    while p + 2 <= len(code):
        w = struct.unpack_from('<H', code, p)[0]
        n = ADS_LENGTHS.get(w, 2)
        if p + n > len(code):
            break
        args = [struct.unpack_from('<H', code, p + 2 + i * 2)[0]
                for i in range((n - 2) // 2)]
        ops.append((p, w, args))
        p += n
    return ops

# Names for the script dumps; johnny/src/ttm.rs is the reference.
TTM_NAMES = {
    0x0FF0: 'END_FRAME',
    0x0080: 'FREE_SHAPES',
    0x0110: 'END_SCENE',
    0x1021: 'SET_DELAY',
    0x2022: 'SET_RANDOM_DELAY',
    0x1051: 'SELECT_BMP_SLOT',
    0x1061: 'SELECT_PAL_SLOT',
    0x1101: 'SCENE_MARKER',   # names a frame, but gets no instance record
    0x1111: 'SCENE',
    0x1121: 'SET_BACKGROUND',
    0x1201: 'GOTO_SCENE',
    0x2002: 'SET_COLOURS',
    0x2012: 'FLIP_NEXT_LOAD', # no-op here: the port mirrors at draw time
    0x4004: 'SET_CLIP',
    0x4204: 'SAVE_REGION',    # no-op here: the backdrop is repainted every tick
    0x4214: 'NOP',            # seg12:0x0860 does nothing either way
    0xC051: 'PLAY_SOUND',
    0xA601: 'NOP',            # seg12:0x02D8 likewise
    0xA002: 'DRAW_PIXEL',
    0xA0A4: 'DRAW_LINE',
    0xA104: 'FILL_RECT',
    0xA404: 'FILL_CIRCLE',
    0xA504: 'DRAW_SPRITE',
    0xA524: 'DRAW_SPRITE_FLIPPED',
    0xA054: 'SAVE_RECT',
    0xA064: 'RESTORE_RECT',
    0xB606: 'COPY_RECT',      # (x, y, w, h, from page, to page)
    0xF01F: 'LOAD_SCREEN',
    0xF02F: 'LOAD_BITMAP',
    0xF05F: 'LOAD_PALETTE',
}


def disasm(code, opcodes=None):
    """TTM TT3: bytecode -> [(offset, opcode, args, string)].

    Opcodes are 16-bit; the low nibble is the operand count, 0x0F meaning one
    NUL-terminated string padded to a word boundary. The interpreter confirms
    this at seg12:0x0461, where it masks the fetched word with 0x0F and jumps
    through a table indexed by the result.

    Pass `opcodes` to reject anything outside a known set; decoding then stops
    at the first word that is not a valid opcode rather than emitting garbage.
    All 41 TTM scripts decode end to end with TTM_OPCODES.

    ADS does not use this rule; use disasm_ads.
    """
    ops = []
    p = 0
    while p + 2 <= len(code):
        w = struct.unpack_from('<H', code, p)[0]
        if opcodes is not None and w not in opcodes:
            break
        count = w & 0x0F
        if count == 0x0F:
            end = code.find(b'\0', p + 2)
            if end < 0:
                break
            ops.append((p, w, [], code[p + 2:end].decode('latin1')))
            p = end + 1
            if p & 1:
                p += 1
        else:
            if p + 2 + count * 2 > len(code):
                break
            args = [struct.unpack_from('<h', code, p + 2 + i * 2)[0]
                    for i in range(count)]
            ops.append((p, w, args, None))
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
