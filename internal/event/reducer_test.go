package event

import (
	"encoding/json"
	"errors"
	"math/rand/v2"
	"reflect"
	"sort"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

func TestReduceIsIdempotentAndInputOrderIndependent(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Name: "peer"}, nil, 1)
	outbound := mailboxGrant(t, installationB, secretB, installationA, secretA.PublicKeyHex(), mailboxHumanB, 2)
	inbound := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 3)
	question := signedText(t, TypeQuestion, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}, "question", "", nil, outbound.ID(), 4)
	answer := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "answer", question.ID(), []string{question.ID()}, inbound.ID(), 1)
	cancel := signedCancel(t, question.ID(), []string{question.ID()}, 8)
	raw := [][]byte{trust.Wire, outbound.Wire, inbound.Wire, question.Wire, answer.Wire, cancel.Wire, append(append([]byte(nil), answer.Wire...), '\n')}
	policy := localPolicy()
	want := Reduce(raw, policy)
	if len(want.Records) != 6 {
		t.Fatalf("record count = %d, want 6", len(want.Records))
	}
	if len(want.DisplayOrder) != 2 || want.DisplayOrder[0] != question.ID() || want.DisplayOrder[1] != answer.ID() {
		t.Fatalf("causal display order = %#v", want.DisplayOrder)
	}
	for range 100 {
		shuffled := append([][]byte(nil), raw...)
		rand.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })
		got := Reduce(shuffled, policy)
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("reduction changed after shuffle\nwant: %#v\ngot:  %#v", want, got)
		}
	}
}

func TestHarnessActivityProjectionIsDeterministicAndMessageInert(t *testing.T) {
	root := signedMessagePayload(t, Schema3, TextPayload{Body: "message"}, 10)
	first := signedActivity(t, activityPayload(domain.HarnessActivityPlan, "", ""), []string{root.ID()}, 1)
	secondPayload := activityPayload(domain.HarnessActivityPlan, "", "")
	secondPayload.Body, secondPayload.Sequence = "new plan", 2
	second := signedActivity(t, secondPayload, []string{first.ID()}, 2)
	command := signedActivity(t, activityPayload(domain.HarnessActivityCommand, "command", domain.HarnessActivityCompleted), []string{second.ID()}, 3)
	otherProviderPayload := secondPayload
	otherProviderPayload.Correlation.Provider = "other"
	otherProvider := signedActivity(t, otherProviderPayload, []string{root.ID()}, 4)
	otherMailbox := mustSign(t, Content{
		Schema: Schema3, Type: TypeHarnessActivity, InstallationID: installationA,
		Sender:  &MailboxAddress{InstallationID: installationA, MailboxID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d13"},
		Parents: []string{root.ID()}, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, secondPayload),
	}, time.Unix(5, 0), secretA)
	raw := wires(root, first, second, command, otherProvider, otherMailbox, second)
	want := Reduce(raw, localPolicy())
	if len(want.Messages) != 1 || len(want.Threads) != 1 || len(want.HarnessActivities) != 4 {
		t.Fatalf("activity changed message state or projection count: messages=%#v threads=%#v activities=%#v", want.Messages, want.Threads, want.HarnessActivities)
	}
	var foundPlan, foundCommand, foundOther bool
	for _, activity := range want.HarnessActivities {
		switch {
		case activity.Kind == domain.HarnessActivityPlan && activity.Correlation.Provider == "home-built" && activity.Sender.MailboxID == mailboxAgentA:
			foundPlan = activity.EventID == second.ID() && activity.Body == "new plan" && activity.Sequence == 2
		case activity.Kind == domain.HarnessActivityCommand:
			foundCommand = activity.EventID == command.ID()
		case activity.Correlation.Provider == "other":
			foundOther = true
		}
	}
	if !foundPlan || !foundCommand || !foundOther || len(want.ConversationOrder) != 6 || want.ConversationOrder[0] != root.ID() {
		t.Fatalf("activity projection/order = %#v order=%#v", want.HarnessActivities, want.ConversationOrder)
	}
	for range 50 {
		shuffled := append([][]byte(nil), raw...)
		rand.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })
		if got := Reduce(shuffled, localPolicy()); !reflect.DeepEqual(got, want) {
			t.Fatalf("activity reduction changed after reorder/duplicate\nwant=%#v\ngot=%#v", want.HarnessActivities, got.HarnessActivities)
		}
	}
}

