// Package event defines HQ's signed canonical event format.
package event

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/btcsuite/btcd/btcec/v2"
	"github.com/btcsuite/btcd/btcec/v2/schnorr"
)

const (
	// Kind is HQ's provisional regular Nostr event kind.
	Kind uint16 = 7281

	SchemaVersion  = 1
	MaxWireBytes   = 64 << 10
	MaxBodyBytes   = 32 << 10
	MaxDetailBytes = 16 << 10
	MaxParents     = 64
)

type Type string

const (
	TypeInstallationCreate Type = "installation.create"
	TypeMailboxCreate      Type = "mailbox.create"
	TypeMailboxBind        Type = "mailbox.bind"
	TypeMailboxContext     Type = "mailbox.context"
	TypeQuestion           Type = "question"
	TypeAnswer             Type = "answer"
	TypeMessage            Type = "message"
	TypeThreadCancel       Type = "thread.cancel"
	TypeMessageArchive     Type = "message.archive"
	TypeMessageRestore     Type = "message.restore"
	TypeMessageReject      Type = "message.reject"
	TypePeerTrust          Type = "peer.trust"
	TypePeerDistrust       Type = "peer.distrust"
	TypeMailboxShare       Type = "mailbox.share"
	TypeMailboxShareRevoke Type = "mailbox.share.revoke"
	TypeHumanAccountCreate Type = "human.account.create"
	TypeHumanAccountSelect Type = "human.account.select"
	TypeHumanDeviceGrant   Type = "human.device.grant"
	TypeHumanDeviceAccept  Type = "human.device.accept"
	TypeHumanDeviceRevoke  Type = "human.device.revoke"
)

type Scope string

const (
	ScopeInstallationPrivate Scope = "installation-private"
	ScopePeerAddressed       Scope = "peer-addressed"
	ScopeAccountAddressed    Scope = "account-addressed"
	ScopePublic              Scope = "public"
)

type ProjectionStatus string

const (
	StatusProjected    ProjectionStatus = "projected"
	StatusUnresolved   ProjectionStatus = "unresolved"
	StatusUnsupported  ProjectionStatus = "unsupported"
	StatusInvalid      ProjectionStatus = "invalid"
	StatusUnauthorized ProjectionStatus = "unauthorized"
)

type MailboxAddress struct {
	InstallationID string `json:"installation_id"`
	MailboxID      string `json:"mailbox_id"`
}

type Origin struct {
	InstallationID string `json:"installation_id"`
	EventID        string `json:"event_id"`
}

type Audience struct {
	HumanAccountID string `json:"human_account_id"`
}

// Content is the JSON document signed inside the Nostr event.
type Content struct {
	Schema         int             `json:"schema"`
	Type           Type            `json:"type"`
	InstallationID string          `json:"installation_id"`
	SignerKeyID    string          `json:"signer_key_id"`
	Sender         *MailboxAddress `json:"sender,omitempty"`
	Recipient      *MailboxAddress `json:"recipient,omitempty"`
	Audience       *Audience       `json:"audience,omitempty"`
	ThreadID       string          `json:"thread_id,omitempty"`
	Parents        []string        `json:"parents"`
	Scope          Scope           `json:"scope"`
	Origin         *Origin         `json:"origin,omitempty"`
	Payload        json.RawMessage `json:"payload"`
}

type TextPayload struct {
	MessageID  string             `json:"message_id,omitempty"`
	Body       string             `json:"body"`
	Details    string             `json:"details,omitempty"`
	Context    *RepositoryContext `json:"context,omitempty"`
	ActorLabel string             `json:"actor_label,omitempty"`
}

type RepositoryContext struct {
	Directory      string `json:"directory"`
	GitCommonDir   string `json:"git_common_dir,omitempty"`
	RemoteIdentity string `json:"remote_identity,omitempty"`
	Worktree       string `json:"worktree,omitempty"`
	Branch         string `json:"branch,omitempty"`
}

type InstallationPayload struct {
	Label string `json:"label,omitempty"`
}

type MailboxPayload struct {
	MailboxID string `json:"mailbox_id"`
	Kind      string `json:"kind"`
	Label     string `json:"label,omitempty"`
}

type MailboxBindingPayload struct {
	MailboxID         string `json:"mailbox_id"`
	Harness           string `json:"harness"`
	ExternalSessionID string `json:"external_session_id"`
}

type MailboxContextPayload struct {
	MailboxID string            `json:"mailbox_id"`
	Context   RepositoryContext `json:"context"`
}

type TargetPayload struct {
	TargetEventID string `json:"target_event_id"`
	Reason        string `json:"reason,omitempty"`
}

type PeerPayload struct {
	InstallationID string   `json:"installation_id"`
	SignerKeyID    string   `json:"signer_key_id,omitempty"`
	Name           string   `json:"name,omitempty"`
	Relays         []string `json:"relays,omitempty"`
}

