#!/bin/sh
set -eu

FUZZ_VERSION='cargo-fuzz 0.12.0'
FUZZ_TOOLCHAIN=${HQ_FUZZ_TOOLCHAIN:-nightly-2026-08-26}
FUZZ_DIRECTORY='crates/hq-protocol/fuzz'
FUZZ_WORK=$(mktemp -d)
trap 'rm -rf "$FUZZ_WORK"' EXIT HUP INT TERM

actual_version=$(cargo fuzz --version)
if [ "$actual_version" != "$FUZZ_VERSION" ]; then
  echo "expected $FUZZ_VERSION, found $actual_version" >&2
  exit 1
fi

for target in signed_event dto_content; do
  corpus="$FUZZ_WORK/$target"
  mkdir "$corpus"
  cp "$FUZZ_DIRECTORY/corpus/$target/"* "$corpus/"
  cargo "+$FUZZ_TOOLCHAIN" fuzz run "$target" \
    --fuzz-dir "$FUZZ_DIRECTORY" \
    "$corpus" \
    -- -runs=512 -max_len=4096 -timeout=5
done
