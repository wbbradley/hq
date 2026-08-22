package event

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/google/uuid"
)

func validateContent(content Content, publicKey string, schema int) (ProjectionStatus, error) {
	if content.Schema != schema {
		return StatusUnsupported, fmt.Errorf("unsupported HQ schema %d", content.Schema)
	}
	if !knownType(content.Type) {
		return StatusUnsupported, fmt.Errorf("unsupported HQ event type %q", content.Type)
	}
	if err := validUUID("installation ID", content.InstallationID); err != nil {
		return StatusInvalid, err
	}
	if err := validHex("signer key ID", content.SignerKeyID, 32); err != nil {
		return StatusInvalid, err
	}
	if publicKey != "" && content.SignerKeyID != publicKey {
		return StatusInvalid, errors.New("signer key ID does not match the Nostr public key")
	}
	if content.Scope != ScopeInstallationPrivate && content.Scope != ScopePeerAddressed && content.Scope != ScopeAccountAddressed {
		if content.Scope == ScopePublic {
			return StatusInvalid, errors.New("public HQ events are reserved but disabled")
		}
		return StatusInvalid, fmt.Errorf("invalid distribution scope %q", content.Scope)
	}
	if content.Scope == ScopeAccountAddressed {
		if content.Audience == nil {
			return StatusInvalid, errors.New("account-addressed event needs a human account audience")
		}
		if err := validUUID("human account audience", content.Audience.HumanAccountID); err != nil {
			return StatusInvalid, err
		}
	} else if content.Audience != nil {
		return StatusInvalid, errors.New("only account-addressed events may name a human account audience")
	}
	if len(content.Parents) > MaxParents {
		return StatusInvalid, fmt.Errorf("event has %d parents; limit is %d", len(content.Parents), MaxParents)
	}
	seenParents := make(map[string]struct{}, len(content.Parents))
	for _, parent := range content.Parents {
		if err := validEventID("parent event ID", parent); err != nil {
			return StatusInvalid, err
		}
		if _, ok := seenParents[parent]; ok {
			return StatusInvalid, fmt.Errorf("duplicate parent event ID %s", parent)
		}
		seenParents[parent] = struct{}{}
	}
	if !slices.IsSorted(content.Parents) {
		return StatusInvalid, errors.New("parent event IDs must use lexical order")
	}
	if content.ThreadID != "" {
		if err := validEventID("thread ID", content.ThreadID); err != nil {
			return StatusInvalid, err
		}
	}
	if content.Origin != nil {
		if err := validUUID("origin installation ID", content.Origin.InstallationID); err != nil {
			return StatusInvalid, err
		}
		if err := validEventID("origin event ID", content.Origin.EventID); err != nil {
			return StatusInvalid, err
		}
	}

	switch content.Type {
	case TypeQuestion, TypeMessage:
		if content.ThreadID != "" || (content.Scope != ScopeAccountAddressed && len(content.Parents) != 0) || (content.Scope == ScopeAccountAddressed && len(content.Parents) == 0) {
			return StatusInvalid, errors.New("root question and message events must omit thread_id and account-addressed roots need membership parents")
		}
		if err := validateMessageAddresses(content, content.Type == TypeQuestion && content.Scope == ScopeAccountAddressed); err != nil {
			return StatusInvalid, err
		}
		if err := validateTextPayload(content.Payload); err != nil {
			return StatusInvalid, err
		}
	case TypeAnswer:
		if len(content.Parents) == 0 || content.ThreadID == "" {
			return StatusInvalid, errors.New("answer events need a thread ID and at least one parent")
		}
		if err := validateMessageAddresses(content, false); err != nil {
			return StatusInvalid, err
		}
		if err := validateTextPayload(content.Payload); err != nil {
			return StatusInvalid, err
		}
	case TypeThreadCancel:
		if len(content.Parents) == 0 || content.ThreadID == "" {
			return StatusInvalid, errors.New("thread cancellation needs a thread ID and parent")
		}
		if content.Sender == nil {
			return StatusInvalid, errors.New("thread cancellation needs a sender")
		}
		if err := validateAddress("sender", *content.Sender); err != nil {
			return StatusInvalid, err
		}
		if content.Sender.InstallationID != content.InstallationID {
			return StatusInvalid, errors.New("sender installation does not match event installation")
		}
		if content.Recipient != nil {
			return StatusInvalid, errors.New("thread cancellation must not name a recipient")
		}
		if err := validateTargetPayload(content.Payload, false); err != nil {
			return StatusInvalid, err
		}
	case TypeMessageArchive, TypeMessageRestore, TypeMessageReject:
		if content.Scope != ScopeInstallationPrivate && content.Scope != ScopeAccountAddressed {
			return StatusInvalid, errors.New("message state event must be installation-private or account-addressed")
		}
		if len(content.Parents) == 0 {
			return StatusInvalid, errors.New("message state event needs a parent")
		}
		if content.Sender == nil || content.Recipient != nil {
			return StatusInvalid, errors.New("message state event needs one sender and no recipient")
		}
		if err := validateAddress("sender", *content.Sender); err != nil {
			return StatusInvalid, err
		}
		if content.Sender.InstallationID != content.InstallationID {
			return StatusInvalid, errors.New("sender installation does not match event installation")
		}
		if err := validateTargetPayload(content.Payload, true); err != nil {
			return StatusInvalid, err
		}
	case TypeInstallationCreate:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload InstallationPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
	case TypeMailboxCreate:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload MailboxPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("mailbox ID", payload.MailboxID); err != nil {
			return StatusInvalid, err
		}
		if payload.Kind != "human" && payload.Kind != "agent" {
			return StatusInvalid, fmt.Errorf("invalid mailbox kind %q", payload.Kind)
		}
	case TypeMailboxBind:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload MailboxBindingPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("mailbox ID", payload.MailboxID); err != nil {
			return StatusInvalid, err
		}
		if strings.TrimSpace(payload.Harness) == "" || strings.TrimSpace(payload.ExternalSessionID) == "" {
			return StatusInvalid, errors.New("mailbox binding needs a harness and external session ID")
		}
	case TypeMailboxContext:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload MailboxContextPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("mailbox ID", payload.MailboxID); err != nil {
			return StatusInvalid, err
		}
		if strings.TrimSpace(payload.Context.Directory) == "" {
			return StatusInvalid, errors.New("mailbox context needs a directory")
		}
	case TypePeerTrust, TypePeerDistrust:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload PeerPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("peer installation ID", payload.InstallationID); err != nil {
			return StatusInvalid, err
		}
		if content.Type == TypePeerTrust {
			if err := validHex("peer signer key ID", payload.SignerKeyID, 32); err != nil {
				return StatusInvalid, err
			}
			for _, relay := range payload.Relays {
				parsed, err := url.Parse(relay)
				if err != nil || (parsed.Scheme != "wss" && parsed.Scheme != "ws") || parsed.Host == "" {
					return StatusInvalid, fmt.Errorf("invalid peer relay %q", relay)
				}
			}
		}
	case TypeMailboxShare, TypeMailboxShareRevoke:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		var payload MailboxSharePayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("mailbox ID", payload.MailboxID); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("peer installation ID", payload.PeerInstallationID); err != nil {
			return StatusInvalid, err
		}
	case TypeHumanAccountCreate:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		if len(content.Parents) != 0 {
			return StatusInvalid, errors.New("human account creation must omit parents")
		}
		var payload HumanAccountPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validateHumanAccountPayload(payload); err != nil {
			return StatusInvalid, err
		}
		if payload.CreatorInstallationID != content.InstallationID || payload.CreatorSignerKeyID != content.SignerKeyID {
			return StatusInvalid, errors.New("human account creator does not match the event signer")
		}
	case TypeHumanAccountSelect:
		if err := validateControl(content); err != nil {
			return StatusInvalid, err
		}
		if len(content.Parents) == 0 {
			return StatusInvalid, errors.New("human account selection needs a membership parent")
		}
		var payload HumanAccountSelectionPayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validUUID("human account ID", payload.AccountID); err != nil {
			return StatusInvalid, err
		}
	case TypeHumanDeviceGrant, TypeHumanDeviceAccept, TypeHumanDeviceRevoke:
		if content.Scope != ScopeAccountAddressed || content.Audience == nil {
			return StatusInvalid, errors.New("human device event must be account-addressed")
		}
		if len(content.Parents) == 0 {
			return StatusInvalid, errors.New("human device event needs a causal parent")
		}
		if err := validateMessageAddresses(content, false); err != nil {
			return StatusInvalid, err
		}
		var payload HumanDevicePayload
		if err := decodePayload(content.Payload, &payload); err != nil {
			return StatusInvalid, err
		}
		if err := validateHumanDevicePayload(payload); err != nil {
			return StatusInvalid, err
		}
		if content.Audience.HumanAccountID != payload.AccountID {
			return StatusInvalid, errors.New("human device audience does not match its account")
		}
		if content.Type == TypeHumanDeviceAccept {
			if payload.InstallationID != content.InstallationID || payload.SignerKeyID != content.SignerKeyID || content.Sender.InstallationID != payload.InstallationID || content.Recipient.InstallationID != payload.CreatorInstallationID {
				return StatusInvalid, errors.New("device acceptance does not match the invited event signer and route")
			}
		} else if payload.CreatorInstallationID != content.InstallationID || payload.CreatorSignerKeyID != content.SignerKeyID || content.Sender.InstallationID != payload.CreatorInstallationID || content.Recipient.InstallationID != payload.InstallationID {
			return StatusInvalid, errors.New("device grant or revocation does not match the creator event signer and route")
		}
	}
	return StatusProjected, nil
}

