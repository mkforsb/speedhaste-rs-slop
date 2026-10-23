#!/bin/sh
# Experiment loop: render the car-select screen headless and count pixels
# differing from the original's capture (re/dos/capture/o_car.png) in the
# small-cars strip (y 45..85).
set -e
cd "$(dirname "$0")/../.."
cargo build -q -p speedhaste-core --example headless
./target/debug/examples/headless refs/full-game-dos/SPEEDH.JCL "wait:30,key:ESC,wait:300,key:ENTER,wait:30,key:DARROW,key:ENTER,wait:250,key:ENTER,wait:300,shot:p_car" re/out/cmp >/dev/null 2>&1
convert re/out/cmp/p_car.ppm -crop 320x40+0+45 +repage /tmp/a.png
convert re/dos/capture/o_car.png -crop 320x40+0+45 +repage /tmp/b.png
compare -metric AE /tmp/a.png /tmp/b.png null: 2>&1; echo
