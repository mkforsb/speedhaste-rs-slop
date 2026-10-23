# Source this before running REA against the Speed Haste binary.
export GHIDRA_INSTALL_DIR=/tools/ghidra_12.1.4_PUBLIC
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk-amd64
export GHIDRA_HEADLESS_MAXMEM=4G
export REA_GHIDRA_STARTUP_TIMEOUT_MS=900000
# Watcom register calling convention isn't modelled; default x86 is the least-bad choice.
export REA_GHIDRA_LANGUAGE_ID="x86:LE:32:default"
export REA_GHIDRA_COMPILER_SPEC_ID="watcom"
