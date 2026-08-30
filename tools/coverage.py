"""Walk the parts of the island a soak from a fixed seed does not reach.

    python3 tools/coverage.py

A soak plays whatever the generator happens to pick on day one in daylight.
Three things are outside that by construction, and each of them is a different
set of scenes and a different backdrop:

* **every story day.** The raft gets built over eleven of them, and the last
  one sails away in it. `JC_DAY=n` starts there.
* **every holiday.** Seven decorations, each keyed to a date, each drawn by a
  scene the scheduler will not otherwise offer. `JC_DATE=YYYY-MM-DD` pins the
  calendar.
* **both halves of the clock.** Phase 12 onward is night, which is a different
  sea (`NIGHT.SCR`) and a different set of scenes. `JC_HHMM=hhmm` pins it.

Each run is short and asks the two questions a run can answer on its own: did
the repaint check stay clean, and did anything turn up that the port has not
implemented.
"""

import os
import subprocess
import sys

ISLAND = './johnny/target/release/jcisland'

DAYS = range(1, 12)
HOLIDAYS = ['1992-01-01', '1992-02-14', '1992-03-17', '1992-07-04',
            '1992-10-31', '1992-11-26', '1992-12-25']
CLOCK = ['0800', '0900', '1100', '1300', '1500', '1600', '1630']


def run(label, extra, ticks, seed):
    env = dict(os.environ, JC_CHECK='1', JC_NOWRITE='1', **extra)
    out = subprocess.run([ISLAND, str(ticks), str(seed)],
                         capture_output=True, text=True, env=env).stdout
    if 'repaint check: clean' not in out:
        return '%s: repaint check not clean' % label
    if 'not implemented' in out:
        return '%s: %s' % (label, out[out.index('not implemented'):][:60])
    return None


def main():
    if not os.path.exists(ISLAND):
        print('%s is not built' % ISLAND)
        return 1
    bad = []
    runs = 0
    for d in DAYS:
        runs += 1
        bad.append(run('day %d' % d, {'JC_DAY': str(d)}, 4000, d * 17))
    for date in HOLIDAYS:
        runs += 1
        bad.append(run(date, {'JC_DATE': date}, 3000, 42))
    for t in CLOCK:
        runs += 1
        bad.append(run(t, {'JC_HHMM': t}, 3000, 5))
    bad = [b for b in bad if b]
    print('%d runs: %d clean' % (runs, runs - len(bad)))
    for line in bad[:20]:
        print('  ' + line)
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
