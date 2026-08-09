package identity

import (
	"bytes"
	"strings"
	"testing"

	"github.com/btcsuite/btcd/btcutil/bech32"
	"github.com/wbbradley/hq/internal/event"
)

func TestNIP49OfficialVector(t *testing.T) {
	secret := event.MustSecretKeyFromHex("3501454135014541350145413501453fefb02227e449e57cf4d3a3ce05378683")
	const encoded = "ncryptsec1qgg9947rlpvqu76pj5ecreduf9jxhselq2nae2kghhvd5g7dgjtcxfqtd67p9m0w57lspw8gsq6yphnm8623nsl8xn9j4jdzz84zm3frztj3z7s35vpzmqf6ksu8r89qk5z2zxfmu5gv8th8wclt0h4p"
	got, err := DecryptNIP49(encoded, []byte("nostr"))
	if err != nil {
		t.Fatal(err)
	}
	if got != secret {
		t.Fatalf("decoded key differs from the official vector")
	}
	_, words, err := bech32.DecodeNoLimit(encoded)
	if err != nil {
		t.Fatal(err)
	}
	payload, err := bech32.ConvertBits(words, 5, 8, false)
	if err != nil {
		t.Fatal(err)
	}
	reencoded, err := EncryptNIP49(secret, []byte("nostr"), payload[1], bytes.NewReader(payload[2:42]))
	if err != nil || reencoded != encoded {
		t.Fatalf("encoded official vector differs: %v", err)
	}
}

func TestNIP49RoundTripAndWrongPassword(t *testing.T) {
	secret := event.MustSecretKeyFromHex("1")
	random := bytes.NewReader(bytes.Repeat([]byte{7}, 40))
	encoded, err := EncryptNIP49(secret, []byte("password"), 16, random)
	if err != nil {
		t.Fatal(err)
	}
	got, err := DecryptNIP49(encoded, []byte("password"))
	if err != nil || got != secret {
		t.Fatalf("round trip = %v, %v", got, err)
	}
	if _, err := DecryptNIP49(encoded, []byte("wrong")); err == nil || strings.Contains(err.Error(), secret.PublicKeyHex()) {
		t.Fatalf("wrong password error = %v", err)
	}
}

func TestNIP49NormalizesPassword(t *testing.T) {
	secret := event.MustSecretKeyFromHex("2")
	random := bytes.NewReader(bytes.Repeat([]byte{8}, 40))
	encoded, err := EncryptNIP49(secret, []byte("e\u0301"), 16, random)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := DecryptNIP49(encoded, []byte("é")); err != nil {
		t.Fatal(err)
	}
}
