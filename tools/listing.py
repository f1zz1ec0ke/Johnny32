"""Check the oracle listing in the Readme against what the oracle just said.

    python3 tools/oracle.py > out.txt
    python3 tools/listing.py out.txt

Twice now a number in the prose has gone stale while the thing it describes
moved on -- the configuration dialog advertising a default speed that had
changed, and this listing claiming 1,327 `tick` answers when there were 895.
Neither is executed by anything, which is exactly why neither was noticed.

The listing is quoted output, so it can be checked like output. `check.sh`
runs the oracle anyway, so this costs nothing beyond the comparison.
"""

import re
import sys

BLOCK = re.compile(r'^  (\w+)\s+(\d+) answers, all identical$')


def listed(path):
    """The name/count pairs in the Readme's fenced oracle listing."""
    out = []
    for line in open(path, encoding='utf-8'):
        m = BLOCK.match(line.rstrip('\n'))
        if m:
            out.append((m.group(1), int(m.group(2))))
    return out


def main():
    if len(sys.argv) < 2:
        raise SystemExit('usage: listing.py <oracle output>')
    real = listed(sys.argv[1])
    doc = listed('Readme.md')
    if not real:
        print('no oracle output to compare against')
        return 1
    if not doc:
        print('no listing found in Readme.md')
        return 1
    if real == doc:
        print('listing     %d queries, matching what the oracle printed'
              % len(real))
        return 0
    print('the Readme listing and the oracle disagree:')
    for name, n in real:
        was = dict(doc).get(name)
        if was != n:
            print('  %-10s Readme says %s, the oracle says %d'
                  % (name, was if was is not None else 'nothing', n))
    for name, n in doc:
        if name not in dict(real):
            print('  %-10s in the Readme, not in the oracle' % name)
    if [n for n, _c in real] != [n for n, _c in doc]:
        print('  (the order differs too)')
    return 1


if __name__ == '__main__':
    sys.exit(main())
