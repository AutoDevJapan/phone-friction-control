#!/bin/bash
# Product-owned compiler input driver, not a replacement copy of an SDK workflow.
set -euo pipefail
test "$#" = 1
OUT="$1"
test ! -e "$OUT"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
git rev-parse HEAD > "$OUT/source.txt"
xcodebuild -version > "$OUT/versions.txt"
xcrun swiftc --version >> "$OUT/versions.txt"
test "$(xcrun --sdk iphonesimulator --show-sdk-version)" = 18.5
grep -F 'Swift version 6.1.2' "$OUT/versions.txt"
SDK=$(xcrun --sdk iphonesimulator --show-sdk-path)
TARGET=arm64-apple-ios18.5-simulator
INPUTS=(ios/SelectionCodec.swift ios/SelectionBridge.swift ios/SelectionPicker.swift
  ios/SelectionEmbedding.swift ios/SelectionSessionAdapter.swift)
shasum -a 256 "${INPUTS[@]}" ios/SelectionSession.h ios/SelectionBridge.h \
  ios/SelectionBridgeABI.c ios/SelectionCodecTests.swift \
  ios/compile-selection-codec.sh ios/check-compiler-evidence.sh Cargo.lock > "$OUT/inputs-sha256.txt"
ARGS=(-target "$TARGET" -sdk "$SDK" -swift-version 6 -parse-as-library
  -module-name PFCSelection -import-objc-header ios/SelectionSession.h)
run_bounded() {
  local stage status started ended
  stage="$1"; shift
  # NUL-separated argv is exact even for spaces/newlines; %q is a readable
  # shell rendering of the same arguments, not an executable reconstruction.
  printf '%s\0' "$@" > "$OUT/$stage-argv.nul"
  printf '%q ' "$@" > "$OUT/$stage-command.txt"
  printf '\n' >> "$OUT/$stage-command.txt"
  started=$(date +%s)
  if perl -e 'alarm 60; exec @ARGV; die "exec failed\n"' "$@"; then status=0; else status=$?; fi
  ended=$(date +%s)
  printf 'stage=%s\nstarted_epoch=%s\nended_epoch=%s\nstatus=%s\nlimit_seconds=60\nlimit_sufficiency_measured=false\n' \
    "$stage" "$started" "$ended" "$status" > "$OUT/$stage-status.txt"
  return "$status"
}
# Each full diagnostic stream is retained; a failed stage stops compilation.
run_bounded typecheck xcrun swiftc "${ARGS[@]}" -typecheck "${INPUTS[@]}" \
  > "$OUT/typecheck.stdout.txt" 2> "$OUT/typecheck.stderr.txt"
run_bounded tests xcrun swiftc "${ARGS[@]}" -typecheck "${INPUTS[@]}" ios/SelectionCodecTests.swift \
  -F "$(xcode-select -p)/Platforms/iPhoneSimulator.platform/Developer/Library/Frameworks" \
  > "$OUT/tests.stdout.txt" 2> "$OUT/tests.stderr.txt"
run_bounded ir xcrun swiftc "${ARGS[@]}" -whole-module-optimization -emit-ir "${INPUTS[@]}" \
  -emit-objc-header-path "$OUT/PFCSelection-Swift.h" -o "$OUT/selection.ll" \
  > "$OUT/ir.stdout.txt" 2> "$OUT/ir.stderr.txt"
run_bounded object xcrun swiftc "${ARGS[@]}" -whole-module-optimization -emit-object "${INPUTS[@]}" \
  -o "$OUT/selection.o" > "$OUT/object.stdout.txt" 2> "$OUT/object.stderr.txt"
run_bounded symbols xcrun nm -gU "$OUT/selection.o" \
  > "$OUT/exported-symbols.txt" 2> "$OUT/symbols.stderr.txt"
for SYMBOL in decode encode equal release; do
  grep -Eq "[[:space:]]_pfc_selection_${SYMBOL}$" "$OUT/exported-symbols.txt"
done
run_bounded c xcrun clang -target "$TARGET" -isysroot "$SDK" -fsyntax-only \
  -std=c11 -Wall -Wextra -Werror ios/SelectionBridgeABI.c \
  > "$OUT/c.stdout.txt" 2> "$OUT/c.stderr.txt"
# A header without C exports is not an ABI comparison. Stop honestly rather
# than treating the handwritten-header syntax check as generated ABI evidence.
for SYMBOL in decode encode equal release; do
  if ! grep -Eq "pfc_selection_${SYMBOL}[[:space:]]*\\(" "$OUT/PFCSelection-Swift.h"; then
    printf 'swift_typecheck=true\ngenerated_abi_compared=false\nreason=generated_export_missing\n' > "$OUT/result.txt"
    exit 1
  fi
done
printf '#include "%s/PFCSelection-Swift.h"\n#include "%s/ios/SelectionBridgeABI.c"\n' \
  "$OUT" "$ROOT" > "$OUT/generated-consumer.m"
run_bounded generated_abi xcrun clang -target "$TARGET" -isysroot "$SDK" \
  -fsyntax-only -x objective-c -Wall -Wextra -Werror "$OUT/generated-consumer.m" \
  > "$OUT/generated-abi.stdout.txt" 2> "$OUT/generated-abi.stderr.txt"
# Non-vacuous rejection control using the same actual generated declarations.
printf '#include "%s/PFCSelection-Swift.h"\nvoid pfc_selection_decode(void);\n' \
  "$OUT" > "$OUT/generated-mismatch.m"
if run_bounded signature_negative xcrun clang -target "$TARGET" -isysroot "$SDK" \
  -fsyntax-only -x objective-c -Wall -Wextra -Werror "$OUT/generated-mismatch.m" \
  > "$OUT/signature-negative.stdout.txt" 2> "$OUT/signature-negative.stderr.txt"; then
  echo 'signature mismatch unexpectedly accepted' >&2
  exit 1
fi
grep -Eq 'conflicting types|conflicting.*declaration' "$OUT/signature-negative.stderr.txt"
printf 'swift_typecheck=true\ngenerated_abi_compared=true\nsignature_negative_rejected=true\nlinked=false\nruntime_executed=false\n' > "$OUT/result.txt"
bash ios/check-compiler-evidence.sh "$OUT"
