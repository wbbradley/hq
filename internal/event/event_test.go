package event

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

const (
	installationA = "0198c7ec-73b0-7cc3-a5f7-e31c77140d01"
	installationB = "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	mailboxHumanA = "00000000-0000-7000-8000-000000000000"
	mailboxAgentA = "0198c7ec-73b0-7cc3-a5f7-e31c77140d11"
	mailboxHumanB = "0198c7ec-73b0-7cc3-a5f7-e31c77140d12"
	parentA       = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
	threadB       = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
	accountA      = "0198c7ec-73b0-7cc3-a5f7-e31c77140d21"
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

func TestHumanAccountEventFixture(t *testing.T) {
	payload := mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), Label: "laptop"})
	signed := mustSign(t, Content{Type: TypeHumanAccountCreate, InstallationID: installationA, Scope: ScopeInstallationPrivate, Payload: payload}, time.Unix(1_700_000_000, 0), secretA)
	const wantID = "cebb4a6d69bf75643db1abfaf20facab2bd5f4fe112a37469b19249574075c82"
	const wantSignature = "be82d6413e1ddd135eb943a132016a159b139f77febe486765b7832221bff3a4281fbc8b644f92d5df3694899264923386fe045c3960acb929f27245fa30cc51"
	if signed.ID() != wantID || signed.Nostr.Sig != wantSignature {
		t.Fatalf("fixture changed:\nid = %s\nsig = %s\nwire = %s", signed.ID(), signed.Nostr.Sig, signed.Wire)
	}
}

