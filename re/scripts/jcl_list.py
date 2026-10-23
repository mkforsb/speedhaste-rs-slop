#!/usr/bin/env python3
"""List / extract entries of a Speed Haste .JCL archive.
usage: jcl_list.py FILE.JCL [OUTDIR]"""
import struct, sys, os
d = open(sys.argv[1], 'rb').read()
magic, nfiles, realn, lastoff = struct.unpack_from('<IiiI', d, len(d) - 16)
assert magic == 0xdf73b489, hex(magic)
dirpos = len(d) - lastoff
base = dirpos
entries = []
for i in range(nfiles):
    name, off, size = struct.unpack_from('<24sii', d, dirpos + i * 32)
    name = name.split(b'\0')[0].decode('latin1')
    entries.append((name, base - off, size))
print(f'nfiles={nfiles} realnfiles={realn} lastoffset={lastoff}', file=sys.stderr)
out = sys.argv[2] if len(sys.argv) > 2 else None
for name, off, size in entries:
    print(f'{name:24s} {off:9d} {size:9d}')
    if out:
        os.makedirs(out, exist_ok=True)
        open(os.path.join(out, name), 'wb').write(d[off:off + size])
