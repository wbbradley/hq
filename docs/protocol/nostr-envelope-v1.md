# Nostr envelope v1

Status: Normative for the Rust rewrite

This protocol moves one already-signed HQ canonical or remote-control event to one installation.
It provides confidential recipient-bound carriage, not application authority. Opening an envelope
returns the exact raw canonical event bytes; the ordinary canonical verification, binding, reducer,
and authorization path remains the only route into application state.

Envelope v1 is independent of canonical v1. Its schema number does not select, upgrade, or
reinterpret a canonical content version.

## Reviewed standards

This specification pins the rules reviewed on 2026-08-27:

- [NIP-44 at `24b2ae9`](https://github.com/nostr-protocol/nips/blob/24b2ae9fdfeb4e5c0d3be854df5977b81afe1983/44.md), including version 2's extended plaintext-length prefix;
- [NIP-59 at `24b2ae9`](https://github.com/nostr-protocol/nips/blob/24b2ae9fdfeb4e5c0d3be854df5977b81afe1983/59.md);
- [NIP-42 at `24b2ae9`](https://github.com/nostr-protocol/nips/blob/24b2ae9fdfeb4e5c0d3be854df5977b81afe1983/42.md); and
- the [published NIP-44 vectors at `671a1f0`](https://github.com/paulmillr/nip44/blob/671a1f04bcfacaf125b0db68adc45bc9ce0e763b/nip44.vectors.json).

A standards change does not silently change HQ envelope v1. It requires review, new vectors, and
either a compatible clarification here or a new envelope version.

## Keys and identities

An installation root key is one normalized BIP-340 key used to sign canonical events, sign seals,
open NIP-44 payloads, and authenticate relay connections. Its 32-byte x-only public key denotes the
even-y secp256k1 point. ECDH uses the corresponding BIP-340-normalized secret scalar.

The recipient is the exact 32-byte root public key supplied by a previously verified route or human
device binding. The transport layer does not discover keys and a `p` tag does not confer mailbox,
account, or project authority.

## Exact layers

All hexadecimal strings are 64 lowercase characters unless noted. Event IDs and signatures follow
NIP-01. HQ emits compact JSON; receivers accept any strict JSON spelling that has exactly the
members and value shapes below. Duplicate, missing, or unknown members are invalid.

### Schema-1 HQ content

The unsigned rumor's `content` is this compact object in the shown member order:

```json
{"schema":1,"type":"hq.canonical","origin_installation_id":"<hex32>","canonical_event_id":"<hex32>","canonical_event":<exact signed HQ event object>}
```

`canonical_event` is embedded as an object, not as an escaped JSON string. Its byte span must equal
the exact canonical event bytes after parsing. `canonical_event_id` must equal its verified NIP-01
ID. `origin_installation_id` must equal the canonical DTO author installation ID.

### Rumor

The rumor is an unsigned kind-7282 NIP-01 event object containing exactly `id`, `pubkey`,
`created_at`, `kind`, `tags`, and `content`; it has no `sig` member. Its author is the canonical
signer, its time is the canonical event's `created_at`, and its tags are exactly
`[["p","<recipient-root-key>"]]`. Its ID must match the NIP-01 preimage.

### Seal

The rumor's exact JSON is NIP-44-v2 encrypted from the sender root key to the recipient root key.
The ciphertext is the content of a signed kind-13 event. Seal tags are exactly empty. Its signer
must equal the rumor author and canonical signer.

### Gift wrap

The seal's exact JSON is NIP-44-v2 encrypted from a fresh one-use BIP-340 key to the recipient root
key. The ciphertext is the content of a signed, retained kind-1059 event. Its tags are exactly
`[["p","<recipient-root-key>"]]`. The signing key must not be used for another prepared wrapper.
HQ does not emit ephemeral kind 21059 because offline delivery is required.

## NIP-44 v2 profile

HQ implements the pinned NIP-44 v2 construction exactly: unhashed secp256k1 ECDH x-coordinate;
HKDF-extract SHA-256 with salt `nip44-v2`; HKDF-expand SHA-256 to a 32-byte ChaCha20 key, 12-byte
nonce, and 32-byte HMAC key; the specified power-of-two padding; ChaCha20 at counter zero;
HMAC-SHA256 over the 32-byte random nonce followed by ciphertext; and padded RFC 4648 base64.

Plaintexts of 1 through 65,535 bytes use the two-byte big-endian length. Larger plaintexts use two
zero bytes followed by a four-byte big-endian length. HQ's tighter transport bounds apply before
the theoretical NIP-44 maximum. A receiver validates the signed containing event before base64
decoding or decrypting, checks encoded and decoded lengths before allocation, checks the version,
verifies the MAC in constant time before decrypting, and validates the exact padding shape and UTF-8.
Derived keys and plaintext working buffers are zeroized when released.

## Preparation, persistence, and retries

Preparation is a one-time transition from a verified canonical record, recipient, current time,
and cryptographic randomness into a prepared retry lineage. It independently samples:

- one 32-byte nonce for the seal encryption;
- one seal timestamp offset;
- one valid one-use secret key;
- one 32-byte nonce for gift-wrap encryption;
- BIP-340 auxiliary randomness for both signatures; and
- one gift-wrap timestamp offset.

Seal and gift-wrap timestamps are independently selected from the inclusive interval
`[now - 172800, now]`. They must never be in the future. The canonical and rumor timestamps are not
randomized.

Before the first network write, the caller must durably commit the prepared lineage's exact wrapper
bytes and metadata: wrapper ID, one-use public key claim, recipient root key, canonical event ID,
canonical bytes digest, seal and gift-wrap timestamps, and exact byte length. The signed wrapper no
longer needs its one-use secret to be published, so HQ does not persist that secret. This narrows the
older Go-era policy and avoids retaining unnecessary decryption/signing material.

Every attempt in one lineage borrows the stored exact bytes. A retry must not re-encrypt, re-sign,
change a timestamp, or create a new wrapper. Durable reconstruction re-verifies the relay-visible
ID, one-use public key, recipient, timestamp, byte length, and wrapper digest. The canonical digest
and seal timestamp are preparation evidence committed atomically with those bytes; the sender
cannot reopen that inner layer after discarding the one-use secret. A uniqueness claim on the
one-use public key must reject its association with any different wrapper ID; claiming it again for
the same wrapper is idempotent.

Creating a second recipient wrapper for the same canonical event is a different lineage and must
use a fresh one-use key. An explicit later rewrap after permanent abandonment is also a new lineage,
never a retry.

## Opening and trust transitions

Opening processes attacker-controlled input in this order:

1. Enforce the complete gift-wrap byte bound and strict JSON object shape.
2. Recompute the outer ID; verify the BIP-340 signature, kind 1059, and exactly one local `p` tag.
3. NIP-44-decrypt the seal with the local root secret and outer one-use public key.
4. Strictly parse and verify the seal ID/signature, kind 13, and empty tags.
5. NIP-44-decrypt the rumor with the local root secret and seal signer.
6. Strictly parse the unsigned rumor; verify its ID, kind 7282, missing signature, author agreement,
   and exactly one local `p` tag.
7. Strictly parse schema-1 HQ content and retain the exact embedded canonical event byte span.
8. Run the ordinary canonical cryptographic and DTO verification transitions solely to check
   canonical ID, origin installation ID, and signer agreement.
9. Return only the exact raw canonical bytes and transport audit metadata. No relay, wrapper, seal,
   rumor, timestamp, or tag is supplied as domain authority.

The later common ingest path repeats ordinary canonical verification before consulting local signer
bindings and reducing. This deliberate re-verification keeps the relay adapter from manufacturing a
privileged canonical trust type.

## NIP-42 authentication

For a relay connection, HQ creates a signed ephemeral kind-22242 event with empty content and tags
exactly `[["relay", relay_url], ["challenge", challenge]]`. `created_at` is current Unix seconds.
The relay URL and challenge are copied exactly from the active connection after the bounds below;
they are neither persisted as facts nor used outside that connection. The root signer must be the
same key used by the installation identity and envelopes. The connection owner, not the envelope
codec, decides when a challenge is current and sends the `AUTH` frame.

## Bounds and failures

- Complete gift-wrap input: 1 through 262,144 bytes.
- Any NIP-44 base64 payload: at most 262,144 bytes before decoding.
- Any decrypted seal or rumor JSON: at most 196,608 bytes.
- Relay URL: 1 through 2,048 UTF-8 bytes; no control characters.
- Relay challenge: 1 through 1,024 UTF-8 bytes; no control characters.
- Quarantine evidence sample: at most 4,096 bytes.

Preparation must reject a canonical event whose resulting wrapper exceeds the complete bound.
Inbound checks are performed before proportional allocation wherever possible. Permanent failures
use a closed, redacted class such as size, malformed JSON, event identity, signature, recipient,
unsupported encryption, MAC, padding, envelope version, canonical verification, or identity
agreement. Errors do not contain ciphertext, plaintext, keys, URLs, challenges, or parser excerpts.

The durable relay package owns the bounded quarantine collection. It may retain the failure class,
receive time, byte length, SHA-256 digest, a verified outer ID when available, and at most the sample
limit of raw outer bytes. It must never retain decrypted seal/rumor/canonical plaintext or secret
material as quarantine evidence. Collection count/age eviction policy is store configuration, not
an envelope wire rule.

## Relay observations and security limits

A relay can observe the gift-wrap ID, one-use public key, recipient root public key, randomized
timestamp, wrapper size, client network address, and retry equality. A NIP-42 relay also sees the
installation root public key and connection timing. It cannot read the seal signer, origin
installation ID, canonical event, mailbox/account/project identifiers, causal links, or body without
a key compromise.

NIP-44 supplies confidentiality and integrity for these payloads but no forward secrecy,
post-compromise security, post-quantum security, or network-address privacy. Timestamp randomization
reduces correlation; it does not eliminate traffic analysis.
