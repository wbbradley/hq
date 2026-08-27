#!/usr/bin/env bash
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
protocol_root="$repository_root/docs/protocol"

required_files=(
  "$repository_root/docs/adr/0004-canonical-fact-nostr-carriage.md"
  "$protocol_root/canonical-fact-v1.md"
  "$protocol_root/remote-control-v1.md"
  "$protocol_root/payload-mapping-v1.md"
  "$protocol_root/trust-transitions.md"
  "$protocol_root/vectors/canonical-installation-v1.json"
  "$protocol_root/vectors/remote-command-v1.json"
  "$protocol_root/vectors/adversarial-v1.json"
)

for required_file in "${required_files[@]}"; do
  test -s "$required_file"
done

for vector in "$protocol_root"/vectors/*.json; do
  jq -e . "$vector" >/dev/null
done

positive_vectors=(
  "$protocol_root/vectors/canonical-installation-v1.json"
  "$protocol_root/vectors/remote-command-v1.json"
)

for vector in "${positive_vectors[@]}"; do
  jq -e '
    .content_bytes == .event.content and
    .event_id == .event.id and
    .public_key == .event.pubkey and
    .signature == .event.sig and
    .event.kind == 6000 and
    .event.tags == [] and
    (.event.id | test("^[0-9a-f]{64}$")) and
    (.event.pubkey | test("^[0-9a-f]{64}$")) and
    (.event.sig | test("^[0-9a-f]{128}$"))
  ' "$vector" >/dev/null

  recorded_preimage=$(jq -jr '.event_preimage_bytes' "$vector")
  reconstructed_preimage=$(jq -cj '[0,.event.pubkey,.event.created_at,.event.kind,.event.tags,.event.content]' "$vector")
  test "$recorded_preimage" = "$reconstructed_preimage"

  calculated_id=$(jq -jr '.event_preimage_bytes' "$vector" | shasum -a 256 | cut -d' ' -f1)
  recorded_id=$(jq -r '.event_id' "$vector")
  test "$calculated_id" = "$recorded_id"

  canonical_content=$(jq -jr '.content_bytes' "$vector" | jq -cj .)
  recorded_content=$(jq -jr '.content_bytes' "$vector")
  test "$canonical_content" = "$recorded_content"
done

while IFS= read -r linked_path; do
  test -e "$linked_path"
done < <(
  perl -ne '
    while (/\[[^]]+\]\(([^)#]+)(?:#[^)]+)?\)/g) {
      my $target = $1;
      next if $target =~ m{^[a-z]+://};
      use File::Spec;
      my $directory = $ARGV;
      $directory =~ s{/[^/]+$}{};
      print File::Spec->rel2abs($target, $directory), "\n";
    }
  ' "${required_files[@]:0:5}"
)

cargo test -p hq-protocol --test spec_consistency
