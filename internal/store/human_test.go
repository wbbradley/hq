package store

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

func TestPairingBundleFixtureAndMalformedInputs(t *testing.T) {
	creatorID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d01"
	targetID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d02"
	accountID := "0198c7ec-73b0-7cc3-a5f7-e31c77140d21"
	creatorKey := event.MustSecretKeyFromHex("1")
	targetKey := event.MustSecretKeyFromHex("2")
	accountPayload, _ := event.MarshalPayload(event.HumanAccountPayload{AccountID: accountID, CreatorInstallationID: creatorID, CreatorSignerKeyID: creatorKey.PublicKeyHex(), Label: "laptop"})
	created, err := event.Sign(event.Content{Type: event.TypeHumanAccountCreate, InstallationID: creatorID, Scope: event.ScopeInstallationPrivate, Payload: accountPayload}, time.Unix(1_700_000_000, 0), creatorKey)
	if err != nil {
		t.Fatal(err)
	}
	device := event.HumanDevicePayload{AccountID: accountID, CreatorInstallationID: creatorID, CreatorSignerKeyID: creatorKey.PublicKeyHex(), InstallationID: targetID, SignerKeyID: targetKey.PublicKeyHex(), Label: "desktop", Relays: []string{"wss://relay.example"}, CreatorRelays: []string{"wss://relay.example"}}
	devicePayload, _ := event.MarshalPayload(device)
	granted, err := event.Sign(event.Content{Type: event.TypeHumanDeviceGrant, InstallationID: creatorID, Sender: &event.MailboxAddress{InstallationID: creatorID, MailboxID: model.HumanMailboxID}, Recipient: &event.MailboxAddress{InstallationID: targetID, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: accountID}, Parents: []string{created.ID()}, Authorities: []string{created.ID()}, Scope: event.ScopeAccountAddressed, Payload: devicePayload}, time.Unix(1_700_000_001, 0), creatorKey)
	if err != nil {
		t.Fatal(err)
	}
	bundle := PairingBundle{Version: 2, AccountID: accountID, AccountLabel: "laptop", CreatorInstallationID: creatorID, CreatorSignerKeyID: creatorKey.PublicKeyHex(), CreatorRelays: []string{"wss://relay.example"}, TargetInstallationID: targetID, TargetSignerKeyID: targetKey.PublicKeyHex(), TargetLabel: "desktop", TargetRelays: []string{"wss://relay.example"}, AccountCreationEvent: created.Wire, DeviceGrantEvent: granted.Wire, AccountAuthorityEvents: [][]byte{created.Wire, granted.Wire}}
	raw, _ := json.Marshal(bundle)
	const wantDigest = "a8667c4fe93412361d901200c8af77bce6385f1b0afd6ccc9b487a38b253eb6b"
	if got := fmt.Sprintf("%x", sha256.Sum256(raw)); got != wantDigest {
		t.Fatalf("bundle fixture digest = %s\nraw = %s", got, raw)
	}
	if parsed, parsedCreate, parsedGrant, history, parsedDevice, err := inspectPairingBundle(raw); err != nil || parsed.AccountID != accountID || !bytes.Equal(parsedCreate.Wire, created.Wire) || !bytes.Equal(parsedGrant.Wire, granted.Wire) || len(history) != 2 || parsedDevice.Label != device.Label {
		t.Fatalf("inspect fixture = %#v %#v %#v %#v %#v, %v", parsed, parsedCreate, parsedGrant, history, parsedDevice, err)
	}

	tests := []struct {
		name string
		raw  func() []byte
	}{
		{"unsupported version", func() []byte { changed := bundle; changed.Version = 3; value, _ := json.Marshal(changed); return value }},
		{"changed target", func() []byte {
			changed := bundle
			changed.TargetLabel = "other"
			value, _ := json.Marshal(changed)
			return value
		}},
		{"missing account event", func() []byte {
			changed := bundle
			changed.AccountCreationEvent = nil
			value, _ := json.Marshal(changed)
			return value
		}},
		{"swapped signed events", func() []byte {
			changed := bundle
			changed.AccountCreationEvent, changed.DeviceGrantEvent = changed.DeviceGrantEvent, changed.AccountCreationEvent
			value, _ := json.Marshal(changed)
			return value
		}},
		{"unknown field", func() []byte { return []byte(strings.TrimSuffix(string(raw), "}") + `,"extra":true}`) }},
		{"trailing data", func() []byte { return append(append([]byte(nil), raw...), []byte(" true")...) }},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if _, _, _, _, _, err := inspectPairingBundle(test.raw()); err == nil {
				t.Fatal("malformed bundle was accepted")
			}
		})
	}
}

