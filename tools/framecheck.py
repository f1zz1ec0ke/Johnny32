"""Look at what every scene actually draws.

    python3 tools/framecheck.py

`tools/sweep.py` proves a tick can be reconstructed; it says nothing about
whether the picture is a picture. This renders a sample of frames from every
one of the sixty-five scenes and complains about the two things that would be
obvious to a person and are invisible to every other check here:

* a frame that is all one colour -- a scene drawing onto nothing, or clipped
  down to nothing;
* a run of frames in which nothing at all moves -- a scene stuck on its first
  frame, or one whose instance never advances.

Both have happened. The standing sequences drew a completely static picture
when `F200`'s target was being converted to state 1 a pass too early, and the
raft came out as three red pixels when one scene inherited another's clip.
Neither showed up in the repaint check, which only asks whether the tick can
be painted again -- an empty picture repaints perfectly.

It is slow: sixty-five renders of several hundred 640x480 frames each.
"""

import glob
import os
import struct
import subprocess
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import scenes as scene_table

PLAY = './johnny/target/release/jcplay'
FRAMES = 'frames'
CAP = '900'
SAMPLE = 12


def read_png(path):
    """Just enough PNG to get the indexed pixels back out of what the port
    wrote: 8-bit, one byte a pixel, every filter type."""
    data = open(path, 'rb').read()
    i, idat, w, h = 8, b'', None, None
    while i < len(data):
        length = struct.unpack_from('>I', data, i)[0]
        kind = data[i + 4:i + 8]
        payload = data[i + 8:i + 8 + length]
        if kind == b'IHDR':
            w, h = struct.unpack_from('>II', payload, 0)
        elif kind == b'IDAT':
            idat += payload
        i += 12 + length
    raw = zlib.decompress(idat)
    out, prev, pos = bytearray(), bytearray(w), 0
    for _ in range(h):
        f = raw[pos]
        pos += 1
        line = bytearray(raw[pos:pos + w])
        pos += w
        if f == 1:
            for x in range(1, w):
                line[x] = (line[x] + line[x - 1]) & 255
        elif f == 2:
            for x in range(w):
                line[x] = (line[x] + prev[x]) & 255
        elif f == 3:
            for x in range(w):
                line[x] = (line[x] + ((line[x - 1] if x else 0) + prev[x]) // 2) & 255
        elif f == 4:
            for x in range(w):
                a = line[x - 1] if x else 0
                b = prev[x]
                c = prev[x - 1] if x else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else
                                      b if pb <= pc else c)) & 255
        out += line
        prev = line
    return w, h, bytes(out)


def main():
    if not os.path.exists(PLAY):
        print('%s is not built' % PLAY)
        return 1
    os.makedirs(FRAMES, exist_ok=True)
    bad, looked = [], 0
    rows = scene_table.scenes()
    for name, seq in rows:
        for f in glob.glob(os.path.join(FRAMES, 'play_*.png')):
            os.remove(f)
        subprocess.run([PLAY, name, seq, CAP], capture_output=True)
        files = sorted(glob.glob(os.path.join(FRAMES, 'play_*.png')))
        if not files:
            bad.append('%s %s: drew nothing at all' % (name, seq))
            continue
        # Sample across the run rather than the first few, which are often a
        # backdrop with nothing on it yet.
        sample = files[::max(1, len(files) // SAMPLE)][:SAMPLE]
        frames, uniform = [], 0
        for path in sample:
            _, _, px = read_png(path)
            looked += 1
            if len(set(px)) < 3:
                uniform += 1
            frames.append(px)
        if uniform:
            bad.append('%s %s: %d of %d sampled frames are one flat colour'
                       % (name, seq, uniform, len(sample)))
        moved = sum(1 for a, b in zip(frames, frames[1:]) if a != b)
        if len(frames) > 2 and moved == 0:
            bad.append('%s %s: nothing moved across %d frames'
                       % (name, seq, len(frames)))
    for f in glob.glob(os.path.join(FRAMES, 'play_*.png')):
        os.remove(f)
    print('%d frames looked at across %d scenes' % (looked, len(rows)))
    for line in bad:
        print('  ' + line)
    print('%d complaints' % len(bad))
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