type MailboxSharePayload struct {
	MailboxID          string `json:"mailbox_id"`
	PeerInstallationID string `json:"peer_installation_id"`
}

// HumanAccountPayload defines one logical human account and its creator.
type HumanAccountPayload struct {
	AccountID             string `json:"account_id"`
	CreatorInstallationID string `json:"creator_installation_id"`
	CreatorSignerKeyID    string `json:"creator_signer_key_id"`
	Label                 string `json:"label"`
}

// HumanAccountSelectionPayload selects the account used by one installation.
type HumanAccountSelectionPayload struct {
	AccountID string `json:"account_id"`
}

// HumanDevicePayload binds an installation key and display data to an account.
type HumanDevicePayload struct {
	AccountID             string   `json:"account_id"`
	CreatorInstallationID string   `json:"creator_installation_id"`
	CreatorSignerKeyID    string   `json:"creator_signer_key_id"`
	InstallationID        string   `json:"installation_id"`
	SignerKeyID           string   `json:"signer_key_id"`
	Label                 string   `json:"label"`
	Relays                []string `json:"relays"`
	CreatorRelays         []string `json:"creator_relays"`
}

type SecretKey [32]byte

func SecretKeyFromHex(value string) (SecretKey, error) {
	var secret SecretKey
	if len(value) < 64 {
		value = strings.Repeat("0", 64-len(value)) + value
	}
	if len(value) != 64 {
		return secret, errors.New("secret key must be at most 32-byte hex")
	}
	if _, err := hex.Decode(secret[:], []byte(value)); err != nil {
		return secret, fmt.Errorf("decode secret key: %w", err)
	}
	var scalar btcec.ModNScalar
	if overflow := scalar.SetByteSlice(secret[:]); overflow || scalar.IsZero() {
		return SecretKey{}, errors.New("secret key is outside the secp256k1 scalar range")
	}
	return secret, nil
}

func MustSecretKeyFromHex(value string) SecretKey {
	secret, err := SecretKeyFromHex(value)
	if err != nil {
		panic(err)
	}
	return secret
}

func (s SecretKey) PublicKeyHex() string {
	_, public := btcec.PrivKeyFromBytes(s[:])
	return hex.EncodeToString(public.SerializeCompressed()[1:])
}

// NostrEvent is the NIP-01 envelope used for canonical HQ events.
type NostrEvent struct {
	ID        string     `json:"id"`
	PubKey    string     `json:"pubkey"`
	CreatedAt int64      `json:"created_at"`
	Kind      uint16     `json:"kind"`
	Tags      [][]string `json:"tags"`
	Content   string     `json:"content"`
	Sig       string     `json:"sig"`
}

func (e NostrEvent) Serialize() ([]byte, error) {
	var buffer bytes.Buffer
	encoder := json.NewEncoder(&buffer)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode([]any{0, e.PubKey, e.CreatedAt, e.Kind, e.Tags, e.Content}); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buffer.Bytes(), []byte{'\n'}), nil
}

func (e NostrEvent) ComputedID() (string, error) {
	serialized, err := e.Serialize()
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(serialized)
	return hex.EncodeToString(sum[:]), nil
}

func (e NostrEvent) CheckID() bool {
	computed, err := e.ComputedID()
	return err == nil && computed == e.ID
}

func (e NostrEvent) VerifySignature() bool {
	if !e.CheckID() {
		return false
	}
	public, err := hex.DecodeString(e.PubKey)
	if err != nil {
		return false
	}
	parsedPublic, err := schnorr.ParsePubKey(public)
	if err != nil {
		return false
	}
	signature, err := hex.DecodeString(e.Sig)
	if err != nil {
		return false
	}
	parsedSignature, err := schnorr.ParseSignature(signature)
	if err != nil {
		return false
	}
	id, err := hex.DecodeString(e.ID)
	return err == nil && parsedSignature.Verify(id, parsedPublic)
}

func (e *NostrEvent) Sign(secret SecretKey) error {
	e.PubKey = secret.PublicKeyHex()
	id, err := e.ComputedID()
	if err != nil {
		return err
	}
	e.ID = id
	idBytes, _ := hex.DecodeString(id)
	private, _ := btcec.PrivKeyFromBytes(secret[:])
	signature, err := schnorr.Sign(private, idBytes, schnorr.FastSign())
	if err != nil {
		return err
	}
	e.Sig = hex.EncodeToString(signature.Serialize())
	return nil
}

// SignedEvent retains both parsed fields and the exact wire bytes.
type SignedEvent struct {
	Wire    []byte
	Nostr   NostrEvent
	Content Content
}

func (e SignedEvent) ID() string { return e.Nostr.ID }

type Inspection struct {
	Event  SignedEvent
	Status ProjectionStatus
	Err    error
}