func TestHarnessActivitySourceSequenceBreaksSameMillisecondTies(t *testing.T) {
	firstPayload := activityPayload(domain.HarnessActivityPlan, "", "")
	firstPayload.Body, firstPayload.OccurredAt, firstPayload.Sequence = "first", 1_000, 1
	secondPayload := firstPayload
	secondPayload.Body, secondPayload.Sequence = "second", 2
	first := signedActivity(t, firstPayload, nil, 1)
	second := signedActivity(t, secondPayload, nil, 1)

	state := Reduce(wires(second, first), localPolicy())
	if len(state.HarnessActivities) != 1 {
		t.Fatalf("activity projections = %#v", state.HarnessActivities)
	}
	for _, activity := range state.HarnessActivities {
		if activity.EventID != second.ID() || activity.Sequence != 2 || activity.Body != "second" {
			t.Fatalf("same-time winner = %#v", activity)
		}
	}
}

func TestHarnessActivityUnsupportedAndAccountAuthorization(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	payload := activityPayload(domain.HarnessActivityProgress, "progress", domain.HarnessActivityRunning)
	accountActivity := mustSign(t, Content{
		Schema: Schema3, Type: TypeHarnessActivity, InstallationID: installationB,
		Sender:   &MailboxAddress{InstallationID: installationB, MailboxID: mailboxAgentA},
		Audience: &Audience{HumanAccountID: accountA}, Parents: []string{accept.ID()}, Authorities: []string{accept.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, payload),
	}, time.Unix(5, 0), secretB)
	state := Reduce(wires(create, grant, accept, accountActivity), localPolicy())
	if state.Records[accountActivity.ID()].Status != StatusProjected || len(state.HarnessActivities) != 1 {
		t.Fatalf("active account activity = %#v %#v", state.Records[accountActivity.ID()], state.HarnessActivities)
	}
	wrong := accountActivity.Content
	wrong.Audience = &Audience{HumanAccountID: "0198c7ec-73b0-7cc3-a5f7-e31c77140d22"}
	wrong.Payload = mustPayload(t, payload)
	wrongRaw := mustSignSchema(t, wrong, secretB, 6)
	wrongState := Reduce(append(wires(create, grant, accept), wrongRaw), localPolicy())
	for _, record := range wrongState.Records {
		if record.Event.Content.Type == TypeHarnessActivity && record.Status != StatusUnauthorized {
			t.Fatalf("unrelated account activity = %#v", record)
		}
	}
	revoke := mustSign(t, Content{
		Type: TypeHumanDeviceRevoke, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA}, Parents: uniqueSortedStrings([]string{grant.ID(), accept.ID()}), Authorities: []string{grant.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, humanDevicePayload()),
	}, time.Unix(7, 0), secretA)
	revokedContent := accountActivity.Content
	revokedContent.Parents = []string{revoke.ID()}
	revokedContent.Payload = mustPayload(t, payload)
	revokedRaw := mustSignSchema(t, revokedContent, secretB, 8)
	revokedState := Reduce(append(wires(create, grant, accept, revoke), revokedRaw), localPolicy())
	for _, record := range revokedState.Records {
		if record.Event.Content.Type == TypeHarnessActivity && record.Status != StatusUnauthorized {
			t.Fatalf("revoked account activity = %#v", record)
		}
	}
}

func signedActivity(t *testing.T, payload HarnessActivityPayload, parents []string, second int64) SignedEvent {
	t.Helper()
	return mustSign(t, Content{
		Schema: Schema3, Type: TypeHarnessActivity, InstallationID: installationA,
		Sender:  &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Parents: parents, Scope: ScopeInstallationPrivate, Payload: mustPayload(t, payload),
	}, time.Unix(second, 0), secretA)
}

func TestMultipleAnswersAndCancellationRelationsUseCausality(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	outbound := mailboxGrant(t, installationB, secretB, installationA, secretA.PublicKeyHex(), mailboxHumanB, 2)
	inbound := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 3)
	question := signedText(t, TypeQuestion, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}, "question", "", nil, outbound.ID(), 50)
	answerBefore := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "before", question.ID(), []string{question.ID()}, inbound.ID(), 90)
	cancelAfter := signedCancel(t, question.ID(), []string{answerBefore.ID()}, 1)
	cancelBefore := signedCancel(t, question.ID(), []string{question.ID()}, 100)
	answerAfter := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "after", question.ID(), []string{cancelBefore.ID()}, inbound.ID(), 2)
	concurrentAnswer := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "concurrent", question.ID(), []string{question.ID()}, inbound.ID(), 3)

	state := Reduce(wires(trust, outbound, inbound, question, answerBefore, cancelAfter, cancelBefore, answerAfter, concurrentAnswer), localPolicy())
	thread := state.Threads[question.ID()]
	if !thread.Answered || !thread.Cancelled || len(thread.AnswerIDs) != 3 || len(thread.CancellationIDs) != 2 {
		t.Fatalf("thread facts = %#v", thread)
	}
	if got := thread.AnswerCancellation[answerBefore.ID()][cancelAfter.ID()]; got != AnswerBeforeCancellation {
		t.Fatalf("answer-before relation = %q", got)
	}
	if got := thread.AnswerCancellation[answerAfter.ID()][cancelBefore.ID()]; got != AnswerAfterCancellation {
		t.Fatalf("answer-after relation = %q", got)
	}
	if got := thread.AnswerCancellation[concurrentAnswer.ID()][cancelBefore.ID()]; got != AnswerConcurrent {
		t.Fatalf("concurrent relation = %q", got)
	}
	// Signed time runs opposite to two causal edges above; the facts must not change.
	if state.Records[cancelAfter.ID()].Event.Nostr.CreatedAt >= state.Records[answerBefore.ID()].Event.Nostr.CreatedAt {
		t.Fatal("test does not contain the intended clock inversion")
	}
}

