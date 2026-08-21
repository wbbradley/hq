package store

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/model"
)

const pairingBundleVersion = 2

func (s *SQLite) HumanAccount(ctx context.Context) (HumanAccount, error) {
	var account HumanAccount
	err := s.db.QueryRowContext(ctx, `SELECT a.account_id,a.label,a.creator_installation_id,a.creator_signer_key_id FROM human_account_default d JOIN human_accounts a ON a.account_id=d.account_id WHERE d.id=1`).Scan(&account.ID, &account.Label, &account.CreatorInstallationID, &account.CreatorSignerKeyID)
	if errors.Is(err, sql.ErrNoRows) {
		return account, errors.New("this installation has no active human account")
	}
	if err != nil {
		return account, err
	}
	account.LocalInstallationID = s.signer.InstallationID
	account.Creator = account.CreatorInstallationID == s.signer.InstallationID
	return account, nil
}

func (s *SQLite) HumanDevices(ctx context.Context) ([]HumanDevice, error) {
	account, err := s.HumanAccount(ctx)
	if err != nil {
		return nil, err
	}
	rows, err := s.db.QueryContext(ctx, `SELECT account_id,installation_id,signer_key_id,label,relays_json,state FROM human_account_devices WHERE account_id=? ORDER BY label,installation_id`, account.ID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var devices []HumanDevice
	for rows.Next() {
		var device HumanDevice
		var relays string
		if err := rows.Scan(&device.AccountID, &device.InstallationID, &device.SignerKeyID, &device.Label, &relays, &device.State); err != nil {
			return nil, err
		}
		_ = json.Unmarshal([]byte(relays), &device.Relays)
		devices = append(devices, device)
	}
	return devices, rows.Err()
}

func (s *SQLite) localAccountAction(ctx context.Context, accountID string) (HumanAccount, []string, string, error) {
	account, err := s.HumanAccount(ctx)
	if err != nil {
		return HumanAccount{}, nil, "", err
	}
	if accountID != "" && account.ID != accountID {
		return HumanAccount{}, nil, "", errors.New("message belongs to another human account")
	}
	state, err := s.canonicalState(ctx)
	if err != nil {
		return HumanAccount{}, nil, "", err
	}
	parents, active := state.AccountActionParents(account.ID, s.signer.InstallationID)
	if !active {
		return HumanAccount{}, nil, "", errors.New("local installation is not an active human account device")
	}
	var label string
	if err := s.db.QueryRowContext(ctx, `SELECT label FROM human_account_devices WHERE account_id=? AND installation_id=? AND state='active'`, account.ID, s.signer.InstallationID).Scan(&label); err != nil {
		return HumanAccount{}, nil, "", err
	}
	return account, parents, label, nil
}

func (s *SQLite) canonicalState(ctx context.Context) (event.State, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT raw FROM canonical_events ORDER BY event_id`)
	if err != nil {
		return event.State{}, err
	}
	defer rows.Close()
	var raw [][]byte
	for rows.Next() {
		var item []byte
		if err := rows.Scan(&item); err != nil {
			return event.State{}, err
		}
		raw = append(raw, item)
	}
	if err := rows.Err(); err != nil {
		return event.State{}, err
	}
	return event.Reduce(raw, s.policy()), nil
}

func (s *SQLite) CreateHumanInvite(ctx context.Context, request HumanInviteRequest) (PairingBundle, error) {
	account, err := s.HumanAccount(ctx)
	if err != nil {
		return PairingBundle{}, err
	}
	if !account.Creator {
		return PairingBundle{}, errors.New("only the account creator can invite a device")
	}
	if request.InstallationID == s.signer.InstallationID {
		return PairingBundle{}, errors.New("the local installation is already an account device")
	}
	if strings.TrimSpace(request.Name) == "" {
		return PairingBundle{}, errors.New("device name is required")
	}
	request.Relays, err = normalizeRelayHints(request.Relays)
	if err != nil {
		return PairingBundle{}, err
	}
	var existingKey, state, grantID string
	err = s.db.QueryRowContext(ctx, `SELECT signer_key_id,state,grant_event_id FROM human_account_devices WHERE account_id=? AND installation_id=?`, account.ID, request.InstallationID).Scan(&existingKey, &state, &grantID)
	if err == nil && existingKey != request.SignerKeyID {
		return PairingBundle{}, errors.New("the installation already has a grant for another key")
	}
	if err != nil && !errors.Is(err, sql.ErrNoRows) {
		return PairingBundle{}, err
	}
	if err == nil && state != "revoked" && grantID != "" {
		bundle, err := s.pairingBundle(ctx, account, request, grantID)
		if err == nil {
			err = s.recordMutation(ctx, bundle)
		}
		return bundle, err
	}

	creationID, creationRaw, err := s.accountCreation(ctx, account.ID)
	if err != nil {
		return PairingBundle{}, err
	}
	parents := append([]string{creationID}, s.deviceEventIDs(ctx, account.ID, request.InstallationID)...)
	parents = uniqueSorted(parents)
	creatorRelays, err := s.localRelayHints(ctx)
	if err != nil {
		return PairingBundle{}, err
	}
	device := event.HumanDevicePayload{
		AccountID: account.ID, CreatorInstallationID: account.CreatorInstallationID,
		CreatorSignerKeyID: account.CreatorSignerKeyID, InstallationID: request.InstallationID,
		SignerKeyID: request.SignerKeyID, Label: request.Name, Relays: request.Relays, CreatorRelays: creatorRelays,
	}
	peerPayload, _ := event.MarshalPayload(event.PeerPayload{InstallationID: request.InstallationID, SignerKeyID: request.SignerKeyID, Name: request.Name, Relays: request.Relays})
	grantPayload, _ := event.MarshalPayload(device)
	contents := []event.Content{
		{Type: event.TypePeerTrust, Parents: s.peerParents(ctx, request.InstallationID), Scope: event.ScopeInstallationPrivate, Payload: peerPayload},
		{Type: event.TypeHumanDeviceGrant, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: request.InstallationID, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: parents, Scope: event.ScopeAccountAddressed, Payload: grantPayload},
	}
	signed, err := s.signContents(ctx, contents, nil)
	if err != nil {
		return PairingBundle{}, err
	}
	authority, err := s.accountAuthorityEvents(ctx, account.ID)
	if err != nil {
		return PairingBundle{}, err
	}
	authority = append(authority, append([]byte(nil), signed[1].Wire...))
	bundle := PairingBundle{
		Version: pairingBundleVersion, AccountID: account.ID, AccountLabel: account.Label,
		CreatorInstallationID: account.CreatorInstallationID, CreatorSignerKeyID: account.CreatorSignerKeyID,
		CreatorRelays: creatorRelays, TargetInstallationID: request.InstallationID, TargetSignerKeyID: request.SignerKeyID,
		TargetLabel: request.Name, TargetRelays: request.Relays, AccountCreationEvent: creationRaw,
		DeviceGrantEvent: signed[1].Wire, AccountAuthorityEvents: authority,
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return PairingBundle{}, err
	}
	defer tx.Rollback()
	commit, err := s.ingestCanonicalTx(ctx, tx, signed, true)
	if err != nil {
		return PairingBundle{}, err
	}
	if err := recordMutationTx(ctx, tx, bundle); err != nil {
		return PairingBundle{}, err
	}
	if err := tx.Commit(); err != nil {
		return PairingBundle{}, err
	}
	s.notifyCanonicalCommit(commit)
	return bundle, nil
}

func (s *SQLite) pairingBundle(ctx context.Context, account HumanAccount, request HumanInviteRequest, grantID string) (PairingBundle, error) {
	_, creationRaw, err := s.accountCreation(ctx, account.ID)
	if err != nil {
		return PairingBundle{}, err
	}
	grantRaw, err := s.eventRaw(ctx, grantID)
	if err != nil {
		return PairingBundle{}, err
	}
	inspection := event.Inspect(grantRaw)
	var device event.HumanDevicePayload
	if inspection.Status == event.StatusInvalid || json.Unmarshal(inspection.Event.Content.Payload, &device) != nil {
		return PairingBundle{}, errors.New("stored device grant is invalid")
	}
	authority, err := s.accountAuthorityEvents(ctx, account.ID)
	if err != nil {
		return PairingBundle{}, err
	}
	return PairingBundle{
		Version: pairingBundleVersion, AccountID: account.ID, AccountLabel: account.Label,
		CreatorInstallationID: account.CreatorInstallationID, CreatorSignerKeyID: account.CreatorSignerKeyID,
		CreatorRelays: device.CreatorRelays, TargetInstallationID: device.InstallationID, TargetSignerKeyID: device.SignerKeyID,
		TargetLabel: device.Label, TargetRelays: device.Relays, AccountCreationEvent: creationRaw, DeviceGrantEvent: grantRaw,
		AccountAuthorityEvents: authority,
	}, nil
}

func (s *SQLite) accountAuthorityEvents(ctx context.Context, accountID string) ([][]byte, error) {
	rows, err := s.db.QueryContext(ctx, `SELECT raw FROM canonical_events WHERE event_type IN (?,?,?,?) ORDER BY event_id`, event.TypeHumanAccountCreate, event.TypeHumanDeviceGrant, event.TypeHumanDeviceAccept, event.TypeHumanDeviceRevoke)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result [][]byte
	for rows.Next() {
		var raw []byte
		if err := rows.Scan(&raw); err != nil {
			return nil, err
		}
		inspection := event.Inspect(raw)
		if inspection.Status == event.StatusInvalid {
			continue
		}
		matched := false
		switch inspection.Event.Content.Type {
		case event.TypeHumanAccountCreate:
			var payload event.HumanAccountPayload
			matched = json.Unmarshal(inspection.Event.Content.Payload, &payload) == nil && payload.AccountID == accountID
		default:
			var payload event.HumanDevicePayload
			matched = json.Unmarshal(inspection.Event.Content.Payload, &payload) == nil && payload.AccountID == accountID
		}
		if matched {
			result = append(result, append([]byte(nil), raw...))
		}
	}
	return result, rows.Err()
}

func (s *SQLite) JoinHumanInvite(ctx context.Context, raw []byte) error {
	bundle, _, grant, authority, payload, err := inspectPairingBundle(raw)
	if err != nil {
		return err
	}
	if bundle.TargetInstallationID != s.signer.InstallationID || bundle.TargetSignerKeyID != s.signer.PublicKey() {
		return errors.New("pairing invite targets another installation or key")
	}
	var active int
	if err := s.db.QueryRowContext(ctx, `SELECT count(*) FROM human_account_devices d JOIN human_account_default a ON a.account_id=d.account_id WHERE d.account_id=? AND d.installation_id=? AND d.signer_key_id=? AND d.state='active'`, bundle.AccountID, s.signer.InstallationID, s.signer.PublicKey()).Scan(&active); err == nil && active > 0 {
		return s.recordMutation(ctx, nil)
	}
	bundle.CreatorRelays, err = normalizeRelayHints(bundle.CreatorRelays)
	if err != nil {
		return err
	}
	peerPayload, _ := event.MarshalPayload(event.PeerPayload{InstallationID: bundle.CreatorInstallationID, SignerKeyID: bundle.CreatorSignerKeyID, Name: bundle.AccountLabel, Relays: bundle.CreatorRelays})
	acceptPayload, _ := event.MarshalPayload(payload)
	now := time.Now().UTC()
	local, err := s.signContents(ctx, []event.Content{
		{Type: event.TypePeerTrust, Parents: s.peerParents(ctx, bundle.CreatorInstallationID), Scope: event.ScopeInstallationPrivate, Payload: peerPayload},
		{Type: event.TypeHumanDeviceAccept, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: bundle.CreatorInstallationID, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: bundle.AccountID}, Parents: []string{grant.ID()}, Scope: event.ScopeAccountAddressed, Payload: acceptPayload},
	}, []time.Time{now, now})
	if err != nil {
		return err
	}
	selectionPayload, _ := event.MarshalPayload(event.HumanAccountSelectionPayload{AccountID: bundle.AccountID})
	selectionParents := append(s.eventTypeIDs(ctx, event.TypeHumanAccountSelect), local[1].ID())
	selectionParents = uniqueSorted(selectionParents)
	selection, err := s.signContents(ctx, []event.Content{{Type: event.TypeHumanAccountSelect, Parents: selectionParents, Scope: event.ScopeInstallationPrivate, Payload: selectionPayload}}, []time.Time{now})
	if err != nil {
		return err
	}
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	additions := append([]event.SignedEvent(nil), authority...)
	additions = append(additions, local[0], local[1], selection[0])
	commit, err := s.ingestCanonicalTx(ctx, tx, additions, true)
	if err != nil {
		return err
	}
	if err := recordMutationTx(ctx, tx, nil); err != nil {
		return err
	}
	if err := tx.Commit(); err != nil {
		return err
	}
	s.notifyCanonicalCommit(commit)
	return nil
}

func (s *SQLite) RevokeHumanDevice(ctx context.Context, installationID string) error {
	account, err := s.HumanAccount(ctx)
	if err != nil {
		return err
	}
	if !account.Creator {
		return errors.New("only the account creator can revoke a device")
	}
	if installationID == s.signer.InstallationID {
		return errors.New("the creator device cannot revoke itself")
	}
	var device HumanDevice
	var relays string
	err = s.db.QueryRowContext(ctx, `SELECT account_id,installation_id,signer_key_id,label,relays_json,state FROM human_account_devices WHERE account_id=? AND installation_id=?`, account.ID, installationID).Scan(&device.AccountID, &device.InstallationID, &device.SignerKeyID, &device.Label, &relays, &device.State)
	if errors.Is(err, sql.ErrNoRows) {
		return errors.New("human account device not found")
	}
	if err != nil {
		return err
	}
	if device.State == "revoked" {
		return s.recordMutation(ctx, nil)
	}
	_ = json.Unmarshal([]byte(relays), &device.Relays)
	var grantRaw []byte
	if err := s.db.QueryRowContext(ctx, `SELECT c.raw FROM human_account_devices d JOIN canonical_events c ON c.event_id=d.grant_event_id WHERE d.account_id=? AND d.installation_id=?`, account.ID, installationID).Scan(&grantRaw); err != nil {
		return err
	}
	grant := event.Inspect(grantRaw)
	var exact event.HumanDevicePayload
	if grant.Status == event.StatusInvalid || json.Unmarshal(grant.Event.Content.Payload, &exact) != nil {
		return errors.New("stored human device grant is invalid")
	}
	payload, _ := event.MarshalPayload(exact)
	content := event.Content{Type: event.TypeHumanDeviceRevoke, Sender: s.localAddress(model.HumanMailboxID), Recipient: &event.MailboxAddress{InstallationID: device.InstallationID, MailboxID: model.HumanMailboxID}, Audience: &event.Audience{HumanAccountID: account.ID}, Parents: s.deviceEventIDs(ctx, account.ID, device.InstallationID), Scope: event.ScopeAccountAddressed, Payload: payload}
	return s.appendContents(ctx, []event.Content{content}, nil, nil)
}

func inspectPairingBundle(raw []byte) (PairingBundle, event.SignedEvent, event.SignedEvent, []event.SignedEvent, event.HumanDevicePayload, error) {
	var bundle PairingBundle
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&bundle); err != nil {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, event.HumanDevicePayload{}, fmt.Errorf("decode pairing invite: %w", err)
	}
	if err := decoder.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, event.HumanDevicePayload{}, errors.New("pairing invite has trailing data")
	}
	if bundle.Version != pairingBundleVersion {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, event.HumanDevicePayload{}, fmt.Errorf("unsupported pairing invite version %d", bundle.Version)
	}
	created := event.Inspect(bundle.AccountCreationEvent)
	granted := event.Inspect(bundle.DeviceGrantEvent)
	if created.Status != event.StatusProjected || created.Event.Content.Type != event.TypeHumanAccountCreate {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, event.HumanDevicePayload{}, errors.New("pairing invite has an invalid account creation event")
	}
	if granted.Status != event.StatusProjected || granted.Event.Content.Type != event.TypeHumanDeviceGrant {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, event.HumanDevicePayload{}, errors.New("pairing invite has an invalid device grant event")
	}
	var account event.HumanAccountPayload
	var device event.HumanDevicePayload
	if json.Unmarshal(created.Event.Content.Payload, &account) != nil || json.Unmarshal(granted.Event.Content.Payload, &device) != nil {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, device, errors.New("pairing invite has malformed signed payloads")
	}
	if bundle.AccountID != account.AccountID || bundle.AccountLabel != account.Label || bundle.CreatorInstallationID != account.CreatorInstallationID || bundle.CreatorSignerKeyID != account.CreatorSignerKeyID ||
		bundle.AccountID != device.AccountID || bundle.CreatorInstallationID != device.CreatorInstallationID || bundle.CreatorSignerKeyID != device.CreatorSignerKeyID || bundle.TargetInstallationID != device.InstallationID || bundle.TargetSignerKeyID != device.SignerKeyID || bundle.TargetLabel != device.Label || !slices.Equal(bundle.TargetRelays, device.Relays) || !slices.Equal(bundle.CreatorRelays, device.CreatorRelays) {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, device, errors.New("pairing invite fields do not match its signed events")
	}
	if !slices.Contains(granted.Event.Content.Parents, created.Event.ID()) {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, device, errors.New("device grant does not name the account creation event")
	}
	authority := make([]event.SignedEvent, 0, len(bundle.AccountAuthorityEvents))
	seen := make(map[string]bool)
	for _, rawEvent := range bundle.AccountAuthorityEvents {
		inspection := event.Inspect(rawEvent)
		if inspection.Status == event.StatusInvalid || !isAccountAuthorityEvent(inspection.Event, bundle.AccountID) || seen[inspection.Event.ID()] {
			return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, device, errors.New("pairing invite has invalid account authority history")
		}
		seen[inspection.Event.ID()] = true
		authority = append(authority, inspection.Event)
	}
	if !seen[created.Event.ID()] || !seen[granted.Event.ID()] {
		return bundle, event.SignedEvent{}, event.SignedEvent{}, nil, device, errors.New("pairing invite authority history omits required events")
	}
	return bundle, created.Event, granted.Event, authority, device, nil
}

func isAccountAuthorityEvent(item event.SignedEvent, accountID string) bool {
	switch item.Content.Type {
	case event.TypeHumanAccountCreate:
		var payload event.HumanAccountPayload
		return json.Unmarshal(item.Content.Payload, &payload) == nil && payload.AccountID == accountID
	case event.TypeHumanDeviceGrant, event.TypeHumanDeviceAccept, event.TypeHumanDeviceRevoke:
		var payload event.HumanDevicePayload
		return json.Unmarshal(item.Content.Payload, &payload) == nil && payload.AccountID == accountID
	default:
		return false
	}
}

func normalizeRelayHints(values []string) ([]string, error) {
	if len(values) > 3 {
		return nil, errors.New("a device may have at most three relay hints")
	}
	result := make([]string, 0, len(values))
	for _, value := range values {
		normalized, err := normalizeRelay(value)
		if err != nil {
			return nil, err
		}
		if !slices.Contains(result, normalized) {
			result = append(result, normalized)
		}
	}
	sort.Strings(result)
	return result, nil
}

func (s *SQLite) localRelayHints(ctx context.Context) ([]string, error) {
	configured, err := s.ListRelays(ctx)
	if err != nil {
		return nil, err
	}
	var hints []string
	for _, relay := range configured {
		if relay.Read || relay.Write {
			hints = append(hints, relay.URL)
		}
		if len(hints) == 3 {
			break
		}
	}
	return hints, nil
}

func (s *SQLite) accountCreation(ctx context.Context, accountID string) (string, []byte, error) {
	var id string
	if err := s.db.QueryRowContext(ctx, `SELECT creation_event_id FROM human_accounts WHERE account_id=?`, accountID).Scan(&id); err != nil {
		return "", nil, err
	}
	raw, err := s.eventRaw(ctx, id)
	return id, raw, err
}

func (s *SQLite) eventRaw(ctx context.Context, id string) ([]byte, error) {
	var raw []byte
	if err := s.db.QueryRowContext(ctx, `SELECT raw FROM canonical_events WHERE event_id=?`, id).Scan(&raw); err != nil {
		return nil, err
	}
	return raw, nil
}

func (s *SQLite) deviceEventIDs(ctx context.Context, accountID, installationID string) []string {
	rows, err := s.db.QueryContext(ctx, `SELECT event_id,raw FROM canonical_events WHERE event_type IN ('human.device.grant','human.device.accept','human.device.revoke')`)
	if err != nil {
		return nil
	}
	defer rows.Close()
	var ids []string
	for rows.Next() {
		var id string
		var raw []byte
		if rows.Scan(&id, &raw) != nil {
			continue
		}
		inspection := event.Inspect(raw)
		var payload event.HumanDevicePayload
		if json.Unmarshal(inspection.Event.Content.Payload, &payload) == nil && payload.AccountID == accountID && payload.InstallationID == installationID {
			ids = append(ids, id)
		}
	}
	return uniqueSorted(ids)
}

func (s *SQLite) eventTypeIDs(ctx context.Context, kind event.Type) []string {
	rows, err := s.db.QueryContext(ctx, `SELECT event_id FROM canonical_events WHERE event_type=?`, kind)
	if err != nil {
		return nil
	}
	defer rows.Close()
	var ids []string
	for rows.Next() {
		var id string
		if rows.Scan(&id) == nil {
			ids = append(ids, id)
		}
	}
	return uniqueSorted(ids)
}

func uniqueSorted(values []string) []string {
	sort.Strings(values)
	result := values[:0]
	for _, value := range values {
		if len(result) == 0 || result[len(result)-1] != value {
			result = append(result, value)
		}
	}
	return result
}
