#!/bin/bash
# Verify compile-only evidence completeness; never infer runtime acceptance.
set -euo pipefail
test "$#" = 1
OUT="$1"
status_matches() {
  awk -v expected="$1" '
    /^status=/ {
      count++
      if ($0 !~ /^status=(0|[1-9][0-9]*)$/) bad=1
      if (expected == "zero" && $0 != "status=0") bad=1
      if (expected == "nonzero" && $0 !~ /^status=[1-9][0-9]*$/) bad=1
    }
    END { exit (bad || count != 1) }
  '
}
for stage in typecheck tests ir object symbols c generated_abi; do
  test -s "$OUT/$stage-argv.nul"
  test -s "$OUT/$stage-command.txt"
  status_matches zero < "$OUT/$stage-status.txt"
done
test -s "$OUT/signature_negative-argv.nul"
status_matches nonzero < "$OUT/signature_negative-status.txt"
grep -Eq 'conflicting types|conflicting.*declaration' "$OUT/signature-negative.stderr.txt"
for file in inputs-sha256.txt source.txt versions.txt selection.ll selection.o \
  PFCSelection-Swift.h exported-symbols.txt; do
  test -s "$OUT/$file"
done
grep -F 'ios/SelectionCodecTests.swift' "$OUT/inputs-sha256.txt" >/dev/null
grep -F 'ios/compile-selection-codec.sh' "$OUT/inputs-sha256.txt" >/dev/null
for item in swift_typecheck=true generated_abi_compared=true signature_negative_rejected=true linked=false runtime_executed=false; do
  test "$(grep -Fxc "$item" "$OUT/result.txt")" = 1
done