func TestMissingParentIsVisibleButDoesNotAnswerQuestion(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	outbound := mailboxGrant(t, installationB, secretB, installationA, secretA.PublicKeyHex(), mailboxHumanB, 2)
	inbound := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 3)
	question := signedText(t, TypeQuestion, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}, "missing for now", "", nil, outbound.ID(), 4)
	answer := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "orphan", question.ID(), []string{question.ID()}, inbound.ID(), 5)
	state := Reduce(wires(trust, outbound, inbound, answer), localPolicy())
	if got := state.Records[answer.ID()].Status; got != StatusUnresolved {
		t.Fatalf("answer status = %q", got)
	}
	got, err := state.Get(answer.ID())
	if err != nil || !got.Incomplete {
		t.Fatalf("get unresolved = %#v, %v", got, err)
	}
	polled := state.Poll(MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA})
	if len(polled) != 1 || polled[0].ID != answer.ID() || !polled[0].Incomplete {
		t.Fatalf("poll unresolved = %#v", polled)
	}
	if _, err := state.Wait(question.ID(), MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}); !errors.Is(err, ErrNotFound) {
		t.Fatalf("wait missing question = %v", err)
	}
	if _, exists := state.Threads[question.ID()]; exists {
		t.Fatal("unseen question was projected as answered")
	}
	resolved := Reduce(wires(trust, outbound, inbound, question, answer), localPolicy())
	if resolved.Records[answer.ID()].Status != StatusProjected || !resolved.Threads[question.ID()].Answered {
		t.Fatalf("resolved state = %#v, %#v", resolved.Records[answer.ID()], resolved.Threads[question.ID()])
	}
}

func TestWaitRequiresQuestionOwnershipAndReturnsFirstDisplayedAnswer(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	outbound := mailboxGrant(t, installationB, secretB, installationA, secretA.PublicKeyHex(), mailboxHumanB, 2)
	inbound := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 3)
	question := signedText(t, TypeQuestion, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}, "question", "", nil, outbound.ID(), 10)
	later := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "later", question.ID(), []string{question.ID()}, inbound.ID(), 30)
	earlier := signedText(t, TypeAnswer, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "earlier", question.ID(), []string{question.ID()}, inbound.ID(), 20)
	state := Reduce(wires(later, trust, outbound, inbound, question, earlier), localPolicy())
	got, err := state.Wait(question.ID(), MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA})
	if err != nil || got.ID != earlier.ID() {
		t.Fatalf("wait = %#v, %v", got, err)
	}
	if _, err := state.Wait(question.ID(), MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}); !errors.Is(err, ErrWaitDenied) {
		t.Fatalf("wait from wrong mailbox = %v", err)
	}
	if !state.Messages[question.ID()].PeerReceived {
		t.Fatal("causal answer did not prove peer receipt")
	}
}

