#!/bin/sh
# Drive the original game in DOSBox (under Xvfb :99) for reference captures.
#   dosbox.sh start            launch DOSBox running SPEED.EXE
#   dosbox.sh key KEYS...      send xdotool key names (e.g. Return Up space)
#   dosbox.sh down KEY / up KEY
#   dosbox.sh shot NAME        save the 320x200 window as re/dos/capture/NAME.png
#   dosbox.sh stop
set -e
export DISPLAY=:99
D=$(dirname "$0")/../dos
# (An empty window id would make `import` grab the X server interactively.)
win() { w=$(timeout 10 xdotool search --class dosbox | head -1); [ -n "$w" ] || { echo "no DOSBox window" >&2; exit 1; }; echo "$w"; }
case "$1" in
  start) pkill -x dosbox || true; SDL_AUDIODRIVER=dummy dosbox -conf "$D/dosbox.conf" >/dev/null 2>&1 & sleep 3 ;;
  key) shift; for k in "$@"; do xdotool windowfocus --sync "$(win)"; xdotool keydown "$k"; sleep 0.15; xdotool keyup "$k"; sleep 0.25; done ;;
  down) xdotool windowfocus --sync "$(win)"; xdotool keydown "$2" ;;
  up) xdotool windowfocus --sync "$(win)"; xdotool keyup "$2" ;;
  shot) mkdir -p "$D/capture"; f="$D/capture/${2:-shot}.png"; import -window "$(win)" "$f.tmp.png"; convert "$f.tmp.png" +repage -sample 320x200! -define png:color-type=2 "$f"; rm "$f.tmp.png"; echo "$f" ;;
  stop) pkill -x dosbox || true ;;
esac
