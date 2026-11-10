#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
OUT=${1:-"$ROOT/build"}
CC=${CC:-cc}
STRIP=${STRIP:-strip}
mkdir -p "$OUT"
# Keep this x86-64 ELF benchmark reproducible and the fault attributable to the
# source-level bounds error. NX stays enabled; canaries/PIE are disabled here.
"$CC" -std=c11 -O0 -g -fno-omit-frame-pointer -fno-stack-protector -no-pie -Wl,-z,noexecstack "$ROOT/src/overflow.c" -o "$OUT/overflow-vuln"
"$CC" -std=c11 -O0 -g -fno-omit-frame-pointer -fno-stack-protector -no-pie -Wl,-z,noexecstack -DFIXED "$ROOT/src/overflow.c" -o "$OUT/overflow-fixed"
"$CC" -std=c11 -O0 -g -fno-omit-frame-pointer -fstack-protector-strong -fPIE -pie -Wl,-z,relro,-z,now,-z,noexecstack "$ROOT/src/benign.c" -o "$OUT/benign"
for NAME in overflow-vuln overflow-fixed benign; do
    cp "$OUT/$NAME" "$OUT/$NAME-stripped"
    "$STRIP" --strip-all "$OUT/$NAME-stripped"
done
OUT=$(CDPATH= cd -- "$OUT" && pwd)
(cd "$ROOT/.." && python3 -m evals.vulnerability.evaluate --build-dir "$OUT" --prepare-only)
"$CC" --version | head -1
printf '%s\n' "Built benchmark at $OUT"