func TestMailboxCapabilitiesAreDirectionalAndRevocationIsRemoveWins(t *testing.T) {
	binding := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	grant := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 2)
	authorized := remoteMessage(t, mailboxAgentA, grant.ID(), 3)
	wrongMailbox := remoteMessage(t, mailboxHumanA, grant.ID(), 4)

	state := Reduce(wires(binding, grant, authorized, wrongMailbox), localPolicy())
	if state.Records[authorized.ID()].Status != StatusProjected || state.Records[wrongMailbox.ID()].Status != StatusUnauthorized {
		t.Fatalf("directional access = %#v, %#v", state.Records[authorized.ID()], state.Records[wrongMailbox.ID()])
	}

	payload := MailboxAccessPayload{MailboxID: mailboxAgentA, GranteeInstallationID: installationB, GranteeSignerKeyID: secretB.PublicKeyHex()}
	revokeConcurrent := mustSign(t, Content{
		Type: TypeMailboxAccessRevoke, InstallationID: installationA,
		Sender: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}, Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Parents: []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: ScopePeerAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(5, 0), secretA)
	concurrent := Reduce(wires(binding, grant, authorized, revokeConcurrent), localPolicy())
	if concurrent.Records[authorized.ID()].Status != StatusUnauthorized {
		t.Fatalf("concurrent revoke did not win: %#v", concurrent.Records[authorized.ID()])
	}

	observation := mustSign(t, Content{
		Type: TypeMailboxAccessObserve, InstallationID: installationA,
		Sender: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}, Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Parents: uniqueSortedStrings([]string{grant.ID(), authorized.ID()}), Authorities: []string{grant.ID()}, Scope: ScopePeerAddressed,
		Payload: mustPayload(t, MailboxAccessObservationPayload{GrantEventID: grant.ID(), MessageEventID: authorized.ID()}),
	}, time.Unix(6, 0), secretA)
	revokeAfter := mustSign(t, Content{
		Type: TypeMailboxAccessRevoke, InstallationID: installationA,
		Sender: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}, Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Parents: uniqueSortedStrings([]string{grant.ID(), observation.ID()}), Authorities: []string{grant.ID()}, Scope: ScopePeerAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(7, 0), secretA)
	blocked := localControl(t, TypePeerBindingBlock, PeerPayload{InstallationID: installationB}, []string{binding.ID()}, 8)
	historical := Reduce(wires(binding, grant, authorized, observation, revokeAfter, blocked), localPolicy())
	if historical.Peers[installationB].Trusted || historical.Records[authorized.ID()].Status != StatusProjected {
		t.Fatalf("historical authorization changed after revoke/block: %#v, %#v", historical.Peers[installationB], historical.Records[authorized.ID()])
	}
}

func TestConcurrentPeerBindingAndBlockFailsClosed(t *testing.T) {
	trust := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	distrust := localControl(t, TypePeerBindingBlock, PeerPayload{InstallationID: installationB}, nil, 2)
	state := Reduce(wires(trust, distrust), localPolicy())
	if state.Peers[installationB].Trusted {
		t.Fatal("concurrent binding and block left peer routable")
	}
}

func TestMailboxObservationProjectionKeepsOnlyTheCausalFrontier(t *testing.T) {
	binding := localControl(t, TypePeerBindingSet, PeerPayload{InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex()}, nil, 1)
	grant := mailboxGrant(t, installationA, secretA, installationB, secretB.PublicKeyHex(), mailboxAgentA, 2)
	events := []SignedEvent{binding, grant}
	var priorObservation string
	for index := int64(0); index < 70; index++ {
		message := remoteMessage(t, mailboxAgentA, grant.ID(), 10+index)
		parents := uniqueSortedStrings([]string{grant.ID(), message.ID(), priorObservation})
		observation := mustSign(t, Content{
			Type: TypeMailboxAccessObserve, InstallationID: installationA,
			Sender: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}, Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
			Parents: parents, Authorities: []string{grant.ID()}, Scope: ScopePeerAddressed,
			Payload: mustPayload(t, MailboxAccessObservationPayload{GrantEventID: grant.ID(), MessageEventID: message.ID()}),
		}, time.Unix(100+index, 0), secretA)
		events = append(events, message, observation)
		priorObservation = observation.ID()
	}
	state := Reduce(wires(events...), localPolicy())
	if got := state.MailboxAccess[grant.ID()].ObservationEventIDs; !reflect.DeepEqual(got, []string{priorObservation}) {
		t.Fatalf("observation frontier = %#v", got)
	}
}

func TestHumanAccountDeviceMembershipConvergesAcrossInputOrder(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	selectAccount := localControl(t, TypeHumanAccountSelect, HumanAccountSelectionPayload{AccountID: accountA}, []string{create.ID()}, 4)
	raw := wires(create, grant, accept, selectAccount)
	want := Reduce(raw, localPolicy())
	device := want.Accounts[accountA].Devices[installationB]
	if !device.Active || device.AcceptEventID != accept.ID() || want.DefaultAccountID != accountA {
		t.Fatalf("membership = %#v, default = %q", device, want.DefaultAccountID)
	}
	for range 100 {
		shuffled := append([][]byte(nil), raw...)
		rand.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })
		if got := Reduce(shuffled, localPolicy()); !reflect.DeepEqual(got, want) {
			t.Fatalf("account reduction changed after shuffle\nwant: %#v\ngot: %#v", want, got)
		}
	}
}

func TestHumanAccountSelectionDoesNotInferAuthorityFromArbitraryDescendant(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	selection := localControl(t, TypeHumanAccountSelect, HumanAccountSelectionPayload{AccountID: accountA}, []string{accept.ID()}, 4)
	state := Reduce(wires(create, grant, accept, selection), localPolicy())
	if state.DefaultAccountID != "" {
		t.Fatalf("selection inferred creator authority from device acceptance: %q", state.DefaultAccountID)
	}
}

