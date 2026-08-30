"""Play every scene the scheduler can pick, with the repaint check on.

    python3 tools/sweep.py

Sixty-five sequences, each run until it settles. For each one this asks three
questions:

* did the repaint check stay clean -- every tick reconstructible from the
  frames alone, pixels and everything else (`JC_CHECK=1`, and see
  `johnny::director::check_repaint`);
* did the sequence *finish*, rather than run into the tick limit;
* did anything turn up that the port has not implemented.

A scene that stops finishing is the loudest signal in this repository that a
sequencer change has gone wrong, and it is not something the oracle can see:
the oracle compares one pass against the binary, and this is what happens when
thousands of them run in a row with scenes ending in between.
"""

import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import scenes as scene_table

PLAY = './johnny/target/release/jcplay'
TICKS = '20000'


def main():
    if not os.path.exists(PLAY):
        print('%s is not built' % PLAY)
        return 1
    env = dict(os.environ, JC_CHECK='1', JC_NOWRITE='1')
    rows = scene_table.scenes()
    clean = finished = 0
    bad = []
    for name, seq in rows:
        out = subprocess.run([PLAY, name, seq, TICKS], capture_output=True,
                             text=True, env=env).stdout
        if 'repaint check: clean' in out:
            clean += 1
        else:
            bad.append('%s %s: repaint check not clean' % (name, seq))
        if 'sequence finished' in out:
            finished += 1
        else:
            bad.append('%s %s: never finished' % (name, seq))
        if 'not implemented' in out:
            bad.append('%s %s: %s' % (name, seq, out[out.index('not implemented'):][:60]))
    print('%d scenes: %d repaint-clean, %d finished' % (len(rows), clean, finished))
    for line in bad[:20]:
        print('  ' + line)
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