func TestEveryKnownEventTypeValidates(t *testing.T) {
	localSender := &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}
	localHuman := &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}
	remoteHuman := &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}
	remoteAccountHuman := &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA}
	tests := []struct {
		name    string
		content Content
	}{
		{"installation create", control(TypeInstallationCreate, mustPayload(t, InstallationPayload{Label: "laptop"}))},
		{"mailbox create", control(TypeMailboxCreate, mustPayload(t, MailboxPayload{MailboxID: mailboxAgentA, Kind: "agent", Label: "codex"}))},
		{"mailbox bind", control(TypeMailboxBind, mustPayload(t, MailboxBindingPayload{MailboxID: mailboxAgentA, Harness: "codex", ExternalSessionID: "thread"}))},
		{"mailbox context", control(TypeMailboxContext, mustPayload(t, MailboxContextPayload{MailboxID: mailboxAgentA, Context: RepositoryContext{Directory: "/repo"}}))},
		{"agent session rename", control(TypeAgentSessionRename, mustPayload(t, AgentSessionRenamePayload{Name: "fred", MailboxID: mailboxAgentA, Harness: "codex", ExternalSessionID: "thread", ThreadName: "Build auth"}))},
		{"question", Content{Type: TypeQuestion, InstallationID: installationA, Sender: localSender, Recipient: localHuman, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "question"})}},
		{"answer", Content{Type: TypeAnswer, InstallationID: installationA, Sender: localHuman, Recipient: localSender, ThreadID: threadB, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TextPayload{Body: "answer"})}},
		{"message", Content{Type: TypeMessage, InstallationID: installationA, Sender: localSender, Recipient: remoteHuman, Scope: ScopePeerAddressed, Payload: mustPayload(t, TextPayload{Body: "message"})}},
		{"cancel", Content{Type: TypeThreadCancel, InstallationID: installationA, Sender: localSender, ThreadID: threadB, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{Reason: "done"})}},
		{"archive", Content{Type: TypeMessageArchive, InstallationID: installationA, Sender: localSender, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: parentA})}},
		{"restore", Content{Type: TypeMessageRestore, InstallationID: installationA, Sender: localSender, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: parentA})}},
		{"reject", Content{Type: TypeMessageReject, InstallationID: installationA, Sender: localSender, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: parentA, Reason: "wrong target"})}},
		{"peer trust", control(TypePeerTrust, mustPayload(t, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Name: "desktop", Relays: []string{"wss://relay.example"}}))},
		{"peer distrust", control(TypePeerDistrust, mustPayload(t, PeerPayload{InstallationID: installationB}))},
		{"mailbox share", control(TypeMailboxShare, mustPayload(t, MailboxSharePayload{MailboxID: mailboxAgentA, PeerInstallationID: installationB}))},
		{"mailbox share revoke", control(TypeMailboxShareRevoke, mustPayload(t, MailboxSharePayload{MailboxID: mailboxAgentA, PeerInstallationID: installationB}))},
		{"human account create", control(TypeHumanAccountCreate, mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), Label: "laptop"}))},
		{"human account select", Content{Type: TypeHumanAccountSelect, InstallationID: installationA, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, HumanAccountSelectionPayload{AccountID: accountA})}},
		{"human device grant", Content{Type: TypeHumanDeviceGrant, InstallationID: installationA, Sender: localHuman, Recipient: remoteAccountHuman, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop", Relays: []string{"wss://relay.example"}})}},
		{"human device revoke", Content{Type: TypeHumanDeviceRevoke, InstallationID: installationA, Sender: localHuman, Recipient: remoteAccountHuman, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop", Relays: []string{"wss://relay.example"}})}},
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
	accept := Content{Type: TypeHumanDeviceAccept, InstallationID: installationB, Sender: remoteAccountHuman, Recipient: localHuman, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop", Relays: []string{"wss://relay.example"}})}
	if _, err := Sign(accept, time.Unix(1_700_000_000, 0), secretB); err != nil {
		t.Fatalf("human device accept: %v", err)
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
		{"root parent", func(content *Content) { content.Parents = []string{parentA} }, "account-addressed roots"},
		{"remote private recipient", func(content *Content) { content.Recipient.InstallationID = installationB }, "remote recipient"},
		{"empty body", func(content *Content) { content.Payload = mustPayload(t, TextPayload{}) }, "body is empty"},
		{"unknown purpose", func(content *Content) {
			content.Payload = mustPayload(t, TextPayload{Body: "valid", Purpose: "made-up"})
		}, "unsupported message purpose"},
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

func TestHumanEventValidationFailures(t *testing.T) {
	device := HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop"}
	local := &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}
	remote := &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}
	tests := []struct {
		name    string
		content Content
		secret  SecretKey
		want    string
	}{
		{"creator mismatch", Content{Type: TypeHumanAccountCreate, InstallationID: installationA, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationB, CreatorSignerKeyID: secretA.PublicKeyHex(), Label: "laptop"})}, secretA, "creator does not match"},
		{"empty account label", Content{Type: TypeHumanAccountCreate, InstallationID: installationA, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex()})}, secretA, "account label"},
		{"grant private", Content{Type: TypeHumanDeviceGrant, InstallationID: installationA, Sender: local, Recipient: remote, Parents: []string{parentA}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, device)}, secretA, "account-addressed"},
		{"grant wrong route", Content{Type: TypeHumanDeviceGrant, InstallationID: installationA, Sender: remote, Recipient: local, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, device)}, secretA, "sender installation"},
		{"bad relay", Content{Type: TypeHumanDeviceGrant, InstallationID: installationA, Sender: local, Recipient: remote, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, func() HumanDevicePayload {
			changed := device
			changed.Relays = []string{"https://relay.example"}
			return changed
		}())}, secretA, "invalid device relay"},
		{"accept wrong key", Content{Type: TypeHumanDeviceAccept, InstallationID: installationB, Sender: remote, Recipient: local, Audience: &Audience{HumanAccountID: accountA}, Parents: []string{parentA}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, device)}, secretA, "invited event signer"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := Sign(test.content, time.Unix(1_700_000_000, 0), test.secret)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
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
	if got := InspectWithSchemas(wire, []int{Schema1}); got.Status != StatusUnsupported || !stringSlicesEqual(got.Event.Wire, wire) {
		t.Fatalf("schema-1-only status = %s, %v, retained=%t", got.Status, got.Err, stringSlicesEqual(got.Event.Wire, wire))
	}
	if got := Inspect(wire); got.Status != StatusProjected || got.Err != nil {
		t.Fatalf("current status = %s, %v", got.Status, got.Err)
	}
}

func TestSchema2MessageSemanticsValidateSignAndInspect(t *testing.T) {
	payload := TextPayload{
		MessageID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d31",
		Body:      "done", Details: "Human-readable explanation.",
		Presentation: model.PresentationFinalAnswer,
		Correlation: model.MessageCorrelation{
			Provider: "home-built", SessionID: "session-1", OperationID: "operation-1",
			ItemID: "item-1", RequestID: "request-1",
		},
		TechnicalSections: []model.TechnicalSection{{
			Namespace: "hq.harness.output",
			Fields: []model.TechnicalField{
				{Key: "status", Label: "Status", Value: "completed"},
				{Key: "source_sequence", Label: "Source sequence", Value: "42"},
			},
		}},
	}
	signed := mustSign(t, schema2Question(t, payload), time.Unix(1_700_000_000, 0), secretA)
	inspection := Inspect(signed.Wire)
	if inspection.Status != StatusProjected || inspection.Err != nil {
		t.Fatalf("inspect schema 2 = %s, %v", inspection.Status, inspection.Err)
	}
	var decoded TextPayload
	if err := decodePayload(inspection.Event.Content.Payload, &decoded); err != nil {
		t.Fatal(err)
	}
	if decoded.Presentation != payload.Presentation || decoded.Correlation != payload.Correlation || !technicalSectionsEqual(decoded.TechnicalSections, payload.TechnicalSections) {
		t.Fatalf("decoded semantics = %#v", decoded)
	}
}

