package nostrwire

import (
	"bytes"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/big"
	"strings"
	"time"

	nostr "fiatjaf.com/nostr"
	"fiatjaf.com/nostr/nip44"
	"github.com/wbbradley/hq/internal/event"
	"golang.org/x/crypto/hkdf"
)

const (
	KindHQRumor      uint16 = 7282
	KindSeal         uint16 = 13
	KindGiftWrap     uint16 = 1059
	KindClientAuth   uint16 = 22242
	MaxGiftWrapBytes        = 256 << 10
	maxPast                 = 2 * 24 * time.Hour
)

type Envelope struct {
	Schema               int             `json:"schema"`
	Type                 string          `json:"type"`
	OriginInstallationID string          `json:"origin_installation_id"`
	CanonicalEventID     string          `json:"canonical_event_id"`
	CanonicalEvent       json.RawMessage `json:"canonical_event"`
}

type Wrapped struct {
	EventID       string
	EphemeralKey  string
	RecipientKey  string
	ExactWire     []byte
	CanonicalID   string
	CanonicalWire []byte
}

type Unwrapped struct {
	Outer          event.NostrEvent
	Seal           event.NostrEvent
	Rumor          event.NostrEvent
	Envelope       Envelope
	CanonicalEvent event.SignedEvent
}

type Codec struct {
	secret event.SecretKey
	random io.Reader
	now    func() time.Time
}

func New(secret event.SecretKey, random io.Reader, now func() time.Time) *Codec {
	if random == nil {
		random = rand.Reader
	}
	if now == nil {
		now = time.Now
	}
	return &Codec{secret: secret, random: random, now: now}
}

func (c *Codec) PublicKey() string { return c.secret.PublicKeyHex() }

func (c *Codec) Wrap(canonical event.SignedEvent, recipientPublicKey string) (Wrapped, error) {
	if inspection := event.Inspect(canonical.Wire); inspection.Status == event.StatusInvalid || inspection.Event.ID() != canonical.ID() {
		return Wrapped{}, errors.New("canonical event is invalid")
	}
	if canonical.Nostr.PubKey != c.PublicKey() {
		return Wrapped{}, errors.New("canonical event was not signed by this installation")
	}
	if _, err := nostr.PubKeyFromHex(recipientPublicKey); err != nil {
		return Wrapped{}, errors.New("recipient public key is invalid")
	}
	envelopeBytes, err := json.Marshal(Envelope{Schema: 1, Type: "hq.canonical", OriginInstallationID: canonical.Content.InstallationID, CanonicalEventID: canonical.ID(), CanonicalEvent: canonical.Wire})
	if err != nil {
		return Wrapped{}, err
	}
	rumor := event.NostrEvent{PubKey: c.PublicKey(), CreatedAt: canonical.Nostr.CreatedAt, Kind: KindHQRumor, Tags: [][]string{{"p", recipientPublicKey}}, Content: string(envelopeBytes)}
	rumor.ID, err = rumor.ComputedID()
	if err != nil {
		return Wrapped{}, err
	}
	rumorWire, err := json.Marshal(rumor)
	if err != nil {
		return Wrapped{}, err
	}
	sealCipher, err := c.encrypt(string(rumorWire), c.secret, recipientPublicKey)
	if err != nil {
		return Wrapped{}, fmt.Errorf("encrypt rumor: %w", err)
	}
	seal := event.NostrEvent{CreatedAt: c.randomPast(), Kind: KindSeal, Tags: [][]string{}, Content: sealCipher}
	if err := seal.Sign(c.secret); err != nil {
		return Wrapped{}, err
	}
	sealWire, err := json.Marshal(seal)
	if err != nil {
		return Wrapped{}, err
	}
	ephemeral, err := randomSecret(c.random)
	if err != nil {
		return Wrapped{}, err
	}
	outerCipher, err := c.encrypt(string(sealWire), ephemeral, recipientPublicKey)
	if err != nil {
		return Wrapped{}, fmt.Errorf("encrypt seal: %w", err)
	}
	outer := event.NostrEvent{CreatedAt: c.randomPast(), Kind: KindGiftWrap, Tags: [][]string{{"p", recipientPublicKey}}, Content: outerCipher}
	if err := outer.Sign(ephemeral); err != nil {
		return Wrapped{}, err
	}
	wire, err := json.Marshal(outer)
	if err != nil {
		return Wrapped{}, err
	}
	if len(wire) > MaxGiftWrapBytes {
		return Wrapped{}, fmt.Errorf("gift wrap is %d bytes; limit is %d", len(wire), MaxGiftWrapBytes)
	}
	return Wrapped{EventID: outer.ID, EphemeralKey: outer.PubKey, RecipientKey: recipientPublicKey, ExactWire: wire, CanonicalID: canonical.ID(), CanonicalWire: canonical.Wire}, nil
}

