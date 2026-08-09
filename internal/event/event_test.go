package event

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"
	"time"
)

const (
	installationA = "0198c7ec-73b0-7cc3-a5f7-e31c77140d01"
	installationB = "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	mailboxHumanA = "00000000-0000-7000-8000-000000000000"
	mailboxAgentA = "0198c7ec-73b0-7cc3-a5f7-e31c77140d11"
	mailboxHumanB = "0198c7ec-73b0-7cc3-a5f7-e31c77140d12"
	parentA       = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
	threadB       = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
)

var (
	secretA = MustSecretKeyFromHex("1")
	secretB = MustSecretKeyFromHex("2")
)

func TestCanonicalEventFixture(t *testing.T) {
	payload := mustPayload(t, TextPayload{Body: "Line one\nLine two", Details: "quote: \"yes\""})
	event := mustSign(t, Content{
		Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: payload,
	}, time.Unix(1_700_000_000, 0), secretA)

	const wantID = "1dedba9588289d2732682a9b55035502e6caae6433cb607bfe1242e0dbf2f2a2"
	const wantSignature = "20c6fe2d1a24cceac6d724a183df9a888adcf787bd3eeb0e5896351af35240df4908108dd22924051ec90edaba533b988e9785e6f51f00bfcfb53c2bf2303247"
	if event.ID() != wantID || jsonSignature(event.Nostr) != wantSignature {
		serialized, _ := event.Nostr.Serialize()
		t.Fatalf("fixture changed:\nid = %s\nsig = %s\ncontent = %s\nserialized = %s", event.ID(), jsonSignature(event.Nostr), event.Nostr.Content, serialized)
	}
	if !event.Nostr.CheckID() || !event.Nostr.VerifySignature() {
		t.Fatal("fixture failed NIP-01 ID or signature validation")
	}
	if got := Inspect(event.Wire); got.Status != StatusProjected || got.Err != nil {
		t.Fatalf("inspect fixture = %s, %v", got.Status, got.Err)
	}
}

func TestEveryKnownEventTypeValidates(t *testing.T) {
	localSender := &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}
	localHuman := &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}
	remoteHuman := &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}
	tests := []struct {
		name    string
		content Content
	}{
		{"installation create", control(TypeInstallationCreate, mustPayload(t, InstallationPayload{Label: "laptop"}))},
		{"mailbox create", control(TypeMailboxCreate, mustPayload(t, MailboxPayload{MailboxID: mailboxAgentA, Kind: "agent", Label: "codex"}))},
		{"mailbox bind", control(TypeMailboxBind, mustPayload(t, MailboxBindingPayload{MailboxID: mailboxAgentA, Harness: "codex", ExternalSessionID: "thread"}))},
		{"question", Content{Type: TypeQuestion, InstallationID: installationA, Sender: localSender, Recipient: localHuman, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "question"})}},
		{"answer", Content{Type: TypeAnswer, InstallationID: installationA, Sender: localHuman, Recipient: localSender, ThreadID: threadB, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "answer"})}},
		{"message", Content{Type: TypeMessage, InstallationID: installationA, Sender: localSender, Recipient: remoteHuman, Scope: ScopePeerAddressed, Payload: mustPayload(t, TextPayload{Body: "message"})}},
		{"cancel", Content{Type: TypeThreadCancel, InstallationID: installationA, Sender: localSender, ThreadID: threadB, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{Reason: "done"})}},
		{"archive", Content{Type: TypeMessageArchive, InstallationID: installationA, Sender: localSender, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: parentA})}},
		{"reject", Content{Type: TypeMessageReject, InstallationID: installationA, Sender: localSender, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: parentA, Reason: "wrong target"})}},
		{"peer trust", control(TypePeerTrust, mustPayload(t, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Name: "desktop", Relays: []string{"wss://relay.example"}}))},
		{"peer distrust", control(TypePeerDistrust, mustPayload(t, PeerPayload{InstallationID: installationB}))},
		{"mailbox share", control(TypeMailboxShare, mustPayload(t, MailboxSharePayload{MailboxID: mailboxAgentA, PeerInstallationID: installationB}))},
		{"mailbox share revoke", control(TypeMailboxShareRevoke, mustPayload(t, MailboxSharePayload{MailboxID: mailboxAgentA, PeerInstallationID: installationB}))},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			event, err := Sign(test.content, time.Unix(1_700_000_000, 0), secretA)
			if err != nil {
				t.Fatal(err)
			}
			if got := Inspect(event.Wire); got.Status != StatusProjected || got.Err != nil {
				t.Fatalf("inspect = %s, %v", got.Status, got.Err)
			}
		})
	}
}

