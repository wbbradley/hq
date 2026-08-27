#!/bin/sh
set -eu

FUZZ_VERSION='cargo-fuzz 0.12.0'
FUZZ_TOOLCHAIN=${HQ_FUZZ_TOOLCHAIN:-nightly-2026-08-26}
FUZZ_DIRECTORY='crates/hq-protocol/fuzz'
FUZZ_CORPUS=$(mktemp -d)
trap 'rm -rf "$FUZZ_CORPUS"' EXIT HUP INT TERM

actual_version=$(cargo fuzz --version)
if [ "$actual_version" != "$FUZZ_VERSION" ]; then
  echo "expected $FUZZ_VERSION, found $actual_version" >&2
  exit 1
fi

cp "$FUZZ_DIRECTORY/corpus/signed_event/canonical-event.json" "$FUZZ_CORPUS/"
cargo "+$FUZZ_TOOLCHAIN" fuzz run signed_event \
  --fuzz-dir "$FUZZ_DIRECTORY" \
  "$FUZZ_CORPUS" \
  -- -runs=512 -max_len=4096 -timeout=5
