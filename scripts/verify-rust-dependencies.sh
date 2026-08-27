#!/bin/sh
set -eu

FUZZ_MANIFEST='crates/hq-protocol/fuzz/Cargo.toml'
FUZZ_POLICY='crates/hq-protocol/fuzz/deny.toml'

cargo deny check
cargo deny --manifest-path "$FUZZ_MANIFEST" --config "$FUZZ_POLICY" check