func TestValidationFailures(t *testing.T) {
	base := Content{
		Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "valid"}),
	}
	tests := []struct {
		name string
		edit func(*Content)
		want string
	}{
		{"bad installation", func(content *Content) { content.InstallationID = "no" }, "canonical UUID"},
		{"public disabled", func(content *Content) { content.Scope = ScopePublic }, "disabled"},
		{"root parent", func(content *Content) { content.Parents = []string{parentA} }, "omit parents"},
		{"remote private recipient", func(content *Content) { content.Recipient.InstallationID = installationB }, "remote recipient"},
		{"empty body", func(content *Content) { content.Payload = mustPayload(t, TextPayload{}) }, "body is empty"},
		{"large body", func(content *Content) {
			content.Payload = mustPayload(t, TextPayload{Body: strings.Repeat("x", MaxBodyBytes+1)})
		}, "body is"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			content := base
			sender, recipient := *base.Sender, *base.Recipient
			content.Sender, content.Recipient = &sender, &recipient
			test.edit(&content)
			_, err := Sign(content, time.Unix(1_700_000_000, 0), secretA)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want text %q", err, test.want)
			}
		})
	}
}

func TestInspectRejectsTamperingAndLimits(t *testing.T) {
	event := mustQuestion(t, "valid", time.Unix(1_700_000_000, 0))
	var tampered NostrEvent
	if err := json.Unmarshal(event.Wire, &tampered); err != nil {
		t.Fatal(err)
	}
	tampered.Content += " "
	raw, err := json.Marshal(tampered)
	if err != nil {
		t.Fatal(err)
	}
	if got := Inspect(raw); got.Status != StatusInvalid || !strings.Contains(got.Err.Error(), "ID") {
		t.Fatalf("tampered status = %s, %v", got.Status, got.Err)
	}
	if got := Inspect(make([]byte, MaxWireBytes+1)); got.Status != StatusInvalid || !strings.Contains(got.Err.Error(), "limit") {
		t.Fatalf("oversize status = %s, %v", got.Status, got.Err)
	}
	var tagged NostrEvent
	if err := json.Unmarshal(event.Wire, &tagged); err != nil {
		t.Fatal(err)
	}
	tagged.Tags = [][]string{{"x", "not canonical"}}
	if err := tagged.Sign(secretA); err != nil {
		t.Fatal(err)
	}
	taggedWire, _ := json.Marshal(tagged)
	if got := Inspect(taggedWire); got.Status != StatusInvalid || !strings.Contains(got.Err.Error(), "tags") {
		t.Fatalf("tagged status = %s, %v", got.Status, got.Err)
	}
}

func TestUnsupportedSchemaCanBeRegistered(t *testing.T) {
	content := Content{
		Schema: 2, Type: TypeQuestion, InstallationID: installationA, SignerKeyID: secretA.PublicKeyHex(),
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Parents:   []string{}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "future"}),
	}
	rawContent, err := json.Marshal(content)
	if err != nil {
		t.Fatal(err)
	}
	nostrEvent := NostrEvent{CreatedAt: 1_700_000_000, Kind: Kind, Tags: [][]string{}, Content: string(rawContent)}
	if err := nostrEvent.Sign(secretA); err != nil {
		t.Fatal(err)
	}
	wire, err := json.Marshal(nostrEvent)
	if err != nil {
		t.Fatal(err)
	}
	if got := Inspect(wire); got.Status != StatusUnsupported {
		t.Fatalf("default status = %s, %v", got.Status, got.Err)
	}
	if got := InspectWithSchemas(wire, []int{1, 2}); got.Status != StatusProjected || got.Err != nil {
		t.Fatalf("registered status = %s, %v", got.Status, got.Err)
	}
}

func TestUnknownTypeAndKindAreUnsupported(t *testing.T) {
	base := Content{Schema: SchemaVersion, Type: "future", InstallationID: installationA, SignerKeyID: secretA.PublicKeyHex(), Parents: []string{}, Scope: ScopeInstallationPrivate, Payload: json.RawMessage(`{}`)}
	for _, kind := range []uint16{Kind, Kind + 1} {
		rawContent, _ := json.Marshal(base)
		nostrEvent := NostrEvent{CreatedAt: 1_700_000_000, Kind: kind, Tags: [][]string{}, Content: string(rawContent)}
		if err := nostrEvent.Sign(secretA); err != nil {
			t.Fatal(err)
		}
		wire, _ := json.Marshal(nostrEvent)
		if got := Inspect(wire); got.Status != StatusUnsupported {
			t.Fatalf("kind %d status = %s, %v", kind, got.Status, got.Err)
		}
	}
}

func control(kind Type, payload json.RawMessage) Content {
	return Content{Type: kind, InstallationID: installationA, Scope: ScopeInstallationPrivate, Payload: payload}
}

func mustQuestion(t *testing.T, body string, created time.Time) SignedEvent {
	t.Helper()
	return mustSign(t, Content{
		Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: body}),
	}, created, secretA)
}

func mustPayload(t *testing.T, value any) json.RawMessage {
	t.Helper()
	payload, err := MarshalPayload(value)
	if err != nil {
		t.Fatal(err)
	}
	return payload
}

func mustSign(t *testing.T, content Content, created time.Time, secret SecretKey) SignedEvent {
	t.Helper()
	event, err := Sign(content, created, secret)
	if err != nil {
		t.Fatal(err)
	}
	return event
}

func jsonSignature(event NostrEvent) string { return event.Sig }

func TestErrorValuesRemainDistinct(t *testing.T) {
	if errors.Is(ErrNotFound, ErrNoAnswer) || errors.Is(ErrWaitDenied, ErrNoAnswer) {
		t.Fatal("query errors must remain distinct")
	}
}