func (c *Codec) Unwrap(raw []byte) (Unwrapped, error) {
	if len(raw) == 0 || len(raw) > MaxGiftWrapBytes {
		return Unwrapped{}, fmt.Errorf("gift wrap size is outside the 1..%d byte limit", MaxGiftWrapBytes)
	}
	var outer event.NostrEvent
	if err := decodeStrict(raw, &outer); err != nil {
		return Unwrapped{}, errors.New("gift wrap is malformed JSON")
	}
	if !outer.VerifySignature() {
		return Unwrapped{}, errors.New("gift wrap signature is invalid")
	}
	if outer.Kind != KindGiftWrap {
		return Unwrapped{}, fmt.Errorf("gift wrap kind is %d; want %d", outer.Kind, KindGiftWrap)
	}
	if !oneRecipientTag(outer.Tags, c.PublicKey()) {
		return Unwrapped{}, errors.New("gift wrap recipient is not this installation")
	}
	sealJSON, err := c.decrypt(outer.Content, c.secret, outer.PubKey)
	if err != nil {
		return Unwrapped{}, fmt.Errorf("decrypt gift wrap: %w", err)
	}
	var seal event.NostrEvent
	if err := decodeStrict([]byte(sealJSON), &seal); err != nil {
		return Unwrapped{}, errors.New("seal is malformed JSON")
	}
	if !seal.VerifySignature() {
		return Unwrapped{}, errors.New("seal signature is invalid")
	}
	if seal.Kind != KindSeal || len(seal.Tags) != 0 {
		return Unwrapped{}, errors.New("seal kind or tags are invalid")
	}
	rumorJSON, err := c.decrypt(seal.Content, c.secret, seal.PubKey)
	if err != nil {
		return Unwrapped{}, fmt.Errorf("decrypt seal: %w", err)
	}
	var rumor event.NostrEvent
	if err := decodeStrict([]byte(rumorJSON), &rumor); err != nil {
		return Unwrapped{}, errors.New("rumor is malformed JSON")
	}
	if rumor.Sig != "" || rumor.PubKey != seal.PubKey || rumor.Kind != KindHQRumor || !rumor.CheckID() || !oneRecipientTag(rumor.Tags, c.PublicKey()) {
		return Unwrapped{}, errors.New("rumor identity, kind, recipient, or ID is invalid")
	}
	var envelope Envelope
	if err := decodeStrict([]byte(rumor.Content), &envelope); err != nil {
		return Unwrapped{}, errors.New("HQ rumor envelope is invalid")
	}
	if envelope.Schema != 1 || envelope.Type != "hq.canonical" || len(envelope.CanonicalEvent) == 0 {
		return Unwrapped{}, errors.New("HQ rumor envelope version or type is unsupported")
	}
	inspection := event.Inspect(envelope.CanonicalEvent)
	if inspection.Status == event.StatusInvalid {
		return Unwrapped{}, fmt.Errorf("canonical event is invalid: %w", inspection.Err)
	}
	canonical := inspection.Event
	if canonical.ID() != envelope.CanonicalEventID || canonical.Nostr.PubKey != seal.PubKey || canonical.Content.InstallationID != envelope.OriginInstallationID {
		return Unwrapped{}, errors.New("seal, origin, and canonical event identities do not match")
	}
	switch canonical.Content.Scope {
	case event.ScopePeerAddressed:
		if canonical.Content.Recipient == nil || canonical.Content.Recipient.InstallationID == canonical.Content.InstallationID {
			return Unwrapped{}, errors.New("canonical event has no remote peer recipient")
		}
	case event.ScopeAccountAddressed:
		if canonical.Content.Audience == nil {
			return Unwrapped{}, errors.New("canonical account event has no audience")
		}
	default:
		return Unwrapped{}, errors.New("canonical event cannot be relayed")
	}
	return Unwrapped{Outer: outer, Seal: seal, Rumor: rumor, Envelope: envelope, CanonicalEvent: canonical}, nil
}