func TestHumanAccountPairingIsSignedIdempotentAndRebuildable(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	invited := openStore(t, filepath.Join(t.TempDir(), "invited", "hq.db"))
	creatorID, creatorKey := creator.InstallationIdentity()
	invitedID, invitedKey := invited.InstallationIdentity()
	if creatorID == invitedID || creatorKey == invitedKey {
		t.Fatal("isolated installations shared identity")
	}
	if err := creator.AddRelay(ctx, RelayConfig{URL: "ws://relay.lan:7447", Read: true, Write: true, UnsafeNoAuth: true}); err != nil {
		t.Fatal(err)
	}

	request := HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop", Relays: []string{"ws://relay.lan:7447"}}
	bundle, err := creator.CreateHumanInvite(ctx, request)
	if err != nil {
		t.Fatal(err)
	}
	repeated, err := creator.CreateHumanInvite(ctx, request)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(bundle.AccountCreationEvent, repeated.AccountCreationEvent) || !bytes.Equal(bundle.DeviceGrantEvent, repeated.DeviceGrantEvent) {
		t.Fatal("repeated invite changed its exact signed events")
	}
	creatorDevices, err := creator.HumanDevices(ctx)
	if err != nil {
		t.Fatal(err)
	}
	pending := false
	for _, device := range creatorDevices {
		pending = pending || device.InstallationID == invitedID && device.State == "pending"
	}
	if !pending {
		t.Fatalf("creator devices before acceptance = %#v", creatorDevices)
	}
	raw, err := json.Marshal(bundle)
	if err != nil {
		t.Fatal(err)
	}
	if err := invited.JoinHumanInvite(ctx, raw); err != nil {
		t.Fatal(err)
	}
	if err := invited.RevokeHumanDevice(ctx, creatorID); err == nil || !strings.Contains(err.Error(), "only the account creator") {
		t.Fatalf("non-creator revoke error = %v", err)
	}
	var countAfterJoin int
	if err := invited.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&countAfterJoin); err != nil {
		t.Fatal(err)
	}
	if err := invited.JoinHumanInvite(ctx, raw); err != nil {
		t.Fatal(err)
	}
	var countAfterRepeat int
	if err := invited.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&countAfterRepeat); err != nil || countAfterRepeat != countAfterJoin {
		t.Fatalf("repeat join event count = %d, want %d: %v", countAfterRepeat, countAfterJoin, err)
	}
	creatorAccount, err := creator.HumanAccount(ctx)
	if err != nil {
		t.Fatal(err)
	}
	invitedAccount, err := invited.HumanAccount(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if creatorAccount.ID != invitedAccount.ID || invitedAccount.Creator || invitedAccount.CreatorInstallationID != creatorID {
		t.Fatalf("creator=%#v invited=%#v", creatorAccount, invitedAccount)
	}
	devices, err := invited.HumanDevices(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(devices) != 2 || devices[1].InstallationID != invitedID && devices[0].InstallationID != invitedID {
		t.Fatalf("joined devices = %#v", devices)
	}
	var active int
	if err := invited.db.QueryRow(`SELECT count(*) FROM human_account_devices WHERE account_id=? AND installation_id=? AND state='active'`, creatorAccount.ID, invitedID).Scan(&active); err != nil || active != 1 {
		t.Fatalf("invited membership active=%d, %v", active, err)
	}

	acceptance := canonicalEventByType(t, invited, event.TypeHumanDeviceAccept)
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{acceptance}); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{acceptance}); err != nil {
		t.Fatalf("repeat acceptance delivery: %v", err)
	}
	if err := creator.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if err := creator.db.QueryRow(`SELECT count(*) FROM human_account_devices WHERE account_id=? AND installation_id=? AND state='active'`, creatorAccount.ID, invitedID).Scan(&active); err != nil || active != 1 {
		t.Fatalf("creator accepted membership active=%d, %v", active, err)
	}
	if err := invited.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	if rebuilt, err := invited.HumanAccount(ctx); err != nil || rebuilt.ID != creatorAccount.ID {
		t.Fatalf("rebuilt invited account = %#v, %v", rebuilt, err)
	}
}

