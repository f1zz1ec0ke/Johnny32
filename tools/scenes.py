"""Every scene the scheduler can pick, as (ADS file, sequence).

Derived from `johnny/src/tables.rs` rather than written down, because a list
written down goes stale the moment the table changes and a sweep that quietly
stops covering a scene is worse than no sweep. `tools/verify.py` checks that
`tables.rs` is byte for byte what `gen_tables.py` writes out of the binary, so
this is the binary's own list at one remove.

There are 65 of them across the ten `.ADS` files.
"""

import re

TABLES = 'johnny/src/tables.rs'


def scenes(path=TABLES):
    """The distinct (file, sequence) pairs, in table order."""
    src = open(path).read()
    at = src.index('ADS_FILES')
    files = dict(re.findall(r'\((\d+),\s*"([A-Z0-9_.]+)"\)', src[at:at + 2000]))
    out = []
    for m in re.finditer(r'ads:\s*(\d+),\s*sequence:\s*(\d+)', src):
        ads, seq = m.group(1), m.group(2)
        # 255 is the table's "no script": Johnny stands still for a moment.
        pair = (files.get(ads), seq)
        if pair[0] and pair not in out:
            out.append(pair)
    if len(out) < 50:
        # A regex that has stopped matching returns a short list rather than
        # an error, and a sweep over four scenes passes very quickly.
        raise SystemExit('%s: only %d scenes matched, which cannot be right'
                         % (path, len(out)))
    return out


if __name__ == '__main__':
    for name, seq in scenes():
        print('%s %s' % (name, seq))