func knownType(kind Type) bool {
	switch kind {
	case TypeInstallationCreate, TypeMailboxCreate, TypeMailboxBind, TypeMailboxContext, TypeQuestion, TypeAnswer, TypeMessage,
		TypeThreadCancel, TypeMessageArchive, TypeMessageRestore, TypeMessageReject, TypePeerTrust, TypePeerDistrust,
		TypeMailboxShare, TypeMailboxShareRevoke, TypeHumanAccountCreate, TypeHumanAccountSelect,
		TypeHumanDeviceGrant, TypeHumanDeviceAccept, TypeHumanDeviceRevoke:
		return true
	default:
		return false
	}
}

func validateHumanAccountPayload(payload HumanAccountPayload) error {
	if err := validUUID("human account ID", payload.AccountID); err != nil {
		return err
	}
	if err := validUUID("creator installation ID", payload.CreatorInstallationID); err != nil {
		return err
	}
	if err := validHex("creator signer key ID", payload.CreatorSignerKeyID, 32); err != nil {
		return err
	}
	if strings.TrimSpace(payload.Label) == "" || !utf8.ValidString(payload.Label) || len(payload.Label) > 200 {
		return errors.New("human account label must be valid non-empty UTF-8 of at most 200 bytes")
	}
	return nil
}