func (c *Codec) AuthEvent(relayURL, challenge string) (event.NostrEvent, error) {
	if relayURL == "" || challenge == "" {
		return event.NostrEvent{}, errors.New("relay URL and challenge are required")
	}
	auth := event.NostrEvent{CreatedAt: c.now().UTC().Unix(), Kind: KindClientAuth, Tags: [][]string{{"relay", relayURL}, {"challenge", challenge}}, Content: ""}
	if err := auth.Sign(c.secret); err != nil {
		return event.NostrEvent{}, err
	}
	return auth, nil
}

func ConversationKey(secret event.SecretKey, publicKey string) ([32]byte, error) {
	public, err := nostr.PubKeyFromHex(publicKey)
	if err != nil {
		return [32]byte{}, err
	}
	return nip44.GenerateConversationKey(public, nostr.SecretKey(secret))
}

func EncryptNIP44(plaintext string, key [32]byte, nonce []byte) (string, error) {
	return nip44.Encrypt(plaintext, key, nip44.WithCustomNonce(nonce))
}

func DecryptNIP44(ciphertext string, key [32]byte) (string, error) {
	if len(ciphertext) > MaxGiftWrapBytes {
		return "", errors.New("NIP-44 payload is too large")
	}
	if len(ciphertext) < 132 || strings.HasPrefix(ciphertext, "#") {
		return "", errors.New("NIP-44 payload has an invalid size or version")
	}
	raw, err := base64.StdEncoding.DecodeString(ciphertext)
	if err != nil || len(raw) < 99 || raw[0] != 2 {
		return "", errors.New("NIP-44 payload is malformed or unsupported")
	}
	nonce, encrypted, givenMAC := raw[1:33], raw[33:len(raw)-32], raw[len(raw)-32:]
	keys := make([]byte, 76)
	if _, err := io.ReadFull(hkdf.Expand(sha256.New, key[:], nonce), keys); err != nil {
		return "", err
	}
	defer clear(keys)
	mac := hmac.New(sha256.New, keys[44:76])
	_, _ = mac.Write(nonce)
	_, _ = mac.Write(encrypted)
	if !hmac.Equal(givenMAC, mac.Sum(nil)) {
		return "", errors.New("NIP-44 MAC is invalid")
	}
	return nip44.Decrypt(ciphertext, key)
}

func (c *Codec) encrypt(plaintext string, secret event.SecretKey, public string) (string, error) {
	key, err := ConversationKey(secret, public)
	if err != nil {
		return "", err
	}
	nonce := make([]byte, 32)
	if _, err := io.ReadFull(c.random, nonce); err != nil {
		return "", err
	}
	defer clear(nonce)
	return EncryptNIP44(plaintext, key, nonce)
}

func (c *Codec) decrypt(ciphertext string, secret event.SecretKey, public string) (string, error) {
	key, err := ConversationKey(secret, public)
	if err != nil {
		return "", err
	}
	return DecryptNIP44(ciphertext, key)
}

func (c *Codec) randomPast() int64 {
	limit := big.NewInt(int64(maxPast / time.Second))
	offset, err := rand.Int(c.random, limit)
	if err != nil {
		return c.now().UTC().Add(-maxPast).Unix()
	}
	return c.now().UTC().Add(-time.Duration(offset.Int64()) * time.Second).Unix()
}

func randomSecret(random io.Reader) (event.SecretKey, error) {
	for {
		var raw [32]byte
		if _, err := io.ReadFull(random, raw[:]); err != nil {
			return event.SecretKey{}, err
		}
		secret, err := event.SecretKeyFromHex(fmt.Sprintf("%x", raw[:]))
		clear(raw[:])
		if err == nil {
			return secret, nil
		}
	}
}

func oneRecipientTag(tags [][]string, recipient string) bool {
	return len(tags) == 1 && len(tags[0]) == 2 && tags[0][0] == "p" && tags[0][1] == recipient
}

func decodeStrict(raw []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return err
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		return errors.New("trailing JSON data")
	}
	return nil
}
