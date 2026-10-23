#!/bin/sh
# Install the Watcom compiler spec into a Ghidra installation.
set -e
L="$1/Ghidra/Processors/x86/data/languages"
cp "$(dirname "$0")/x86watcom.cspec" "$L/"
grep -q 'id="watcom"' "$L/x86.ldefs" || \
  sed -i '0,/<compiler name="gcc" spec="x86gcc.cspec" id="gcc"\/>/s//&\n    <compiler name="Watcom" spec="x86watcom.cspec" id="watcom"\/>/' "$L/x86.ldefs"
grep -n watcom "$L/x86.ldefs"