func validateHumanDevicePayload(payload HumanDevicePayload) error {
	if err := validateHumanAccountPayload(HumanAccountPayload{AccountID: payload.AccountID, CreatorInstallationID: payload.CreatorInstallationID, CreatorSignerKeyID: payload.CreatorSignerKeyID, Label: "account"}); err != nil {
		return err
	}
	if err := validUUID("device installation ID", payload.InstallationID); err != nil {
		return err
	}
	if err := validHex("device signer key ID", payload.SignerKeyID, 32); err != nil {
		return err
	}
	if payload.InstallationID == payload.CreatorInstallationID {
		return errors.New("creator installation is already an account device")
	}
	if strings.TrimSpace(payload.Label) == "" || !utf8.ValidString(payload.Label) || len(payload.Label) > 200 {
		return errors.New("device label must be valid non-empty UTF-8 of at most 200 bytes")
	}
	for _, group := range []struct {
		name   string
		relays []string
	}{{"device", payload.Relays}, {"creator", payload.CreatorRelays}} {
		name, relays := group.name, group.relays
		if len(relays) > 3 {
			return fmt.Errorf("%s may have at most three relay hints", name)
		}
		for _, relay := range relays {
			parsed, err := url.Parse(relay)
			if err != nil || (parsed.Scheme != "wss" && parsed.Scheme != "ws") || parsed.Host == "" {
				return fmt.Errorf("invalid %s relay %q", name, relay)
			}
		}
	}
	return nil
}

