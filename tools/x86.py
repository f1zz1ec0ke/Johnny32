"""A 16-bit x86 interpreter, big enough to run SCRANTIC.EXE's own functions.

The point is to stop checking this port against another copy of my own
reasoning. `tools/difftest.sh` compares the Rust decoder against the Python
one, but both were written from the same reading of the same disassembly, so a
shared misreading passes clean. This runs the **original code** and compares
against that.

It is not a PC emulator. There is no BIOS, no interrupts, no video, no DOS. It
loads the NE segments, applies their relocations, gives them a stack and a
copy of the data segment, and executes. Far calls to imported functions
(`KERNEL`, `USER`, `GDI`, `MMSYSTEM`) hit a stub table; a call to something
with no stub raises, which is the right answer -- it means the function under
test reaches outside what can be modelled, and the result would be a guess.

Only the instructions these functions actually use are implemented. Anything
else raises `Unsupported` with the address and the bytes, so a gap announces
itself instead of quietly computing the wrong answer.

    from x86 import Machine
    m = Machine('original/scrantic.exe')
    m.call(seg=4, off=0x1E86)          # the engine's rand()
    print(m.ax)
"""

import struct

import ne_info


class Unsupported(Exception):
    pass


class Answer(Exception):
    """Raised by a hook that has seen what the caller wanted.

    Running a routine to its natural end often means running the parts below
    it as well -- the walk driver picks a route and then goes on to animate
    the turn. When the question is only "which route", a hook that raises
    this stops the machine at the moment the answer exists.
    """

    def __init__(self, value):
        super().__init__(value)
        self.value = value


class Trap(Exception):
    """A far call to something the harness has no stub for."""


def _s16(v):
    return v - 0x10000 if v & 0x8000 else v


def _s8(v):
    return v - 0x100 if v & 0x80 else v


