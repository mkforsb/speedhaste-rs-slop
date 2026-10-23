#!/usr/bin/env python3
"""Compare our S3M renders (WAV) with libopenmpt's (raw s16 stereo): RMS
envelope correlation and spectral centroid per song.
usage: cmp_audio.py OURS_DIR OMPT_DIR NAMES..."""
import struct, sys, math
def load(path, raw):
    d = open(path, 'rb').read()
    if not raw:
        d = d[44:]
    n = len(d) // 2
    s = struct.unpack(f'<{n}h', d[:n * 2])
    return [(s[i] + s[i + 1]) / 65536 for i in range(0, n - 1, 2)]
def env(x, w=1102):
    return [math.sqrt(sum(v * v for v in x[i:i + w]) / w) for i in range(0, len(x) - w, w)]
def corr(a, b):
    n = min(len(a), len(b)); a, b = a[:n], b[:n]
    ma, mb = sum(a) / n, sum(b) / n
    num = sum((x - ma) * (y - mb) for x, y in zip(a, b))
    den = math.sqrt(sum((x - ma) ** 2 for x in a) * sum((y - mb) ** 2 for y in b))
    return num / den if den else 0
for name in sys.argv[3:]:
    a = load(f'{sys.argv[1]}/{name}.S3M.wav', False)
    b = load(f'{sys.argv[2]}/{name}.raw', True)
    ea, eb = env(a), env(b)
    ra = sum(ea) / len(ea); rb = sum(eb) / len(eb)
    print(f'{name:10s} envelope corr {corr(ea, eb):.3f}  level ours/ompt {ra / rb if rb else 0:.2f}')