func validateMessageAddresses(content Content, accountQuestion bool) error {
	if content.Sender == nil || (content.Recipient == nil && !accountQuestion) {
		return errors.New("message event needs a sender and its route recipient")
	}
	if err := validateAddress("sender", *content.Sender); err != nil {
		return err
	}
	if content.Recipient != nil {
		if err := validateAddress("recipient", *content.Recipient); err != nil {
			return err
		}
	}
	if content.Sender.InstallationID != content.InstallationID {
		return errors.New("sender installation does not match event installation")
	}
	if content.Scope == ScopeInstallationPrivate && content.Recipient.InstallationID != content.InstallationID {
		return errors.New("installation-private message has a remote recipient")
	}
	if content.Scope == ScopePeerAddressed && content.Recipient.InstallationID == content.InstallationID {
		return errors.New("peer-addressed message has a local recipient")
	}
	if content.Scope == ScopeAccountAddressed && accountQuestion && content.Recipient != nil {
		return errors.New("account question must use its audience instead of a recipient")
	}
	return nil
}

func validateControl(content Content) error {
	if content.Scope != ScopeInstallationPrivate {
		return errors.New("control event must be installation-private")
	}
	if content.Sender != nil || content.Recipient != nil || content.Audience != nil || content.ThreadID != "" {
		return errors.New("control event must omit sender, recipient, audience, and thread_id")
	}
	return nil
}

func validateTextPayload(raw json.RawMessage) error {
	var payload TextPayload
	if err := decodePayload(raw, &payload); err != nil {
		return err
	}
	if strings.TrimSpace(payload.Body) == "" {
		return errors.New("message body is empty")
	}
	if payload.MessageID != "" {
		if err := validUUID("message ID", payload.MessageID); err != nil {
			return err
		}
	}
	if payload.Context != nil && strings.TrimSpace(payload.Context.Directory) == "" {
		return errors.New("message context needs a directory")
	}
	if !utf8.ValidString(payload.Body) || !utf8.ValidString(payload.Details) {
		return errors.New("message text is not valid UTF-8")
	}
	if len(payload.Body) > MaxBodyBytes {
		return fmt.Errorf("message body is %d bytes; limit is %d", len(payload.Body), MaxBodyBytes)
	}
	if len(payload.Details) > MaxDetailBytes {
		return fmt.Errorf("message details are %d bytes; limit is %d", len(payload.Details), MaxDetailBytes)
	}
	return nil
}

func validateTargetPayload(raw json.RawMessage, requireTarget bool) error {
	var payload TargetPayload
	if err := decodePayload(raw, &payload); err != nil {
		return err
	}
	if requireTarget {
		if err := validEventID("target event ID", payload.TargetEventID); err != nil {
			return err
		}
	}
	return nil
}

func decodePayload(raw json.RawMessage, target any) error {
	if len(raw) == 0 || string(raw) == "null" {
		return errors.New("event payload is required")
	}
	if err := decodeStrict(raw, target); err != nil {
		return fmt.Errorf("decode %T payload: %w", target, err)
	}
	return nil
}

func validateAddress(name string, address MailboxAddress) error {
	if err := validUUID(name+" installation ID", address.InstallationID); err != nil {
		return err
	}
	return validUUID(name+" mailbox ID", address.MailboxID)
}

func validUUID(name, value string) error {
	parsed, err := uuid.Parse(value)
	if err != nil || parsed.String() != strings.ToLower(value) {
		return fmt.Errorf("%s must be a canonical UUID", name)
	}
	return nil
}

func validEventID(name, value string) error { return validHex(name, value, 32) }

func validHex(name, value string, bytes int) error {
	if len(value) != bytes*2 || value != strings.ToLower(value) {
		return fmt.Errorf("%s must be %d-byte lowercase hex", name, bytes)
	}
	if _, err := hex.DecodeString(value); err != nil {
		return fmt.Errorf("%s must be %d-byte lowercase hex", name, bytes)
	}
	return nil
}
