package nostrwire

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/event"
)

const (
	wireInstallationA = "0198c7ec-73b0-7cc3-a5f7-e31c77140d01"
	wireInstallationB = "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	wireMailboxA      = "0198c7ec-73b0-7cc3-a5f7-e31c77140d11"
	wireMailboxB      = "00000000-0000-7000-8000-000000000000"
)

func TestNIP44OfficialVector(t *testing.T) {
	one := event.MustSecretKeyFromHex("1")
	two := event.MustSecretKeyFromHex("2")
	key, err := ConversationKey(one, two.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	if got := strings.ToLower(stringHex(key[:])); got != "c41c775356fd92eadc63ff5a0dc1da211b268cbea22316767095b2871ea1412d" {
		t.Fatalf("conversation key = %s", got)
	}
	nonce := make([]byte, 32)
	nonce[31] = 1
	payload, err := EncryptNIP44("a", key, nonce)
	if err != nil {
		t.Fatal(err)
	}
	const want = "AgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABee0G5VSK0/9YypIObAtDKfYEAjD35uVkHyB0F4DwrcNaCXlCWZKaArsGrY6M9wnuTMxWfp1RTN9Xga8no+kF5Vsb"
	if payload != want {
		t.Fatalf("payload differs from official vector\n%s", payload)
	}
	otherKey, err := ConversationKey(two, one.PublicKeyHex())
	if err != nil || otherKey != key {
		t.Fatalf("swapped conversation key differs: %v", err)
	}
	plaintext, err := DecryptNIP44(payload, otherKey)
	if err != nil || plaintext != "a" {
		t.Fatalf("decrypt = %q, %v", plaintext, err)
	}
	corrupt := payload[:len(payload)-2] + "AA"
	if _, err := DecryptNIP44(corrupt, key); err == nil {
		t.Fatal("bad MAC succeeded")
	}
}

func TestGiftWrapRoundTripAndFreshWrapperKeys(t *testing.T) {
	senderSecret := event.MustSecretKeyFromHex("11")
	recipientSecret := event.MustSecretKeyFromHex("22")
	canonical := peerCanonical(t, senderSecret)
	now := time.Unix(1_800_000_000, 0).UTC()
	sender := New(senderSecret, nil, func() time.Time { return now })
	first, err := sender.Wrap(canonical, recipientSecret.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	second, err := sender.Wrap(canonical, recipientSecret.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	if first.EventID == second.EventID || first.EphemeralKey == second.EphemeralKey {
		t.Fatal("gift wraps reused an event ID or ephemeral key")
	}
	recipient := New(recipientSecret, nil, func() time.Time { return now })
	got, err := recipient.Unwrap(first.ExactWire)
	if err != nil {
		t.Fatal(err)
	}
	if got.CanonicalEvent.ID() != canonical.ID() || string(got.CanonicalEvent.Wire) != string(canonical.Wire) || got.Seal.PubKey != senderSecret.PublicKeyHex() {
		t.Fatalf("unwrapped event = %#v", got)
	}
	if first.EventID != got.Outer.ID || first.EphemeralKey != got.Outer.PubKey {
		t.Fatal("wrapper metadata changed")
	}
}

func TestGiftWrapProtocolFixture(t *testing.T) {
	sender := event.MustSecretKeyFromHex("11")
	recipient := event.MustSecretKeyFromHex("22")
	reader := &sequenceReader{next: 1}
	wrapped, err := New(sender, reader, func() time.Time { return time.Unix(1_800_000_000, 0) }).Wrap(peerCanonical(t, sender), recipient.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	const wantID = "23ed163a939f1e224c32db406445eae4012aab4f8efb92921bdc68f1290eaec5"
	if wrapped.EventID != wantID {
		t.Fatalf("fixture ID = %s\nephemeral = %s", wrapped.EventID, wrapped.EphemeralKey)
	}
	const wantWireSHA256 = "a3f5384d36770275b6890409f62390984dcb8e14784e574d6601124c90e7b86d"
	if got := fmt.Sprintf("%x", sha256.Sum256(wrapped.ExactWire)); got != wantWireSHA256 {
		t.Fatalf("fixture wire SHA-256 = %s", got)
	}
}

func TestGiftWrapRejectsOuterFaultsBeforeDecrypt(t *testing.T) {
	senderSecret := event.MustSecretKeyFromHex("11")
	recipientSecret := event.MustSecretKeyFromHex("22")
	wrapped, err := New(senderSecret, bytes.NewReader(bytes.Repeat([]byte{8}, 512)), nil).Wrap(peerCanonical(t, senderSecret), recipientSecret.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	var outer event.NostrEvent
	if err := json.Unmarshal(wrapped.ExactWire, &outer); err != nil {
		t.Fatal(err)
	}
	outer.Content += "x"
	tampered, _ := json.Marshal(outer)
	if _, err := New(recipientSecret, nil, nil).Unwrap(tampered); err == nil || !strings.Contains(err.Error(), "signature") {
		t.Fatalf("tampered outer error = %v", err)
	}
	wrong := event.MustSecretKeyFromHex("33")
	if _, err := New(wrong, nil, nil).Unwrap(wrapped.ExactWire); err == nil || !strings.Contains(err.Error(), "recipient") {
		t.Fatalf("wrong recipient error = %v", err)
	}
	if _, err := New(recipientSecret, nil, nil).Unwrap(make([]byte, MaxGiftWrapBytes+1)); err == nil || !strings.Contains(err.Error(), "size") {
		t.Fatalf("oversize error = %v", err)
	}
	if _, err := New(recipientSecret, nil, nil).Unwrap([]byte("{")); err == nil || !strings.Contains(err.Error(), "malformed JSON") {
		t.Fatalf("malformed JSON error = %v", err)
	}
}

func TestGiftWrapRejectsSealCanonicalSignerMismatch(t *testing.T) {
	sealSecret := event.MustSecretKeyFromHex("11")
	canonicalSecret := event.MustSecretKeyFromHex("12")
	recipientSecret := event.MustSecretKeyFromHex("22")
	canonical := peerCanonical(t, canonicalSecret)
	codec := New(sealSecret, bytes.NewReader(bytes.Repeat([]byte{9}, 512)), func() time.Time { return time.Unix(1_800_000_000, 0) })
	envelope, _ := json.Marshal(Envelope{Schema: 1, Type: "hq.canonical", OriginInstallationID: canonical.Content.InstallationID, CanonicalEventID: canonical.ID(), CanonicalEvent: canonical.Wire})
	rumor := event.NostrEvent{PubKey: sealSecret.PublicKeyHex(), CreatedAt: canonical.Nostr.CreatedAt, Kind: KindHQRumor, Tags: [][]string{{"p", recipientSecret.PublicKeyHex()}}, Content: string(envelope)}
	rumor.ID, _ = rumor.ComputedID()
	rumorWire, _ := json.Marshal(rumor)
	sealCipher, err := codec.encrypt(string(rumorWire), sealSecret, recipientSecret.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	seal := event.NostrEvent{CreatedAt: codec.randomPast(), Kind: KindSeal, Tags: [][]string{}, Content: sealCipher}
	if err := seal.Sign(sealSecret); err != nil {
		t.Fatal(err)
	}
	sealWire, _ := json.Marshal(seal)
	ephemeral := event.MustSecretKeyFromHex("13")
	outerCipher, err := codec.encrypt(string(sealWire), ephemeral, recipientSecret.PublicKeyHex())
	if err != nil {
		t.Fatal(err)
	}
	outer := event.NostrEvent{CreatedAt: codec.randomPast(), Kind: KindGiftWrap, Tags: [][]string{{"p", recipientSecret.PublicKeyHex()}}, Content: outerCipher}
	if err := outer.Sign(ephemeral); err != nil {
		t.Fatal(err)
	}
	wire, _ := json.Marshal(outer)
	if _, err := New(recipientSecret, nil, nil).Unwrap(wire); err == nil || !strings.Contains(err.Error(), "identities do not match") {
		t.Fatalf("signer mismatch error = %v", err)
	}
}

func TestAuthEvent(t *testing.T) {
	secret := event.MustSecretKeyFromHex("44")
	auth, err := New(secret, nil, func() time.Time { return time.Unix(1_800_000_000, 0) }).AuthEvent("wss://relay.example", "challenge")
	if err != nil {
		t.Fatal(err)
	}
	if auth.Kind != KindClientAuth || !auth.VerifySignature() || len(auth.Tags) != 2 || auth.Tags[1][1] != "challenge" {
		t.Fatalf("auth event = %#v", auth)
	}
}

func peerCanonical(t *testing.T, secret event.SecretKey) event.SignedEvent {
	t.Helper()
	payload, err := event.MarshalPayload(event.TextPayload{MessageID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d70", Body: "hello", Context: &event.RepositoryContext{Directory: "/repo"}})
	if err != nil {
		t.Fatal(err)
	}
	content := event.Content{Type: event.TypeMessage, InstallationID: wireInstallationA, Sender: &event.MailboxAddress{InstallationID: wireInstallationA, MailboxID: wireMailboxA}, Recipient: &event.MailboxAddress{InstallationID: wireInstallationB, MailboxID: wireMailboxB}, Scope: event.ScopePeerAddressed, Payload: payload}
	signed, err := event.Sign(content, time.Unix(1_800_000_000, 0), secret)
	if err != nil {
		t.Fatal(err)
	}
	return signed
}

func stringHex(raw []byte) string {
	const alphabet = "0123456789abcdef"
	encoded := make([]byte, len(raw)*2)
	for index, value := range raw {
		encoded[index*2] = alphabet[value>>4]
		encoded[index*2+1] = alphabet[value&15]
	}
	return string(encoded)
}

type sequenceReader struct{ next byte }

func (r *sequenceReader) Read(target []byte) (int, error) {
	for index := range target {
		target[index] = r.next
		r.next++
	}
	return len(target), nil
}