func TestJoinHumanInviteValidatesBeforeWriting(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	invited := openStore(t, filepath.Join(t.TempDir(), "invited", "hq.db"))
	invitedID, invitedKey := invited.InstallationIdentity()
	bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop"})
	if err != nil {
		t.Fatal(err)
	}
	bundle.TargetLabel = "tampered"
	raw, _ := json.Marshal(bundle)
	var before int
	_ = invited.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&before)
	if err := invited.JoinHumanInvite(ctx, raw); err == nil {
		t.Fatal("tampered invite was accepted")
	}
	var after int
	_ = invited.db.QueryRow(`SELECT count(*) FROM canonical_events`).Scan(&after)
	if after != before {
		t.Fatalf("invalid invite wrote events: before=%d after=%d", before, after)
	}

	bundle.TargetLabel = "desktop"
	bundle.TargetInstallationID = creator.signer.InstallationID
	raw, _ = json.Marshal(bundle)
	if err := invited.JoinHumanInvite(ctx, raw); err == nil {
		t.Fatal("wrong-target invite was accepted")
	}
	other := openStore(t, filepath.Join(t.TempDir(), "other", "hq.db"))
	otherID, otherKey := other.InstallationIdentity()
	otherBundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: otherID, SignerKeyID: otherKey, Name: "other"})
	if err != nil {
		t.Fatal(err)
	}
	otherRaw, _ := json.Marshal(otherBundle)
	if err := invited.JoinHumanInvite(ctx, otherRaw); err == nil || !strings.Contains(err.Error(), "another installation or key") {
		t.Fatalf("valid invite for another installation error = %v", err)
	}
}

