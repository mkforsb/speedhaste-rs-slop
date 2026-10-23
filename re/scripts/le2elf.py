#!/usr/bin/env python3
"""Convert a DOS/4GW LE executable into a static i386 ELF32 so Ghidra (and
therefore REA) can load it.

Objects are placed at their LE preferred base addresses, all internal fixups
are applied, and each object becomes a PT_LOAD segment plus a section.

usage: le2elf.py SPEED.EXE OUT.elf
"""
import struct
import sys


def parse_le(d):
    le = struct.unpack_from('<I', d, 0x3c)[0]
    assert d[le:le + 2] == b'LE', 'not an LE executable'
    u32 = lambda off: struct.unpack_from('<I', d, le + off)[0]
    h = dict(npages=u32(0x14), eip_obj=u32(0x18), eip=u32(0x1c), pagesize=u32(0x28),
             lastpage=u32(0x2c), objtab=u32(0x40), nobjs=u32(0x44), objpagetab=u32(0x48),
             fixpagetab=u32(0x68), fixrectab=u32(0x6c), datapages=u32(0x80))
    objs = []
    for i in range(h['nobjs']):
        vsize, base, flags, pti, npg, _ = struct.unpack_from('<6I', d, le + h['objtab'] + 24 * i)
        objs.append(dict(vsize=vsize, base=base, flags=flags, pti=pti, npg=npg))
    return le, h, objs


def load_pages(d, le, h, objs):
    """Return per-object bytearrays (vsize long) and a page->(obj, offset) map."""
    images = []
    pagemap = {}  # 1-based logical page index -> (obj index, offset in obj)
    for oi, o in enumerate(objs):
        img = bytearray(max(o['vsize'], o['npg'] * h['pagesize']))
        for p in range(o['npg']):
            idx = o['pti'] + p  # 1-based
            ent = d[le + h['objpagetab'] + 4 * (idx - 1): le + h['objpagetab'] + 4 * idx]
            pnum = (ent[0] << 16) | (ent[1] << 8) | ent[2]
            ptype = ent[3]
            assert ptype == 0, f'unsupported page type {ptype}'
            size = h['lastpage'] if pnum == h['npages'] else h['pagesize']
            src = h['datapages'] + (pnum - 1) * h['pagesize']
            img[p * h['pagesize']: p * h['pagesize'] + size] = d[src:src + size]
            pagemap[idx] = (oi, p * h['pagesize'])
        images.append(img)
    return images, pagemap


def apply_fixups(d, le, h, objs, images, pagemap):
    fpt = le + h['fixpagetab']
    frt = le + h['fixrectab']
    stats = {}
    for idx in range(1, h['npages'] + 1):
        start, end = struct.unpack_from('<II', d, fpt + 4 * (idx - 1))
        pos = frt + start
        end = frt + end
        oi, pageoff = pagemap[idx]
        img = images[oi]
        while pos < end:
            src = d[pos]; flg = d[pos + 1]; pos += 2
            if src & 0x20:
                cnt = d[pos]; pos += 1
                srcoffs = None
            else:
                srcoffs = [struct.unpack_from('<h', d, pos)[0]]; pos += 2
            tgt = flg & 3
            assert tgt == 0, f'non-internal fixup target {tgt}'
            if flg & 0x40:
                tobj = struct.unpack_from('<H', d, pos)[0]; pos += 2
            else:
                tobj = d[pos]; pos += 1
            stype = src & 0x0f
            if stype == 2:      # 16-bit selector: no offset field
                toff = 0
            elif flg & 0x10:
                toff = struct.unpack_from('<I', d, pos)[0]; pos += 4
            else:
                toff = struct.unpack_from('<H', d, pos)[0]; pos += 2
            assert not (flg & 0x04), 'additive fixups unsupported'
            if srcoffs is None:
                srcoffs = list(struct.unpack_from(f'<{cnt}h', d, pos)); pos += 2 * cnt
            target = objs[tobj - 1]['base'] + toff
            for so in srcoffs:
                off = pageoff + so
                if off < 0:
                    continue    # Fixup straddling from the previous page; applied there.
                stats[stype] = stats.get(stype, 0) + 1
                if stype == 7:      # 32-bit offset
                    if off + 4 <= len(img):
                        struct.pack_into('<I', img, off, target & 0xffffffff)
                elif stype == 8:    # 32-bit self-relative
                    srcaddr = objs[oi]['base'] + off + 4
                    if off + 4 <= len(img):
                        struct.pack_into('<i', img, off, (target - srcaddr + 2**31) % 2**32 - 2**31)
                elif stype == 5:    # 16-bit offset
                    if off + 2 <= len(img):
                        struct.pack_into('<H', img, off, target & 0xffff)
                elif stype == 2:    # 16-bit selector: flat model, use a fake selector
                    if off + 2 <= len(img):
                        struct.pack_into('<H', img, off, tobj * 8)
                elif stype == 6:    # 16:32 pointer
                    if off + 6 <= len(img):
                        struct.pack_into('<IH', img, off, target & 0xffffffff, tobj * 8)
                elif stype == 3:    # 16:16 pointer
                    if off + 4 <= len(img):
                        struct.pack_into('<HH', img, off, target & 0xffff, tobj * 8)
                else:
                    raise ValueError(f'unsupported fixup source type {stype}')
    return stats


