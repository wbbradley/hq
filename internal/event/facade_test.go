package event

import (
	"reflect"
	"testing"

	"github.com/wbbradley/hq/internal/eventstate"
)

func TestReduceFacadeIsThePureCore(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Name: "peer"}, nil, 1)
	outbound := mailboxGrant(t, installationB, secretB, installationA, secretA.PublicKeyHex(), mailboxHumanB, 2)
	inbound := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 3)
	question := signedText(t, TypeQuestion, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}, "question", "", nil, outbound.ID(), 4)
	answer := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "answer", question.ID(), []string{question.ID()}, inbound.ID(), 5)
	raw := wires(answer, trust, question, outbound, inbound, answer)
	if got, want := Reduce(raw, localPolicy()), eventstate.Reduce(raw, localPolicy()); !reflect.DeepEqual(got, want) {
		t.Fatalf("event facade diverged from pure core\ngot:  %#v\nwant: %#v", got, want)
	}
}