func TestHumanAccountRequiresGrantAndInvitedKey(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	withoutGrant := Reduce(wires(create, accept), localPolicy())
	if withoutGrant.Records[accept.ID()].Status != StatusUnresolved {
		t.Fatalf("acceptance without grant = %#v", withoutGrant.Records[accept.ID()])
	}

	forgedPayload := HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop"}
	forgedRaw := mustSignSchema(t, Content{
		Schema: SchemaVersion,
		Type:   TypeHumanDeviceGrant, InstallationID: installationB,
		SignerKeyID: secretB.PublicKeyHex(),
		Sender:      &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Recipient:   &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Audience:    &Audience{HumanAccountID: accountA},
		Parents:     []string{create.ID()}, Authorities: []string{create.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, forgedPayload),
	}, secretB, 5)
	state := Reduce(append(wires(create), forgedRaw), localPolicy())
	if len(state.Invalid) != 1 || state.Invalid[0].Status != StatusInvalid {
		t.Fatalf("forged grant invalid records = %#v", state.Invalid)
	}
	if !Reduce(wires(create, grant, accept), localPolicy()).Accounts[accountA].Devices[installationB].Active {
		t.Fatal("valid invited-key acceptance was not active")
	}
	changed := humanDevicePayload()
	changed.Label = "forged label"
	changedAcceptance := mustSign(t, Content{
		Type: TypeHumanDeviceAccept, InstallationID: installationB,
		Sender:    &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA},
		Parents:   []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, changed),
	}, time.Unix(6, 0), secretB)
	changedState := Reduce(wires(create, grant, changedAcceptance), localPolicy())
	if changedState.Records[changedAcceptance.ID()].Status != StatusUnresolved || changedState.Accounts[accountA].Devices[installationB].Active {
		t.Fatalf("changed acceptance = %#v, %#v", changedState.Records[changedAcceptance.ID()], changedState.Accounts[accountA].Devices[installationB])
	}
}

func TestConflictingHumanAccountCreationFailsClosed(t *testing.T) {
	create, _, _ := humanMembershipEvents(t)
	conflict := mustSign(t, Content{
		Type: TypeHumanAccountCreate, InstallationID: installationB, Scope: ScopeInstallationPrivate,
		Payload: mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationB, CreatorSignerKeyID: secretB.PublicKeyHex(), Label: "desktop"}),
	}, time.Unix(2, 0), secretB)
	state := Reduce(wires(create, conflict), localPolicy())
	if _, ok := state.Accounts[accountA]; ok || state.Records[create.ID()].Status != StatusUnresolved || state.Records[conflict.ID()].Status != StatusUnresolved {
		t.Fatalf("conflicting account = %#v, %#v, %#v", state.Accounts[accountA], state.Records[create.ID()], state.Records[conflict.ID()])
	}
}