func TestSchemaSpecificTextPayloadDecodingIsStrict(t *testing.T) {
	payload := mustPayload(t, TextPayload{Body: "hello", Presentation: model.PresentationUpdate})
	schema1 := Content{
		Schema: Schema1, Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: payload,
	}
	if _, err := Sign(schema1, time.Unix(1_700_000_000, 0), secretA); err == nil || !strings.Contains(err.Error(), "unknown field") {
		t.Fatalf("schema 1 extended payload error = %v", err)
	}
	if _, err := Sign(schema2Question(t, TextPayload{Body: "hello", Presentation: model.PresentationUpdate}), time.Unix(1_700_000_000, 0), secretA); err != nil {
		t.Fatalf("schema 2 payload: %v", err)
	}
}

func TestSchema2SemanticValidationFailures(t *testing.T) {
	tests := []struct {
		name    string
		payload TextPayload
		want    string
	}{
		{name: "presentation", payload: TextPayload{Body: "x", Presentation: "question"}, want: "presentation"},
		{name: "provider without session", payload: TextPayload{Body: "x", Correlation: model.MessageCorrelation{Provider: "provider"}}, want: "correlation"},
		{name: "item without operation", payload: TextPayload{Body: "x", Correlation: model.MessageCorrelation{Provider: "provider", SessionID: "session", ItemID: "item"}}, want: "operation"},
		{name: "namespace", payload: TextPayload{Body: "x", TechnicalSections: []model.TechnicalSection{{Namespace: "Not Namespaced", Fields: []model.TechnicalField{{Key: "key", Value: "value"}}}}}, want: "namespace"},
		{name: "key", payload: TextPayload{Body: "x", TechnicalSections: []model.TechnicalSection{{Namespace: "hq.test", Fields: []model.TechnicalField{{Key: "Display Key", Value: "value"}}}}}, want: "key"},
		{name: "empty fields", payload: TextPayload{Body: "x", TechnicalSections: []model.TechnicalSection{{Namespace: "hq.test"}}}, want: "field"},
		{name: "duplicate pair", payload: TextPayload{Body: "x", TechnicalSections: []model.TechnicalSection{
			{Namespace: "hq.test", Fields: []model.TechnicalField{{Key: "same", Value: "one"}}},
			{Namespace: "hq.test", Fields: []model.TechnicalField{{Key: "same", Value: "two"}}},
		}}, want: "duplicate"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := Sign(schema2Question(t, test.payload), time.Unix(1_700_000_000, 0), secretA)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
	invalidUTF8 := []model.TechnicalSection{{Namespace: "hq.test", Fields: []model.TechnicalField{{Key: "key", Value: string([]byte{0xff})}}}}
	if err := validateTechnicalSections(invalidUTF8); err == nil || !strings.Contains(err.Error(), "UTF-8") {
		t.Fatalf("invalid UTF-8 error = %v", err)
	}
}

func TestSchema2PresentationAndCorrelationShapes(t *testing.T) {
	presentations := []model.PresentationKind{"", model.PresentationUpdate, model.PresentationFinalAnswer, model.PresentationStatus, model.PresentationNotice}
	for _, presentation := range presentations {
		if _, err := Sign(schema2Question(t, TextPayload{Body: "x", Presentation: presentation}), time.Unix(1_700_000_000, 0), secretA); err != nil {
			t.Fatalf("presentation %q: %v", presentation, err)
		}
	}
	correlations := []model.MessageCorrelation{
		{},
		{Provider: "provider", SessionID: "session"},
		{Provider: "provider", SessionID: "session", OperationID: "operation"},
		{Provider: "provider", SessionID: "session", OperationID: "operation", ItemID: "item"},
		{Provider: "provider", SessionID: "session", OperationID: "operation", RequestID: "request"},
		{Provider: "provider", SessionID: "session", OperationID: "operation", ItemID: "item", RequestID: "request"},
	}
	for _, correlation := range correlations {
		if _, err := Sign(schema2Question(t, TextPayload{Body: "x", Correlation: correlation}), time.Unix(1_700_000_000, 0), secretA); err != nil {
			t.Fatalf("correlation %#v: %v", correlation, err)
		}
	}
}

func TestSchema2TechnicalAndCorrelationBounds(t *testing.T) {
	sections := make([]model.TechnicalSection, MaxTechnicalSections+1)
	for index := range sections {
		sections[index] = model.TechnicalSection{Namespace: fmt.Sprintf("hq.section_%d", index), Fields: []model.TechnicalField{{Key: "key", Value: "value"}}}
	}
	tooManySectionFields := make([]model.TechnicalField, MaxTechnicalFieldsPerSection+1)
	for index := range tooManySectionFields {
		tooManySectionFields[index] = model.TechnicalField{Key: fmt.Sprintf("key_%d", index), Value: "value"}
	}
	tooManyTotalFields := make([]model.TechnicalSection, 5)
	for sectionIndex := range tooManyTotalFields {
		fields := make([]model.TechnicalField, 26)
		for fieldIndex := range fields {
			fields[fieldIndex] = model.TechnicalField{Key: fmt.Sprintf("key_%d", fieldIndex), Value: "value"}
		}
		tooManyTotalFields[sectionIndex] = model.TechnicalSection{Namespace: fmt.Sprintf("hq.total_%d", sectionIndex), Fields: fields}
	}
	tests := []struct {
		name     string
		sections []model.TechnicalSection
		want     string
	}{
		{name: "section count", sections: sections, want: "technical sections"},
		{name: "fields per section", sections: []model.TechnicalSection{{Namespace: "hq.fields", Fields: tooManySectionFields}}, want: "has 33 fields"},
		{name: "total field count", sections: tooManyTotalFields, want: "technical fields"},
		{name: "label bytes", sections: []model.TechnicalSection{{Namespace: "hq.label", Fields: []model.TechnicalField{{Key: "key", Label: strings.Repeat("界", MaxTechnicalLabelBytes/3+1), Value: "value"}}}}, want: "label"},
		{name: "value bytes", sections: []model.TechnicalSection{{Namespace: "hq.value", Fields: []model.TechnicalField{{Key: "key", Value: strings.Repeat("界", MaxTechnicalValueBytes/3+1)}}}}, want: "value"},
		{name: "aggregate bytes", sections: []model.TechnicalSection{{Namespace: "hq.aggregate", Fields: []model.TechnicalField{{Key: "one", Value: strings.Repeat("a", MaxTechnicalValueBytes)}, {Key: "two", Value: strings.Repeat("b", MaxTechnicalValueBytes)}, {Key: "three", Value: strings.Repeat("c", MaxTechnicalValueBytes)}, {Key: "four", Value: strings.Repeat("d", MaxTechnicalValueBytes)}}}}, want: "technical payload"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if err := validateTechnicalSections(test.sections); err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
	correlationTests := []struct {
		name        string
		correlation model.MessageCorrelation
		want        string
	}{
		{name: "provider bytes", correlation: model.MessageCorrelation{Provider: strings.Repeat("界", MaxCorrelationProviderBytes/3+1), SessionID: "session"}, want: "provider"},
		{name: "identity bytes", correlation: model.MessageCorrelation{Provider: "provider", SessionID: strings.Repeat("界", MaxCorrelationIDBytes/3+1)}, want: "session"},
		{name: "identity control", correlation: model.MessageCorrelation{Provider: "provider", SessionID: "bad\nidentity"}, want: "printable"},
		{name: "identity utf8", correlation: model.MessageCorrelation{Provider: "provider", SessionID: string([]byte{0xff})}, want: "UTF-8"},
	}
	for _, test := range correlationTests {
		t.Run(test.name, func(t *testing.T) {
			if err := validateMessageCorrelation(test.correlation); err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want %q", err, test.want)
			}
		})
	}
}

func TestSchema2SignedWireLimitIncludesEscaping(t *testing.T) {
	payload := TextPayload{
		Body:    strings.Repeat("\\", MaxBodyBytes),
		Details: strings.Repeat("\\", MaxDetailBytes),
	}
	_, err := Sign(schema2Question(t, payload), time.Unix(1_700_000_000, 0), secretA)
	if err == nil || !strings.Contains(err.Error(), "wire") || !strings.Contains(err.Error(), "limit") {
		t.Fatalf("escaped signed-wire error = %v", err)
	}
}

func TestSchema2SignedWireLimitIncludesEscapedMultibyteText(t *testing.T) {
	payload := TextPayload{
		Body:    "x" + strings.Repeat("\u2028", (MaxBodyBytes-1)/3),
		Details: strings.Repeat("\u2028", MaxDetailBytes/3),
	}
	_, err := Sign(schema2Question(t, payload), time.Unix(1_700_000_000, 0), secretA)
	if err == nil || !strings.Contains(err.Error(), "wire") || !strings.Contains(err.Error(), "limit") {
		t.Fatalf("multibyte signed-wire error = %v", err)
	}
}

func schema2Question(t *testing.T, payload TextPayload) Content {
	t.Helper()
	return Content{
		Schema: Schema2, Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: mustPayload(t, payload),
	}
}

func stringSlicesEqual(left, right []byte) bool { return string(left) == string(right) }

func technicalSectionsEqual(left, right []model.TechnicalSection) bool {
	leftJSON, _ := json.Marshal(left)
	rightJSON, _ := json.Marshal(right)
	return string(leftJSON) == string(rightJSON)
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