func TestHumanDeviceRevocationIsIdempotentAndProjectsRemotely(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	invited := openStore(t, filepath.Join(t.TempDir(), "invited", "hq.db"))
	invitedID, invitedKey := invited.InstallationIdentity()
	bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop"})
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := json.Marshal(bundle)
	if err := invited.JoinHumanInvite(ctx, raw); err != nil {
		t.Fatal(err)
	}
	if err := creator.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByType(t, invited, event.TypeHumanDeviceAccept)}); err != nil {
		t.Fatal(err)
	}
	if err := creator.RevokeHumanDevice(ctx, invitedID); err != nil {
		t.Fatal(err)
	}
	var before int
	_ = creator.db.QueryRow(`SELECT count(*) FROM canonical_events WHERE event_type='human.device.revoke'`).Scan(&before)
	if err := creator.RevokeHumanDevice(ctx, invitedID); err != nil {
		t.Fatal(err)
	}
	var after int
	_ = creator.db.QueryRow(`SELECT count(*) FROM canonical_events WHERE event_type='human.device.revoke'`).Scan(&after)
	if before != 1 || after != before {
		t.Fatalf("revoke counts before=%d after=%d", before, after)
	}
	revocation := canonicalEventByType(t, creator, event.TypeHumanDeviceRevoke)
	acceptance := canonicalEventByTypeAndInstallation(t, invited, event.TypeHumanDeviceAccept, invitedID)
	if err := invited.AppendCanonical(ctx, []event.SignedEvent{revocation}); err != nil {
		t.Fatal(err)
	}
	var state string
	if err := invited.db.QueryRow(`SELECT state FROM human_account_devices WHERE account_id=? AND installation_id=?`, bundle.AccountID, invitedID).Scan(&state); err != nil || state != "revoked" {
		t.Fatalf("remote device state = %q, %v", state, err)
	}
	if _, err := invited.HumanAccount(ctx); err == nil {
		t.Fatal("revoked installation retained an active default account")
	}
	agent, err := invited.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "revoked-agent"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	text, _ := event.MarshalPayload(event.TextPayload{MessageID: "0198c7ec-73b0-7cc3-a5f7-e31c77140df2", Body: "late revoked message"})
	late, err := invited.signer.Sign(ctx, event.Content{Type: event.TypeQuestion, Sender: invited.localAddress(agent.ID), Audience: &event.Audience{HumanAccountID: bundle.AccountID}, Parents: uniqueSorted([]string{acceptance.ID(), revocation.ID()}), Authorities: []string{acceptance.ID()}, Scope: event.ScopeAccountAddressed, Payload: text}, time.Now().UTC())
	if err != nil {
		t.Fatal(err)
	}
	_, creatorKey := creator.InstallationIdentity()
	wrapped, err := invited.WireCodec(nil, nil).Wrap(late, creatorKey)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := creator.ReceiveGiftWrap(ctx, wrapped.ExactWire, "wss://relay.example", time.Now().UTC()); err == nil {
		t.Fatal("revoked device traffic projected")
	}
	status, err := creator.NetworkStatus(ctx)
	if err != nil || status.RevokedDeviceTraffic != 1 {
		t.Fatalf("revoked device status = %#v, %v", status, err)
	}
	regrant, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop"})
	if err != nil {
		t.Fatal(err)
	}
	repeated, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: invitedID, SignerKeyID: invitedKey, Name: "desktop"})
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Equal(regrant.DeviceGrantEvent, bundle.DeviceGrantEvent) || !bytes.Equal(regrant.DeviceGrantEvent, repeated.DeviceGrantEvent) {
		t.Fatal("regrant was not new and idempotent")
	}
	devices, err := creator.HumanDevices(ctx)
	if err != nil {
		t.Fatal(err)
	}
	for _, device := range devices {
		if device.InstallationID == invitedID && device.State != "pending" {
			t.Fatalf("regranted device state = %q", device.State)
		}
	}
}