func TestHumanDeviceRevokeAndConcurrentAcceptFailClosed(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	payload := humanDevicePayload()
	revokeAfter := mustSign(t, Content{
		Type: TypeHumanDeviceRevoke, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA},
		Parents:   uniqueSortedStrings([]string{grant.ID(), accept.ID()}), Authorities: []string{grant.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(5, 0), secretA)
	after := Reduce(wires(create, grant, accept, revokeAfter), localPolicy())
	if after.Accounts[accountA].Devices[installationB].Active {
		t.Fatal("later revocation left device active")
	}

	concurrentRevoke := mustSign(t, Content{
		Type: TypeHumanDeviceRevoke, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA},
		Parents:   []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(6, 0), secretA)
	concurrent := Reduce(wires(create, grant, accept, concurrentRevoke), localPolicy())
	if concurrent.Accounts[accountA].Devices[installationB].Active {
		t.Fatal("concurrent acceptance and revocation left device active")
	}
}

func TestAccountQuestionAnswerAndCancellationConverge(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	question := mustSign(t, Content{
		Type: TypeQuestion, InstallationID: installationB,
		Sender:   &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Audience: &Audience{HumanAccountID: accountA}, Parents: []string{accept.ID()}, Authorities: []string{accept.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, TextPayload{Body: "account question", ActorLabel: "desktop agent"}),
	}, time.Unix(4, 0), secretB)
	answerParents := []string{create.ID(), question.ID()}
	sort.Strings(answerParents)
	answer := mustSign(t, Content{
		Type: TypeAnswer, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Audience:  &Audience{HumanAccountID: accountA}, ThreadID: question.ID(), Parents: answerParents, Authorities: []string{create.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, TextPayload{Body: "account answer", ActorLabel: "laptop"}),
	}, time.Unix(5, 0), secretA)
	cancelParents := []string{accept.ID(), question.ID()}
	sort.Strings(cancelParents)
	cancel := mustSign(t, Content{
		Type: TypeThreadCancel, InstallationID: installationB,
		Sender:   &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Audience: &Audience{HumanAccountID: accountA}, ThreadID: question.ID(), Parents: cancelParents, Authorities: []string{accept.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, TargetPayload{Reason: "no longer needed"}),
	}, time.Unix(6, 0), secretB)
	reject := mustSign(t, Content{
		Type: TypeMessageReject, InstallationID: installationA,
		Sender:   &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Audience: &Audience{HumanAccountID: accountA}, Parents: answerParents, Authorities: []string{create.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, TargetPayload{TargetEventID: question.ID(), Reason: "not for this account"}),
	}, time.Unix(7, 0), secretA)
	want := Reduce(wires(create, grant, accept, question, answer, cancel, reject), localPolicy())
	thread := want.Threads[question.ID()]
	if !thread.Answered || !thread.Cancelled || thread.AnswerCancellation[answer.ID()][cancel.ID()] != AnswerConcurrent {
		t.Fatalf("account thread = %#v", thread)
	}
	if got := want.Messages[question.ID()]; got.Recipient.InstallationID != installationA || got.Recipient.MailboxID != mailboxHumanA || got.AudienceAccountID != accountA {
		t.Fatalf("local account inbox projection = %#v", got)
	}
	if got := want.Messages[question.ID()]; !got.Rejected || !got.Archived {
		t.Fatalf("account rejection = %#v", got)
	}
	for range 100 {
		shuffled := wires(create, grant, accept, question, answer, cancel, reject)
		rand.Shuffle(len(shuffled), func(i, j int) { shuffled[i], shuffled[j] = shuffled[j], shuffled[i] })
		if got := Reduce(shuffled, localPolicy()); !reflect.DeepEqual(got, want) {
			t.Fatalf("account state changed after shuffle\nwant: %#v\ngot: %#v", want, got)
		}
	}
}

func TestAccountTrafficNeedsMembershipInTheNamedAccount(t *testing.T) {
	create, grant, accept := humanMembershipEvents(t)
	otherAccount := "0198c7ec-73b0-7cc3-a5f7-e31c77140d22"
	question := mustSign(t, Content{
		Type: TypeQuestion, InstallationID: installationB,
		Sender:   &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		Audience: &Audience{HumanAccountID: otherAccount}, Parents: []string{accept.ID()}, Authorities: []string{accept.ID()}, Scope: ScopeAccountAddressed,
		Payload: mustPayload(t, TextPayload{Body: "wrong account"}),
	}, time.Unix(4, 0), secretB)
	state := Reduce(wires(create, grant, accept, question), localPolicy())
	if state.Records[question.ID()].Status != StatusUnauthorized {
		t.Fatalf("wrong-account question = %#v", state.Records[question.ID()])
	}
}

func humanMembershipEvents(t *testing.T) (SignedEvent, SignedEvent, SignedEvent) {
	t.Helper()
	create := mustSign(t, Content{
		Type: TypeHumanAccountCreate, InstallationID: installationA, Scope: ScopeInstallationPrivate,
		Payload: mustPayload(t, HumanAccountPayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), Label: "laptop"}),
	}, time.Unix(1, 0), secretA)
	payload := humanDevicePayload()
	grant := mustSign(t, Content{
		Type: TypeHumanDeviceGrant, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA},
		Parents:   []string{create.ID()}, Authorities: []string{create.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(2, 0), secretA)
	accept := mustSign(t, Content{
		Type: TypeHumanDeviceAccept, InstallationID: installationB,
		Sender:    &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Audience:  &Audience{HumanAccountID: accountA},
		Parents:   []string{grant.ID()}, Authorities: []string{grant.ID()}, Scope: ScopeAccountAddressed, Payload: mustPayload(t, payload),
	}, time.Unix(3, 0), secretB)
	return create, grant, accept
}

func humanDevicePayload() HumanDevicePayload {
	return HumanDevicePayload{AccountID: accountA, CreatorInstallationID: installationA, CreatorSignerKeyID: secretA.PublicKeyHex(), InstallationID: installationB, SignerKeyID: secretB.PublicKeyHex(), Label: "desktop", Relays: []string{"wss://relay.example"}}
}

func TestArchiveAndRejectRetainCanonicalMessage(t *testing.T) {
	message := signedText(t, TypeMessage, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "message", "", nil, "", 1)
	archive := localMessageState(t, TypeMessageArchive, message.ID(), []string{message.ID()}, 2)
	reject := localMessageState(t, TypeMessageReject, message.ID(), []string{archive.ID()}, 3)
	state := Reduce(wires(message, archive, reject), localPolicy())
	projected := state.Messages[message.ID()]
	if !projected.Archived || !projected.Rejected {
		t.Fatalf("message state = %#v", projected)
	}
	for _, id := range []string{message.ID(), archive.ID(), reject.ID()} {
		if _, ok := state.Records[id]; !ok {
			t.Fatalf("canonical event %s was removed", id)
		}
	}
}

