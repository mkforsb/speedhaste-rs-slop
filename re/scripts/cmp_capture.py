#!/usr/bin/env python3
"""Compare a DOSBox audio capture (re/scripts/dosbox_audio.sh) with our renders
(s3m_render example, raw s16 stereo 22050 Hz): band-energy and RMS envelope
correlation after alignment, and level ratio.
usage: cmp_capture.py CAPTURE.raw START_S DURATION_S OURS.raw...  (needs numpy)"""
import numpy as np, sys
def load(path, step): return np.fromfile(path,dtype=np.int16).reshape(-1,2).astype(float)[::step]
def first(a): return int(np.argmax(np.abs(a).sum(axis=1)>200))
cap, t0, dur = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
D=load(cap,2)[int(t0*22050):]; D=D[first(D):]; D=D[np.arange(0,len(D)-1,22050/21739).astype(int)]
def bands(m, hop=441, n=2048, nb=16):
    edges=np.geomspace(60,8000,nb+1); f=np.fft.rfftfreq(n,1/22050)
    S=np.array([np.abs(np.fft.rfft(m[i:i+n]*np.hanning(n)))**2 for i in range(0,len(m)-n,hop)])
    return np.log10(np.stack([S[:,(f>=edges[k])&(f<edges[k+1])].sum(axis=1) for k in range(nb)],axis=1)+1e3)
def env(m, W=441): n=len(m)//W; return np.sqrt((m[:n*W].reshape(n,W)**2).mean(axis=1))
N=int(dur*22050)
BD=bands(D[:N].mean(axis=1)); ED=env(D[:N].mean(axis=1))
for path in sys.argv[4:]:
    O=load(path,1); O=O[first(O):][:N]; BO=bands(O.mean(axis=1)); EO=env(O.mean(axis=1))
    best=(-9,0)
    for k in range(-25,26):
        a=BD[max(k,0):]; b=BO[max(-k,0):]; m=min(len(a),len(b))
        c=np.mean([np.corrcoef(a[:m,j],b[:m,j])[0,1] for j in range(16)]); best=max(best,(c,k))
    k=best[1]; a=ED[max(k,0):]; b=EO[max(-k,0):]; m=min(len(a),len(b))
    print(f'{path}: band corr {best[0]:.3f} at {k*20:+d} ms, RMS envelope corr {np.corrcoef(a[:m],b[:m])[0,1]:.3f}, level ours/dosbox {b[:m].mean()/a[:m].mean():.2f}')
