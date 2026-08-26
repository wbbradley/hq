package event

import (
	"encoding/json"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

func TestSchema3RequiresTypedAuthorityReferences(t *testing.T) {
	grantID := mustSign(t, Content{
		Schema: Schema3, Type: TypeMailboxAccessGrant, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: model.HumanMailboxID},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: model.HumanMailboxID},
		Scope:     ScopePeerAddressed,
		Payload: mustPayload(t, MailboxAccessPayload{
			MailboxID: mailboxHumanA, GranteeInstallationID: installationB,
			GranteeSignerKeyID: secretB.PublicKeyHex(),
		}),
	}, time.Unix(1, 0), secretA).ID()
	payload := mustPayload(t, TextPayload{Body: "authorized"})
	content := Content{
		Schema: Schema3, Type: TypeMessage, InstallationID: installationB,
		Sender:    &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Parents:   []string{grantID}, Authorities: []string{grantID}, Scope: ScopePeerAddressed,
		Payload: payload,
	}
	if _, err := Sign(content, time.Unix(2, 0), secretB); err != nil {
		t.Fatal(err)
	}
	content.Parents = nil
	if _, err := Sign(content, time.Unix(2, 0), secretB); err == nil {
		t.Fatal("authority outside causal parents was accepted")
	}
	content.Parents = []string{grantID}
	content.Authorities = nil
	if _, err := Sign(content, time.Unix(2, 0), secretB); err == nil {
		t.Fatal("peer message without capability authority was accepted")
	}
}

func TestSchema3RejectsLegacySchemas(t *testing.T) {
	payload := mustPayload(t, TextPayload{Body: "legacy"})
	for _, schema := range []int{1, 2} {
		content := Content{
			Schema: schema, Type: TypeMessage, InstallationID: installationA,
			Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
			Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: model.HumanMailboxID},
			Scope:     ScopeInstallationPrivate, Payload: payload,
		}
		rawContent, err := json.Marshal(content)
		if err != nil {
			t.Fatal(err)
		}
		nostr := NostrEvent{CreatedAt: int64(schema), Kind: Kind, Tags: [][]string{}, Content: string(rawContent)}
		if err := nostr.Sign(secretA); err != nil {
			t.Fatal(err)
		}
		raw, err := json.Marshal(nostr)
		if err != nil {
			t.Fatal(err)
		}
		inspection := Inspect(raw)
		if inspection.Status != StatusUnsupported {
			t.Fatalf("schema %d status = %s", schema, inspection.Status)
		}
	}
}

func TestMailboxAccessFactsHaveStrictShapes(t *testing.T) {
	grant := Content{
		Schema: Schema3, Type: TypeMailboxAccessGrant, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: model.HumanMailboxID},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: model.HumanMailboxID},
		Scope:     ScopePeerAddressed,
		Payload:   mustPayload(t, MailboxAccessPayload{MailboxID: mailboxHumanA, GranteeInstallationID: installationB, GranteeSignerKeyID: secretB.PublicKeyHex()}),
	}
	signed, err := Sign(grant, time.Unix(3, 0), secretA)
	if err != nil {
		t.Fatal(err)
	}
	observe := Content{
		Schema: Schema3, Type: TypeMailboxAccessObserve, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: model.HumanMailboxID},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: model.HumanMailboxID},
		Parents:   []string{signed.ID()}, Authorities: []string{signed.ID()}, Scope: ScopePeerAddressed,
		Payload: mustPayload(t, MailboxAccessObservationPayload{GrantEventID: signed.ID(), MessageEventID: signed.ID()}),
	}
	if _, err := Sign(observe, time.Unix(4, 0), secretA); err == nil {
		t.Fatal("observation whose message target is not a distinct parent was accepted")
	}
}