func TestRestoreSupersedesArchiveAndCanBeArchivedAgain(t *testing.T) {
	message := signedText(t, TypeMessage, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "message", "", nil, "", 1)
	archive := localMessageState(t, TypeMessageArchive, message.ID(), []string{message.ID()}, 2)
	restore := localMessageState(t, TypeMessageRestore, message.ID(), []string{message.ID(), archive.ID()}, 3)
	state := Reduce(wires(message, archive, restore), localPolicy())
	if projected := state.Messages[message.ID()]; projected.Archived || !projected.ArchivedAt.IsZero() {
		t.Fatalf("restored message = %#v", projected)
	}
	rearchive := localMessageState(t, TypeMessageArchive, message.ID(), []string{message.ID(), restore.ID()}, 4)
	state = Reduce(wires(message, archive, restore, rearchive), localPolicy())
	if projected := state.Messages[message.ID()]; !projected.Archived || !projected.ArchivedAt.Equal(time.Unix(4, 0).UTC()) {
		t.Fatalf("rearchived message = %#v", projected)
	}
}

func TestMessageStateTargetMustBeItsCausalAncestor(t *testing.T) {
	first := signedText(t, TypeMessage, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "first", "", nil, "", 1)
	second := signedText(t, TypeMessage, installationA, secretA,
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}, "second", "", nil, "", 2)
	badArchive := localMessageState(t, TypeMessageArchive, first.ID(), []string{second.ID()}, 3)
	state := Reduce(wires(first, second, badArchive), localPolicy())
	if state.Records[badArchive.ID()].Status != StatusInvalid || state.Messages[first.ID()].Archived {
		t.Fatalf("bad archive = %#v, message = %#v", state.Records[badArchive.ID()], state.Messages[first.ID()])
	}
}

func TestSchema3MessageProjectsTypedSemanticsWithoutParsingDetails(t *testing.T) {
	payload := TextPayload{
		Body: "typed output", Details: "Kind: notice\nHarness session: user-authored",
		Presentation:      model.PresentationUpdate,
		Correlation:       model.MessageCorrelation{Provider: "home-built", SessionID: "session", OperationID: "operation", ItemID: "item"},
		TechnicalSections: []model.TechnicalSection{{Namespace: "vendor.experimental", Fields: []model.TechnicalField{{Key: "opaque", Label: "Opaque", Value: "value"}}}},
	}
	signed := signedMessagePayload(t, Schema3, payload, 1)
	state := Reduce(wires(signed, signed), localPolicy())
	projected := state.Messages[signed.ID()]
	if projected.Presentation != payload.Presentation || projected.Correlation != payload.Correlation || projected.Details != payload.Details || !reflect.DeepEqual(projected.TechnicalSections, payload.TechnicalSections) {
		t.Fatalf("schema 3 projection = %#v", projected)
	}
	if !reflect.DeepEqual(state.Records[signed.ID()].Event.Wire, signed.Wire) {
		t.Fatal("schema 3 canonical bytes changed during reduction")
	}
}

func signedMessagePayload(t *testing.T, schema int, payload TextPayload, second int64) SignedEvent {
	t.Helper()
	return mustSign(t, Content{
		Schema: schema, Type: TypeQuestion, InstallationID: installationA,
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Scope:     ScopeInstallationPrivate, Payload: mustPayload(t, payload),
	}, time.Unix(second, 0), secretA)
}

func TestMalformedKnownEventAndUnsupportedSchemaDoNotProject(t *testing.T) {
	malformed := Content{
		Schema: SchemaVersion, Type: TypeQuestion, InstallationID: installationA, SignerKeyID: secretA.PublicKeyHex(),
		Sender:    &MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA},
		Recipient: &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA},
		Parents:   []string{}, Scope: ScopeInstallationPrivate, Payload: json.RawMessage(`{"body":"hello","extra":true}`),
	}
	badWire := mustSignSchema(t, malformed, secretA, 1)
	unknown := malformed
	unknown.Schema = 4
	unknown.InstallationID = installationB
	unknown.SignerKeyID = secretB.PublicKeyHex()
	unknown.Sender = &MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB}
	unknown.Recipient = &MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}
	unknown.Scope = ScopePeerAddressed
	unknownWire := mustSignSchema(t, unknown, secretB, 2)

	state := Reduce([][]byte{badWire, unknownWire}, localPolicy())
	if len(state.Invalid) != 1 || len(state.Messages) != 0 {
		t.Fatalf("invalid projection = %#v, messages = %#v", state.Invalid, state.Messages)
	}
	inspection := Inspect(unknownWire)
	if state.Records[inspection.Event.ID()].Status != StatusUnsupported {
		t.Fatalf("unsupported schema event = %#v", state.Records[inspection.Event.ID()])
	}
}