func TestAccountFanoutUsesOneDurableWrapperPerActiveDevice(t *testing.T) {
	ctx := context.Background()
	creator := openStore(t, filepath.Join(t.TempDir(), "creator", "hq.db"))
	desktop := openStore(t, filepath.Join(t.TempDir(), "desktop", "hq.db"))
	server := openStore(t, filepath.Join(t.TempDir(), "server", "hq.db"))
	const relay = "wss://relay.example"
	if err := creator.AddRelay(ctx, RelayConfig{URL: relay, Read: true, Write: true, RequireAuth: true}); err != nil {
		t.Fatal(err)
	}

	join := func(target *SQLite, label string) PairingBundle {
		t.Helper()
		targetID, targetKey := target.InstallationIdentity()
		bundle, err := creator.CreateHumanInvite(ctx, HumanInviteRequest{InstallationID: targetID, SignerKeyID: targetKey, Name: label, Relays: []string{relay}})
		if err != nil {
			t.Fatal(err)
		}
		raw, _ := json.Marshal(bundle)
		if err := target.JoinHumanInvite(ctx, raw); err != nil {
			t.Fatal(err)
		}
		if err := creator.AppendCanonical(ctx, []event.SignedEvent{canonicalEventByTypeAndInstallation(t, target, event.TypeHumanDeviceAccept, targetID)}); err != nil {
			t.Fatal(err)
		}
		return bundle
	}
	desktopBundle := join(desktop, "desktop")
	serverBundle := join(server, "server")
	serverDevices, err := server.HumanDevices(ctx)
	if err != nil || len(serverDevices) != 3 {
		t.Fatalf("new device authority history = %#v, %v", serverDevices, err)
	}

	agent, err := creator.ResolveMailbox(ctx, model.SessionIdentity{Harness: "codex", ExternalSessionID: "fanout-agent"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		t.Fatal(err)
	}
	questionID := "0198c7ec-73b0-7cc3-a5f7-e31c77140df1"
	if err := creator.Create(ctx, model.Message{ID: questionID, SenderMailboxID: agent.ID, RecipientMailboxID: model.HumanMailboxID, Body: "fan out", Context: model.RepositoryContext{Directory: "/repo"}, CreatedAt: time.Now().UTC()}); err != nil {
		t.Fatal(err)
	}
	question, err := creator.Get(ctx, questionID)
	if err != nil {
		t.Fatal(err)
	}
	rows, err := creator.db.Query(`SELECT recipient_installation_id FROM outbox WHERE event_id=? ORDER BY recipient_installation_id`, question.EventID)
	if err != nil {
		t.Fatal(err)
	}
	var recipients []string
	for rows.Next() {
		var recipient string
		_ = rows.Scan(&recipient)
		recipients = append(recipients, recipient)
	}
	rows.Close()
	wantRecipients := []string{desktopBundle.TargetInstallationID, serverBundle.TargetInstallationID}
	slices.Sort(wantRecipients)
	if !slices.Equal(recipients, wantRecipients) {
		t.Fatalf("fanout recipients = %#v, want %#v", recipients, wantRecipients)
	}
	if _, err := creator.PrepareOutbound(ctx, 1000); err != nil {
		t.Fatal(err)
	}
	wrappers := make(map[string]string)
	rows, err = creator.db.Query(`SELECT recipient_installation_id,hex(exact_gift_wrap_bytes) FROM outbox WHERE event_id=?`, question.EventID)
	if err != nil {
		t.Fatal(err)
	}
	for rows.Next() {
		var recipient, raw string
		_ = rows.Scan(&recipient, &raw)
		wrappers[recipient] = raw
	}
	rows.Close()
	if len(wrappers) != 2 || wrappers[wantRecipients[0]] == "" || wrappers[wantRecipients[0]] == wrappers[wantRecipients[1]] {
		t.Fatalf("per-device wrappers = %#v", wrappers)
	}
	if err := creator.Rebuild(ctx); err != nil {
		t.Fatal(err)
	}
	for recipient, want := range wrappers {
		var got string
		if err := creator.db.QueryRow(`SELECT hex(exact_gift_wrap_bytes) FROM outbox WHERE event_id=? AND recipient_installation_id=?`, question.EventID, recipient).Scan(&got); err != nil || got != want {
			t.Fatalf("wrapper after rebuild for %s changed: %v", recipient, err)
		}
	}
	if err := creator.RevokeHumanDevice(ctx, desktopBundle.TargetInstallationID); err != nil {
		t.Fatal(err)
	}
	states := make(map[string]string)
	rows, _ = creator.db.Query(`SELECT recipient_installation_id,state FROM outbox WHERE event_id=?`, question.EventID)
	for rows.Next() {
		var recipient, state string
		_ = rows.Scan(&recipient, &state)
		states[recipient] = state
	}
	rows.Close()
	if states[desktopBundle.TargetInstallationID] != "revoked" || states[serverBundle.TargetInstallationID] != "queued" {
		t.Fatalf("fanout after revoke = %#v", states)
	}
	status, err := creator.NetworkStatus(ctx)
	if err != nil || status.AccountMembers != 2 || status.PendingAccountFanout == 0 {
		t.Fatalf("account network status = %#v, %v", status, err)
	}
}

func canonicalEventByType(t *testing.T, s *SQLite, kind event.Type) event.SignedEvent {
	t.Helper()
	var raw []byte
	if err := s.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_type=? ORDER BY created_at DESC,event_id DESC LIMIT 1`, kind).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	inspection := event.Inspect(raw)
	if inspection.Status == event.StatusInvalid {
		t.Fatal(inspection.Err)
	}
	return inspection.Event
}

func canonicalEventByTypeAndInstallation(t *testing.T, s *SQLite, kind event.Type, installationID string) event.SignedEvent {
	t.Helper()
	var raw []byte
	if err := s.db.QueryRow(`SELECT raw FROM canonical_events WHERE event_type=? AND installation_id=? ORDER BY created_at DESC,event_id DESC LIMIT 1`, kind, installationID).Scan(&raw); err != nil {
		t.Fatal(err)
	}
	inspection := event.Inspect(raw)
	if inspection.Status == event.StatusInvalid {
		t.Fatal(inspection.Err)
	}
	return inspection.Event
}
