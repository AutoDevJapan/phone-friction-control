#!/bin/bash
# Product-owned compiler input driver, not a replacement copy of an SDK workflow.
set -euo pipefail
test "$#" = 1
OUT="$1"
test ! -e "$OUT"
mkdir -p "$OUT"
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
  ios/SelectionBridgeABI.c Cargo.lock > "$OUT/inputs-sha256.txt"
ARGS=(-target "$TARGET" -sdk "$SDK" -swift-version 6 -parse-as-library
  -module-name PFCSelection -import-objc-header ios/SelectionSession.h)
run_bounded() {
  local stage status started ended
  stage="$1"; shift
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
run_bounded ir xcrun swiftc "${ARGS[@]}" -emit-ir "${INPUTS[@]}" \
  -emit-objc-header-path "$OUT/PFCSelection-Swift.h" -o "$OUT/selection.ll" \
  > "$OUT/ir.stdout.txt" 2> "$OUT/ir.stderr.txt"
run_bounded c xcrun clang -target "$TARGET" -isysroot "$SDK" -fsyntax-only \
  -std=c11 -Wall -Wextra -Werror ios/SelectionBridgeABI.c \
  > "$OUT/c.stdout.txt" 2> "$OUT/c.stderr.txt"
# Generated-header export presence is an observation, not assumed ABI proof.
grep -n 'pfc_selection_' "$OUT/PFCSelection-Swift.h" > "$OUT/generated-c-exports.txt" || true
printf 'swift_typecheck=true\nlinked=false\nruntime_executed=false\n' > "$OUT/result.txt"