func localPolicy() Policy {
	return Policy{InstallationID: installationA, RootKeyID: secretA.PublicKeyHex(), HumanMailboxID: mailboxHumanA}
}

func localControl(t *testing.T, kind Type, payload any, parents []string, second int64) SignedEvent {
	t.Helper()
	content := control(kind, mustPayload(t, payload))
	content.Parents = append([]string(nil), parents...)
	if kind == TypeHumanAccountSelect {
		content.Authorities = append([]string(nil), parents...)
	}
	return mustSign(t, content, time.Unix(second, 0), secretA)
}

func signedText(t *testing.T, kind Type, installation string, secret SecretKey, sender, recipient MailboxAddress, body, thread string, parents []string, authority string, second int64) SignedEvent {
	t.Helper()
	scope := ScopeInstallationPrivate
	if sender.InstallationID != recipient.InstallationID {
		scope = ScopePeerAddressed
	}
	authorities := []string(nil)
	if authority != "" {
		authorities = []string{authority}
		parents = append(parents, authority)
	}
	return mustSign(t, Content{
		Type: kind, InstallationID: installation, Sender: &sender, Recipient: &recipient,
		ThreadID: thread, Parents: uniqueSortedStrings(parents), Authorities: authorities, Scope: scope,
		Payload: mustPayload(t, TextPayload{Body: body}),
	}, time.Unix(second, 0), secret)
}

func signedCancel(t *testing.T, thread string, parents []string, second int64) SignedEvent {
	t.Helper()
	sender := MailboxAddress{InstallationID: installationA, MailboxID: mailboxAgentA}
	return mustSign(t, Content{
		Type: TypeThreadCancel, InstallationID: installationA, Sender: &sender, ThreadID: thread,
		Parents: append([]string(nil), parents...), Scope: ScopeInstallationPrivate,
		Payload: mustPayload(t, TargetPayload{Reason: "cancelled"}),
	}, time.Unix(second, 0), secretA)
}

func localMessageState(t *testing.T, kind Type, target string, parents []string, second int64) SignedEvent {
	t.Helper()
	sender := MailboxAddress{InstallationID: installationA, MailboxID: mailboxHumanA}
	return mustSign(t, Content{
		Type: kind, InstallationID: installationA, Sender: &sender, Parents: append([]string(nil), parents...),
		Scope: ScopeInstallationPrivate, Payload: mustPayload(t, TargetPayload{TargetEventID: target}),
	}, time.Unix(second, 0), secretA)
}

func remoteMessage(t *testing.T, recipientMailbox, grantID string, second int64) SignedEvent {
	t.Helper()
	return signedText(t, TypeMessage, installationB, secretB,
		MailboxAddress{InstallationID: installationB, MailboxID: mailboxHumanB},
		MailboxAddress{InstallationID: installationA, MailboxID: recipientMailbox}, "hello", "", nil, grantID, second)
}

func mailboxGrant(t *testing.T, grantorInstallation string, grantorSecret SecretKey, granteeInstallation, granteeSignerKeyID, mailboxID string, second int64) SignedEvent {
	t.Helper()
	return mustSign(t, Content{
		Type: TypeMailboxAccessGrant, InstallationID: grantorInstallation,
		Sender:    &MailboxAddress{InstallationID: grantorInstallation, MailboxID: mailboxHumanA},
		Recipient: &MailboxAddress{InstallationID: granteeInstallation, MailboxID: mailboxHumanA},
		Scope:     ScopePeerAddressed,
		Payload: mustPayload(t, MailboxAccessPayload{
			MailboxID: mailboxID, GranteeInstallationID: granteeInstallation, GranteeSignerKeyID: granteeSignerKeyID,
		}),
	}, time.Unix(second, 0), grantorSecret)
}

func uniqueSortedStrings(values []string) []string {
	seen := make(map[string]bool)
	result := make([]string, 0, len(values))
	for _, value := range values {
		if value != "" && !seen[value] {
			seen[value] = true
			result = append(result, value)
		}
	}
	sort.Strings(result)
	return result
}

func mustSignSchema(t *testing.T, content Content, secret SecretKey, second int64) []byte {
	t.Helper()
	rawContent, err := json.Marshal(content)
	if err != nil {
		t.Fatal(err)
	}
	event := NostrEvent{CreatedAt: second, Kind: Kind, Tags: [][]string{}, Content: string(rawContent)}
	if err := event.Sign(secret); err != nil {
		t.Fatal(err)
	}
	wire, err := json.Marshal(event)
	if err != nil {
		t.Fatal(err)
	}
	return wire
}

func wires(events ...SignedEvent) [][]byte {
	result := make([][]byte, len(events))
	for index, event := range events {
		result[index] = event.Wire
	}
	return result
}