def write_elf(path, objs, images, entry):
    # Layout: ELF header, program headers, segment data (page aligned), shstrtab, section headers.
    nph = len(objs)
    ehsize, phsize, shsize = 52, 32, 40
    off = 0x1000
    segs = []
    for o, img in zip(objs, images):
        data = bytes(img[:o['vsize']])
        segs.append((o, off, data))
        off += (len(data) + 0xfff) & ~0xfff
    names = [b''] + [b'.text' if o['flags'] & 4 else b'.data' for o in objs] + [b'.shstrtab']
    shstr = b'\0'
    name_off = []
    for n in names:
        if n:
            name_off.append(len(shstr)); shstr += n + b'\0'
        else:
            name_off.append(0)
    shstr_off = off
    off += len(shstr)
    off = (off + 3) & ~3
    shoff = off
    nsh = len(names)
    out = bytearray(shoff + nsh * shsize)
    struct.pack_into('<4sBBBBB7xHHIIIIIHHHHHH', out, 0, b'\x7fELF', 1, 1, 1, 0, 0,
                     2, 3, 1, entry, ehsize, shoff, 0, ehsize, phsize, nph, shsize, nsh, nsh - 1)
    for i, (o, foff, data) in enumerate(segs):
        pflags = (4 if o['flags'] & 1 else 0) | (2 if o['flags'] & 2 else 0) | (1 if o['flags'] & 4 else 0)
        struct.pack_into('<8I', out, ehsize + i * phsize, 1, foff, o['base'], o['base'],
                         len(data), o['vsize'], pflags, 0x1000)
        out[foff:foff + len(data)] = data
    out[shstr_off:shstr_off + len(shstr)] = shstr
    # Section headers.
    sh = shoff + shsize  # index 0 is null
    for i, (o, foff, data) in enumerate(segs):
        shflags = 2 | (1 if o['flags'] & 2 else 0) | (4 if o['flags'] & 4 else 0)  # ALLOC|WRITE|EXEC
        struct.pack_into('<10I', out, sh, name_off[i + 1], 1, shflags, o['base'], foff,
                         len(data), 0, 0, 16, 0)
        sh += shsize
    struct.pack_into('<10I', out, sh, name_off[-1], 3, 0, 0, shstr_off, len(shstr), 0, 0, 1, 0)
    open(path, 'wb').write(out)


def main():
    d = open(sys.argv[1], 'rb').read()
    le, h, objs = parse_le(d)
    images, pagemap = load_pages(d, le, h, objs)
    stats = apply_fixups(d, le, h, objs, images, pagemap)
    entry = objs[h['eip_obj'] - 1]['base'] + h['eip']
    write_elf(sys.argv[2], objs, images, entry)
    print(f'objects: {[(hex(o["base"]), hex(o["vsize"])) for o in objs]}')
    print(f'fixups applied by source type: {stats}')
    print(f'entry: {entry:#x}')


if __name__ == '__main__':
    main()