class Machine:
    """One loaded executable, with its segments laid out in a flat address
    space at `segment_index << 16` so a (seg, off) pair is trivially a linear
    address. Real 16-bit paragraphs are not modelled: nothing in these
    functions does pointer arithmetic across a segment boundary.
    """

    # The stack lives *inside* the data segment, because this is a Win16
    # small-model program and its code assumes SS == DS. It matters: an
    # argument reached through `bp` and the same argument reached through `si`
    # have to be the same word, and they only are if the two segments are one.
    # Segment images here are well under 64K, so the top of the segment is
    # free.
    STACK_TOP = 0xF000
    RETURN = 0xDEAD  # a far return address the runner recognises

    def __init__(self, path):
        data = open(path, 'rb').read()
        self.info = ne_info.parse(data)
        self.mem = bytearray(0x60 << 16)
        self.segs = {}
        for seg in self.info['segments']:
            i = seg['index']
            body = data[seg['offset']:seg['offset'] + seg['length']]
            base = i << 16
            self.mem[base:base + len(body)] = body
            self.segs[i] = seg
            if seg['kind'] == 'DATA':
                self.ds = i
        self.imports = {}
        self._relocate(data)
        self.regs = dict(ax=0, bx=0, cx=0, dx=0, si=0, di=0, bp=0, sp=0)
        self.sregs = dict(cs=0, ds=self.ds, es=self.ds, ss=self.ds)
        self.flags = dict(zf=0, sf=0, cf=0, of=0)
        self.stubs = {}
        self.hooks = {}
        self.hooked = {}
        self.steps = 0
        self.limit = 5_000_000

    # ---- loading ---------------------------------------------------------

    def _relocate(self, data):
        """Turn each fixup into something the runner can act on.

        Internal fixups become the real (segment, offset) of the target.
        Imported ones become a synthetic far address in a reserved segment,
        which `call_far` looks up in the stub table -- except the handful of
        imports that are *values* rather than functions, which are written
        straight in.

        What each record patches matters: 1,266 of them are far addresses and
        four bytes wide, but seventy are a bare segment or a bare offset and
        two bytes wide. Writing four bytes for one of those overwrites the two
        bytes after it, which is a corrupted instruction sitting quietly in
        the middle of a function until something runs it.
        """
        self.import_at = {}
        for seg in self.info['segments']:
            body = data[seg['offset']:seg['offset'] + seg['length']]
            base = seg['index'] << 16
            for rec in ne_info.relocation_records(data, self.info, seg):
                target = self._resolve(rec['desc'])
                if target is None:
                    continue
                tseg, toff = target
                for off in ne_info.fixup_sites(body, rec):
                    at = base + off
                    if rec['addr'] == ne_info.ADDR_FAR:
                        struct.pack_into('<HH', self.mem, at, toff, tseg)
                    elif rec['addr'] == ne_info.ADDR_SEGMENT:
                        self._patch16(at, tseg, rec['additive'])
                    elif rec['addr'] == ne_info.ADDR_OFFSET:
                        self._patch16(at, toff, rec['additive'])
                    elif rec['addr'] == ne_info.ADDR_LOBYTE:
                        self.mem[at] = toff & 0xFF

    def _patch16(self, at, value, additive):
        if additive:
            value += struct.unpack_from('<H', self.mem, at)[0]
        struct.pack_into('<H', self.mem, at, value & 0xFFFF)

    # Imports that are constants, not entry points. Win16 exports these so a
    # program can walk a "huge" allocation across segment boundaries; nothing
    # here allocates one that big, so the arithmetic degenerates to the plain
    # kind either way.
    VALUE_IMPORTS = {'KERNEL.113': 3, 'KERNEL.114': 8}

    def _resolve(self, note):
        if note.startswith('seg'):
            seg, off = note.split(':')
            return int(seg[3:]), int(off, 16)
        if note in self.VALUE_IMPORTS:
            return 0, self.VALUE_IMPORTS[note]
        # An import: give it an address in the trap segment, keyed by name.
        idx = self.imports.setdefault(note, len(self.imports))
        return 0x50, idx

    # ---- memory ----------------------------------------------------------

    def _lin(self, seg, off):
        return (seg << 16) + (off & 0xFFFF)

    def read16(self, seg, off):
        return struct.unpack_from('<H', self.mem, self._lin(seg, off))[0]

    def read8(self, seg, off):
        return self.mem[self._lin(seg, off)]

    def write16(self, seg, off, v):
        struct.pack_into('<H', self.mem, self._lin(seg, off), v & 0xFFFF)

    def write8(self, seg, off, v):
        self.mem[self._lin(seg, off)] = v & 0xFF

    # ---- registers -------------------------------------------------------

    W = ['ax', 'cx', 'dx', 'bx', 'sp', 'bp', 'si', 'di']
    B = ['al', 'cl', 'dl', 'bl', 'ah', 'ch', 'dh', 'bh']
    S = ['es', 'cs', 'ss', 'ds']

    def get_w(self, i):
        return self.regs[self.W[i]] if self.W[i] != 'sp' else self.regs['sp']

    def set_w(self, i, v):
        self.regs[self.W[i]] = v & 0xFFFF

    def get_b(self, i):
        name = self.B[i]
        full = self.regs[name[0] + 'x']
        return full & 0xFF if name[1] == 'l' else (full >> 8) & 0xFF

    def set_b(self, i, v):
        name = self.B[i]
        key = name[0] + 'x'
        full = self.regs[key]
        if name[1] == 'l':
            self.regs[key] = (full & 0xFF00) | (v & 0xFF)
        else:
            self.regs[key] = (full & 0x00FF) | ((v & 0xFF) << 8)

    def __getattr__(self, k):
        if k in ('ax', 'bx', 'cx', 'dx', 'si', 'di', 'bp', 'sp'):
            return self.regs[k]
        if k in ('cs', 'ds', 'es', 'ss'):
            return self.sregs[k]
        raise AttributeError(k)

    # ---- stack -----------------------------------------------------------

    def push(self, v):
        self.regs['sp'] = (self.regs['sp'] - 2) & 0xFFFF
        self.write16(self.sregs['ss'], self.regs['sp'], v)

    def pop(self):
        v = self.read16(self.sregs['ss'], self.regs['sp'])
        self.regs['sp'] = (self.regs['sp'] + 2) & 0xFFFF
        return v

    # ---- running ---------------------------------------------------------

    def call(self, seg, off, args=()):
        """Run one far function with `args` pushed the way a Pascal/cdecl
        caller would (last argument deepest), and return `ax`.
        """
        self.regs['sp'] = self.STACK_TOP
        self.sregs['ss'] = self.ds
        for a in reversed(args):
            self.push(a & 0xFFFF)
        self.push(0)             # a return segment the runner recognises
        self.push(self.RETURN)
        self.sregs['cs'] = seg
        self.ip = off
        try:
            self.run()
        except Answer as a:
            return a.value
        return self.regs['ax']

    def run(self):
        while True:
            if self.sregs['cs'] == 0 and self.ip == self.RETURN:
                return
            self.steps += 1
            if self.steps > self.limit:
                raise Unsupported('ran away at %04x:%04x' % (self.sregs['cs'], self.ip))
            self.step()

    def fetch8(self):
        v = self.read8(self.sregs['cs'], self.ip)
        self.ip = (self.ip + 1) & 0xFFFF
        return v

    def fetch16(self):
        v = self.read16(self.sregs['cs'], self.ip)
        self.ip = (self.ip + 2) & 0xFFFF
        return v

    # ---- addressing ------------------------------------------------------

    def modrm(self, seg_override=None):
        """Decode a ModRM byte into (mod, reg, rm_descriptor)."""
        b = self.fetch8()
        mod, reg, rm = b >> 6, (b >> 3) & 7, b & 7
        if mod == 3:
            return mod, reg, ('r', rm)
        seg = seg_override if seg_override is not None else self.sregs['ds']
        if mod == 0 and rm == 6:
            return mod, reg, ('m', seg, self.fetch16())
        bases = [
            self.bx + self.si, self.bx + self.di,
            self.bp + self.si, self.bp + self.di,
            self.si, self.di, self.bp, self.bx,
        ]
        addr = bases[rm]
        if rm in (2, 3, 6) and seg_override is None:
            seg = self.sregs['ss']
        if mod == 1:
            addr += _s8(self.fetch8())
        elif mod == 2:
            addr += _s16(self.fetch16())
        return mod, reg, ('m', seg, addr & 0xFFFF)

    def load(self, rm, width=16):
        if rm[0] == 'r':
            return self.get_w(rm[1]) if width == 16 else self.get_b(rm[1])
        return self.read16(rm[1], rm[2]) if width == 16 else self.read8(rm[1], rm[2])

    def store(self, rm, v, width=16):
        if rm[0] == 'r':
            (self.set_w if width == 16 else self.set_b)(rm[1], v)
        else:
            (self.write16 if width == 16 else self.write8)(rm[1], rm[2], v)

    # ---- flags -----------------------------------------------------------

    def set_flags(self, res, width=16, cf=None, of=None):
        mask = 0xFFFF if width == 16 else 0xFF
        top = 0x8000 if width == 16 else 0x80
        self.flags['zf'] = 1 if (res & mask) == 0 else 0
        self.flags['sf'] = 1 if (res & top) else 0
        if cf is not None:
            self.flags['cf'] = cf
        if of is not None:
            self.flags['of'] = of

    def sub_flags(self, a, b, width=16):
        mask = 0xFFFF if width == 16 else 0xFF
        res = (a - b) & mask
        top = 0x8000 if width == 16 else 0x80
        self.set_flags(res, width, cf=1 if (a & mask) < (b & mask) else 0,
                       of=1 if ((a ^ b) & (a ^ res) & top) else 0)
        return res

    def add_flags(self, a, b, width=16):
        mask = 0xFFFF if width == 16 else 0xFF
        res = (a + b) & mask
        top = 0x8000 if width == 16 else 0x80
        self.set_flags(res, width, cf=1 if (a + b) > mask else 0,
                       of=1 if (~(a ^ b) & (a ^ res) & top) else 0)
        return res

    def cond(self, code):
        f = self.flags
        return [
            f['of'], not f['of'],
            f['cf'], not f['cf'],
            f['zf'], not f['zf'],
            f['cf'] or f['zf'], not (f['cf'] or f['zf']),
            f['sf'], not f['sf'],
            0, 1,                                   # parity: unused here
            f['sf'] != f['of'], f['sf'] == f['of'],
            f['zf'] or (f['sf'] != f['of']),
            (not f['zf']) and (f['sf'] == f['of']),
        ][code]

    # ---- far calls -------------------------------------------------------

    def hook(self, seg, off, fn=None):
        """Answer a call to one of the program's own functions here instead.

        The point is to run a routine that sits on top of a subsystem this
        interpreter has no business in -- the blitter, the pager, anything
        that ends in GDI -- without dragging the subsystem in. `fn` is called
        with the machine and should leave a result in `ax`; the default
        returns zero. Every hook is counted, so a run can report what it
        stood in for rather than quietly pretending.
        """
        self.hooks[(seg, off)] = fn or (lambda m: m.regs.__setitem__('ax', 0))

    def call_far(self, seg, off):
        hooked = self.hooks.get((seg, off))
        if hooked is not None:
            self.hooked[(seg, off)] = self.hooked.get((seg, off), 0) + 1
            hooked(self)
            return
        if seg == 0x50:
            name = [k for k, v in self.imports.items() if v == off]
            name = name[0] if name else 'import#%d' % off
            stub = self.stubs.get(name)
            if stub is None:
                raise Trap('no stub for %s' % name)
            stub(self)
            return
        self.push(self.sregs['cs'])
        self.push(self.ip)
        self.sregs['cs'] = seg
        self.ip = off

    # ---- one instruction -------------------------------------------------

    def step(self):
        start = self.ip
        seg_override = None
        op = self.fetch8()
        while op in (0x26, 0x2E, 0x36, 0x3E):
            seg_override = self.sregs[{0x26: 'es', 0x2E: 'cs', 0x36: 'ss', 0x3E: 'ds'}[op]]
            op = self.fetch8()

        # --- mov ---------------------------------------------------------
        if op in (0x88, 0x89, 0x8A, 0x8B):
            w = 16 if op & 1 else 8
            _, reg, rm = self.modrm(seg_override)
            if op & 2:
                v = self.load(rm, w)
                (self.set_w if w == 16 else self.set_b)(reg, v)
            else:
                self.store(rm, self.get_w(reg) if w == 16 else self.get_b(reg), w)
        elif op == 0x8C:                                    # mov rm, sreg
            _, reg, rm = self.modrm(seg_override)
            self.store(rm, self.sregs[self.S[reg]])
        elif op == 0x8E:                                    # mov sreg, rm
            _, reg, rm = self.modrm(seg_override)
            self.sregs[self.S[reg]] = self.load(rm)
        elif 0xB8 <= op <= 0xBF:
            self.set_w(op - 0xB8, self.fetch16())
        elif 0xB0 <= op <= 0xB7:
            self.set_b(op - 0xB0, self.fetch8())
        elif op in (0xC6, 0xC7):
            w = 16 if op & 1 else 8
            _, _, rm = self.modrm(seg_override)
            self.store(rm, self.fetch16() if w == 16 else self.fetch8(), w)
        elif op == 0xA0:                                    # mov al, [imm]
            self.set_b(0, self.read8(seg_override or self.sregs['ds'], self.fetch16()))
        elif op == 0xA1:
            self.set_w(0, self.read16(seg_override or self.sregs['ds'], self.fetch16()))
        elif op == 0xA2:
            self.write8(seg_override or self.sregs['ds'], self.fetch16(), self.get_b(0))
        elif op == 0xA3:
            self.write16(seg_override or self.sregs['ds'], self.fetch16(), self.ax)
        elif op == 0x8D:                                    # lea
            _, reg, rm = self.modrm(seg_override)
            self.set_w(reg, rm[2])
        elif op == 0xC4:                                    # les
            _, reg, rm = self.modrm(seg_override)
            self.set_w(reg, self.read16(rm[1], rm[2]))
            self.sregs['es'] = self.read16(rm[1], rm[2] + 2)
        elif op == 0xC5:                                    # lds
            _, reg, rm = self.modrm(seg_override)
            self.set_w(reg, self.read16(rm[1], rm[2]))
            self.sregs['ds'] = self.read16(rm[1], rm[2] + 2)

        # --- stack -------------------------------------------------------
        elif 0x50 <= op <= 0x57:
            self.push(self.get_w(op - 0x50))
        elif 0x58 <= op <= 0x5F:
            self.set_w(op - 0x58, self.pop())
        elif op == 0x06:
            self.push(self.sregs['es'])
        elif op == 0x07:
            self.sregs['es'] = self.pop()
        elif op == 0x0E:
            self.push(self.sregs['cs'])
        elif op == 0x1E:
            self.push(self.sregs['ds'])
        elif op == 0x1F:
            self.sregs['ds'] = self.pop()
        elif op == 0x68:
            self.push(self.fetch16())
        elif op == 0x6A:
            self.push(_s8(self.fetch8()) & 0xFFFF)

        # --- arithmetic on rm/reg ----------------------------------------
        elif op in (0x00, 0x01, 0x02, 0x03, 0x28, 0x29, 0x2A, 0x2B,
                    0x30, 0x31, 0x32, 0x33, 0x20, 0x21, 0x22, 0x23,
                    0x08, 0x09, 0x0A, 0x0B, 0x38, 0x39, 0x3A, 0x3B,
                    0x84, 0x85, 0x10, 0x11, 0x12, 0x13, 0x18, 0x19):
            w = 16 if op & 1 else 8
            _, reg, rm = self.modrm(seg_override)
            a = self.load(rm, w)
            b = self.get_w(reg) if w == 16 else self.get_b(reg)
            if op & 2 and op not in (0x84, 0x85):
                a, b = b, a
            kind = op & 0xF8
            if op in (0x84, 0x85):
                self.set_flags(a & b, w, cf=0, of=0)
                return
            if kind == 0x00 or kind == 0x10:
                carry = self.flags['cf'] if kind == 0x10 else 0
                res = self.add_flags(a, b + carry, w)
            elif kind in (0x28, 0x38, 0x18):
                res = self.sub_flags(a, b, w)
            elif kind == 0x30:
                res = a ^ b
                self.set_flags(res, w, cf=0, of=0)
            elif kind == 0x20:
                res = a & b
                self.set_flags(res, w, cf=0, of=0)
            else:                                            # 0x08 or
                res = a | b
                self.set_flags(res, w, cf=0, of=0)
            if kind == 0x38:                                 # cmp writes nothing
                return
            if op & 2:
                (self.set_w if w == 16 else self.set_b)(reg, res)
            else:
                self.store(rm, res, w)

        # --- arithmetic with an immediate --------------------------------
        elif op in (0x80, 0x81, 0x83):
            w = 16 if op != 0x80 else 8
            _, kind, rm = self.modrm(seg_override)
            a = self.load(rm, w)
            if op == 0x81:
                b = self.fetch16()
            elif op == 0x83:
                b = _s16(_s8(self.fetch8()) & 0xFFFF) & 0xFFFF
            else:
                b = self.fetch8()
            if kind == 0:
                res = self.add_flags(a, b, w)
            elif kind == 5 or kind == 7:
                res = self.sub_flags(a, b, w)
            elif kind == 1:
                res = a | b
                self.set_flags(res, w, cf=0, of=0)
            elif kind == 4:
                res = a & b
                self.set_flags(res, w, cf=0, of=0)
            elif kind == 6:
                res = a ^ b
                self.set_flags(res, w, cf=0, of=0)
            else:
                raise Unsupported('grp1/%d at %04x' % (kind, start))
            if kind != 7:
                self.store(rm, res, w)
        elif op in (0xA8, 0xA9):
            w = 16 if op & 1 else 8
            b = self.fetch16() if w == 16 else self.fetch8()
            a = self.ax if w == 16 else self.get_b(0)
            self.set_flags(a & b, w, cf=0, of=0)
        elif op in (0x04, 0x05, 0x2C, 0x2D, 0x3C, 0x3D, 0x24, 0x25,
                    0x0C, 0x0D, 0x34, 0x35):
            w = 16 if op & 1 else 8
            b = self.fetch16() if w == 16 else self.fetch8()
            a = self.ax if w == 16 else self.get_b(0)
            kind = op & 0xF8
            if kind == 0x00:
                res = self.add_flags(a, b, w)
            elif kind in (0x28, 0x38):
                res = self.sub_flags(a, b, w)
            elif kind == 0x20:
                res = a & b
                self.set_flags(res, w, cf=0, of=0)
            elif kind == 0x08:
                res = a | b
                self.set_flags(res, w, cf=0, of=0)
            else:
                res = a ^ b
                self.set_flags(res, w, cf=0, of=0)
            if kind != 0x38:
                (self.set_w if w == 16 else self.set_b)(0, res)

        # --- inc / dec ---------------------------------------------------
        elif 0x40 <= op <= 0x47:
            i = op - 0x40
            cf = self.flags['cf']
            self.set_w(i, self.add_flags(self.get_w(i), 1))
            self.flags['cf'] = cf
        elif 0x48 <= op <= 0x4F:
            i = op - 0x48
            cf = self.flags['cf']
            self.set_w(i, self.sub_flags(self.get_w(i), 1))
            self.flags['cf'] = cf
        elif op in (0xFE, 0xFF):
            w = 16 if op == 0xFF else 8
            _, kind, rm = self.modrm(seg_override)
            if kind == 0:
                cf = self.flags['cf']
                self.store(rm, self.add_flags(self.load(rm, w), 1, w), w)
                self.flags['cf'] = cf
            elif kind == 1:
                cf = self.flags['cf']
                self.store(rm, self.sub_flags(self.load(rm, w), 1, w), w)
                self.flags['cf'] = cf
            elif kind == 6:
                self.push(self.load(rm, w))
            elif kind == 2:                                  # call near rm
                target = self.load(rm, w)
                self.push(self.ip)
                self.ip = target
            elif kind == 3:                                  # call far rm
                off = self.read16(rm[1], rm[2])
                seg = self.read16(rm[1], rm[2] + 2)
                self.call_far(seg, off)
            elif kind == 4:                                  # jmp near rm
                self.ip = self.load(rm, w)
            elif kind == 5:                                  # jmp far rm
                off = self.read16(rm[1], rm[2])
                self.sregs['cs'] = self.read16(rm[1], rm[2] + 2)
                self.ip = off
            else:
                raise Unsupported('grp5/%d at %04x' % (kind, start))

        # --- shifts ------------------------------------------------------
        elif op in (0xC0, 0xC1, 0xD0, 0xD1, 0xD2, 0xD3):
            w = 16 if op & 1 else 8
            _, kind, rm = self.modrm(seg_override)
            if op in (0xC0, 0xC1):
                n = self.fetch8()
            elif op in (0xD0, 0xD1):
                n = 1
            else:
                n = self.get_b(1)
            v = self.load(rm, w)
            mask = 0xFFFF if w == 16 else 0xFF
            top = 0x8000 if w == 16 else 0x80
            for _ in range(n & 31):
                if kind == 4:                                # shl
                    self.flags['cf'] = 1 if v & top else 0
                    v = (v << 1) & mask
                elif kind == 5:                              # shr
                    self.flags['cf'] = v & 1
                    v >>= 1
                elif kind == 7:                              # sar
                    self.flags['cf'] = v & 1
                    v = ((v ^ top) - top) >> 1 & mask if False else \
                        ((_s16(v) if w == 16 else _s8(v)) >> 1) & mask
                else:
                    raise Unsupported('shift/%d at %04x' % (kind, start))
            if n & 31:
                self.set_flags(v, w)
            self.store(rm, v, w)

        # --- mul / div / neg / not ---------------------------------------
        elif op in (0xF6, 0xF7):
            w = 16 if op & 1 else 8
            _, kind, rm = self.modrm(seg_override)
            v = self.load(rm, w)
            if kind == 0:                                    # test imm
                b = self.fetch16() if w == 16 else self.fetch8()
                self.set_flags(v & b, w, cf=0, of=0)
            elif kind == 2:                                  # not
                self.store(rm, ~v, w)
            elif kind == 3:                                  # neg
                self.store(rm, self.sub_flags(0, v, w), w)
            elif kind == 4:                                  # mul
                if w == 16:
                    r = self.ax * v
                    self.regs['ax'] = r & 0xFFFF
                    self.regs['dx'] = (r >> 16) & 0xFFFF
                else:
                    self.regs['ax'] = (self.get_b(0) * v) & 0xFFFF
            elif kind == 5:                                  # imul
                if w == 16:
                    r = _s16(self.ax) * _s16(v)
                    self.regs['ax'] = r & 0xFFFF
                    self.regs['dx'] = (r >> 16) & 0xFFFF
                else:
                    self.regs['ax'] = (_s8(self.get_b(0)) * _s8(v)) & 0xFFFF
            elif kind == 6:                                  # div
                if v == 0:
                    raise Unsupported('divide by zero at %04x' % start)
                if w == 16:
                    n = (self.dx << 16) | self.ax
                    self.regs['ax'] = (n // v) & 0xFFFF
                    self.regs['dx'] = (n % v) & 0xFFFF
                else:
                    n = self.ax
                    self.set_b(0, n // v)
                    self.set_b(4, n % v)
            elif kind == 7:                                  # idiv
                if v == 0:
                    raise Unsupported('divide by zero at %04x' % start)
                n = _s16(self.dx) << 16 | self.ax if w == 16 else _s16(self.ax)
                if w == 16:
                    n = (self.dx << 16) | self.ax
                    if n & 0x80000000:
                        n -= 1 << 32
                    d = _s16(v)
                else:
                    n = _s16(self.ax)
                    d = _s8(v)
                q = int(n / d) if d else 0
                r = n - q * d
                if w == 16:
                    self.regs['ax'] = q & 0xFFFF
                    self.regs['dx'] = r & 0xFFFF
                else:
                    self.set_b(0, q)
                    self.set_b(4, r)
            else:
                raise Unsupported('grp3/%d at %04x' % (kind, start))

        # --- sign extension ----------------------------------------------
        elif op == 0x98:                                     # cbw / cwde
            self.regs['ax'] = _s8(self.get_b(0)) & 0xFFFF
        elif op == 0x99:                                     # cwd / cdq
            self.regs['dx'] = 0xFFFF if self.ax & 0x8000 else 0

        # --- control flow -------------------------------------------------
        elif op == 0xE8:                                     # call near
            rel = _s16(self.fetch16())
            self.push(self.ip)
            self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0xE9:
            rel = _s16(self.fetch16())
            self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0xEB:
            rel = _s8(self.fetch8())
            self.ip = (self.ip + rel) & 0xFFFF
        elif 0x70 <= op <= 0x7F:
            rel = _s8(self.fetch8())
            if self.cond(op - 0x70):
                self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0x0F:                                     # 0F 8x: long jcc
            op2 = self.fetch8()
            if 0x80 <= op2 <= 0x8F:
                rel = _s16(self.fetch16())
                if self.cond(op2 - 0x80):
                    self.ip = (self.ip + rel) & 0xFFFF
            else:
                raise Unsupported('0f %02x at %04x' % (op2, start))
        elif op == 0xE3:                                     # jcxz
            rel = _s8(self.fetch8())
            if self.cx == 0:
                self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0xE2:                                     # loop
            rel = _s8(self.fetch8())
            self.regs['cx'] = (self.cx - 1) & 0xFFFF
            if self.cx:
                self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0xE1:                                     # loope
            rel = _s8(self.fetch8())
            self.regs['cx'] = (self.cx - 1) & 0xFFFF
            if self.cx and self.flags['zf']:
                self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0xE0:                                     # loopne
            rel = _s8(self.fetch8())
            self.regs['cx'] = (self.cx - 1) & 0xFFFF
            if self.cx and not self.flags['zf']:
                self.ip = (self.ip + rel) & 0xFFFF
        elif op == 0x9A:                                     # call far imm
            off = self.fetch16()
            seg = self.fetch16()
            self.call_far(seg, off)
        elif op == 0xEA:                                     # jmp far imm
            off = self.fetch16()
            self.sregs['cs'] = self.fetch16()
            self.ip = off
        elif op == 0xC3:                                     # ret near
            self.ip = self.pop()
        elif op == 0xC2:
            n = self.fetch16()
            self.ip = self.pop()
            self.regs['sp'] = (self.regs['sp'] + n) & 0xFFFF
        elif op == 0xCB:                                     # retf
            self.ip = self.pop()
            self.sregs['cs'] = self.pop()
        elif op == 0xCA:
            n = self.fetch16()
            self.ip = self.pop()
            self.sregs['cs'] = self.pop()
            self.regs['sp'] = (self.regs['sp'] + n) & 0xFFFF
        elif op == 0x90:                                     # nop (xchg ax, ax)
            pass
        elif 0x91 <= op <= 0x97:                             # xchg ax, r
            i = op - 0x90
            a, b = self.ax, self.get_w(i)
            self.regs['ax'] = b
            self.set_w(i, a)
        elif op in (0x86, 0x87):                             # xchg rm, reg
            w = 16 if op & 1 else 8
            _, reg, rm = self.modrm(seg_override)
            a = self.load(rm, w)
            b = self.get_w(reg) if w == 16 else self.get_b(reg)
            self.store(rm, b, w)
            (self.set_w if w == 16 else self.set_b)(reg, a)
        elif op == 0xC9:                                     # leave
            self.regs['sp'] = self.regs['bp']
            self.regs['bp'] = self.pop()
        elif op == 0x9C:
            self.push(0)
        elif op == 0x9D:
            self.pop()
        elif op == 0xF8:
            self.flags['cf'] = 0
        elif op == 0xF9:
            self.flags['cf'] = 1
        elif op == 0xFA or op == 0xFB:                       # cli / sti
            pass
        else:
            raise Unsupported('opcode %02x at %04x:%04x' % (op, self.sregs['cs'], start))
