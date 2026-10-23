#!/bin/sh
# Record the original game's audio output (VTAL's software mixer through a
# Sound Blaster Pro at 22050 Hz) with DOSBox under Xvfb :99.
#   dosbox_audio.sh OUT.raw SECONDS [game options]
# Output: raw s16le stereo at 44100 Hz (DOSBox's mixer rate). Effects are
# off (FXOn No) so the music stays clean; with -nologo the menu song starts
# ~4 s in and, after 20 s idle, the attract song HASTE9 at ~26 s.
# Notes: cycles=max made DOSBox fall behind and pad the audio (2/3 speed);
# VTAL's SB16 driver puts 16-bit DMA on the 8-bit channel, which DOSBox
# garbles, hence the SB Pro. The SB Pro plays at 21739 Hz, not 22050.
set -e
D=$(cd "$(dirname "$0")/../dos" && pwd)
rm -rf "$D/game_sb" && cp -r "$D/game" "$D/game_sb"
sed -i 's/^\(MusicDevice\|FXDevice\)\(\s\+\).*/\1\2"Soundblaster Pro"/; s/^\(MusicPort\|FXPort\)\(\s\+\).*/\1\2$0220/;
        s/^\(MusicIRQ\|FXIRQ\)\(\s\+\).*/\1\27/; s/^\(MusicDMA\|FXDMA\)\(\s\+\).*/\1\21/;
        s/^\(MusicRate\|FXRate\)\(\s\+\).*/\1\222050/; s/^\(FXOn\s\+\).*/\1No/' "$D/game_sb/SPEED.CFG"
OUT=$1; SECS=$2; shift 2
sed -e "s#^cycles=.*#cycles=fixed 40000#" -e "s#^sbtype=.*#sbtype=sbpro2\nirq=7\ndma=1#" \
    -e "s#/re/dos/game\$#/re/dos/game_sb#" -e "s#^speed.exe.*#speed.exe $*#" "$D/dosbox.conf" > "$D/dosbox_sb.conf"
pkill -x dosbox || true
DISPLAY=:99 SDL_AUDIODRIVER=disk SDL_DISKAUDIOFILE="$OUT" dosbox -conf "$D/dosbox_sb.conf" >/dev/null 2>&1 &
sleep "$SECS"; pkill -x dosbox || true
