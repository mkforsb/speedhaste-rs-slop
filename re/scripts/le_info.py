#!/usr/bin/env python3
"""Dump LE (DOS/4GW linear executable) header and object table."""
import struct, sys
d = open(sys.argv[1], 'rb').read()
le = struct.unpack_from('<I', d, 0x3c)[0]
assert d[le:le+2] == b'LE'
f = lambda off, fmt='<I': struct.unpack_from(fmt, d, le + off)[0]
hdr = dict(
    cpu=f(0x08, '<H'), os=f(0x0a, '<H'), npages=f(0x14), eip_obj=f(0x18), eip=f(0x1c),
    esp_obj=f(0x20), esp=f(0x24), pagesize=f(0x28), lastpage=f(0x2c),
    fixup_size=f(0x30), loader_size=f(0x38), objtab=f(0x40), nobjs=f(0x44),
    objpagetab=f(0x48), resnametab=f(0x58), fixpagetab=f(0x68), fixrectab=f(0x6c),
    datapages=f(0x80), nresnametab=f(0x88), autods=f(0x94), debuginfo=f(0x98), debuglen=f(0x9c))
for k, v in hdr.items(): print(f'{k:12s} {v:#x}')
for i in range(hdr['nobjs']):
    vsize, base, flags, pti, npg, _ = struct.unpack_from('<6I', d, le + hdr['objtab'] + 24*i)
    print(f'obj{i+1}: vsize={vsize:#x} base={base:#x} flags={flags:#x} pagetab_idx={pti} npages={npg}')