func MarshalPayload(value any) (json.RawMessage, error) {
	raw, err := json.Marshal(value)
	if err != nil {
		return nil, fmt.Errorf("marshal event payload: %w", err)
	}
	return raw, nil
}

// Sign creates a deterministic canonical HQ event for the given content.
func Sign(content Content, createdAt time.Time, secret SecretKey) (SignedEvent, error) {
	content.SignerKeyID = secret.PublicKeyHex()
	if content.Schema == 0 {
		content.Schema = SchemaVersion
	}
	if content.Parents == nil {
		content.Parents = []string{}
	}
	if len(content.Payload) == 0 {
		content.Payload = json.RawMessage(`{}`)
	}
	if status, err := validateContent(content, "", SchemaVersion); status != StatusProjected {
		return SignedEvent{}, err
	}
	rawContent, err := json.Marshal(content)
	if err != nil {
		return SignedEvent{}, fmt.Errorf("marshal event content: %w", err)
	}
	if !utf8.Valid(rawContent) {
		return SignedEvent{}, errors.New("event content is not valid UTF-8")
	}
	nostrEvent := NostrEvent{
		CreatedAt: createdAt.UTC().Unix(),
		Kind:      Kind,
		Tags:      [][]string{},
		Content:   string(rawContent),
	}
	if err := nostrEvent.Sign(secret); err != nil {
		return SignedEvent{}, fmt.Errorf("sign event: %w", err)
	}
	wire, err := json.Marshal(nostrEvent)
	if err != nil {
		return SignedEvent{}, fmt.Errorf("marshal signed event: %w", err)
	}
	inspection := Inspect(wire)
	if inspection.Status != StatusProjected {
		return SignedEvent{}, fmt.Errorf("inspect signed event: %w", inspection.Err)
	}
	return inspection.Event, nil
}

// Inspect verifies the Nostr event before decoding or validating HQ content.
func Inspect(raw []byte) Inspection {
	return InspectWithSchemas(raw, []int{SchemaVersion})
}

// InspectWithSchemas permits a reducer upgraded with a compatible schema
// decoder to re-evaluate events retained by an older HQ release.
func InspectWithSchemas(raw []byte, schemas []int) Inspection {
	result := Inspection{Status: StatusInvalid}
	if len(raw) == 0 {
		result.Err = errors.New("event is empty")
		return result
	}
	if len(raw) > MaxWireBytes {
		result.Err = fmt.Errorf("event is %d bytes; limit is %d", len(raw), MaxWireBytes)
		return result
	}
	if !utf8.Valid(raw) {
		result.Err = errors.New("event wire data is not valid UTF-8")
		return result
	}
	var nostrEvent NostrEvent
	if err := decodeStrict(raw, &nostrEvent); err != nil {
		result.Err = fmt.Errorf("decode Nostr event: %w", err)
		return result
	}
	result.Event = SignedEvent{Wire: bytes.Clone(raw), Nostr: nostrEvent}
	if !nostrEvent.CheckID() {
		result.Err = errors.New("event ID does not match its signed content")
		return result
	}
	if !nostrEvent.VerifySignature() {
		result.Err = errors.New("event signature is invalid")
		return result
	}
	if nostrEvent.Kind != Kind {
		result.Status = StatusUnsupported
		result.Err = fmt.Errorf("unsupported Nostr kind %d", nostrEvent.Kind)
		return result
	}
	if len(nostrEvent.Tags) != 0 {
		result.Err = errors.New("HQ schema 1 events must have an empty tags array")
		return result
	}
	var header struct {
		Schema int `json:"schema"`
	}
	if err := json.Unmarshal([]byte(nostrEvent.Content), &header); err != nil {
		result.Err = fmt.Errorf("decode event header: %w", err)
		return result
	}
	if !containsSchema(schemas, header.Schema) {
		result.Status = StatusUnsupported
		result.Err = fmt.Errorf("unsupported HQ schema %d", header.Schema)
		return result
	}
	var content Content
	if err := decodeStrict([]byte(nostrEvent.Content), &content); err != nil {
		result.Err = fmt.Errorf("decode HQ event: %w", err)
		return result
	}
	result.Event.Content = content
	status, err := validateContent(content, nostrEvent.PubKey, header.Schema)
	result.Status, result.Err = status, err
	return result
}

func containsSchema(schemas []int, schema int) bool {
	for _, candidate := range schemas {
		if candidate == schema {
			return true
		}
	}
	return false
}

func decodeStrict(raw []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return err
	}
	if decoder.More() {
		return errors.New("extra JSON value")
	}
	var extra any
	if err := decoder.Decode(&extra); !errors.Is(err, io.EOF) {
		if err == nil {
			return errors.New("extra JSON value")
		}
		return err
	}
	return nil
}
