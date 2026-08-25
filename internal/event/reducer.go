package event

import (
	"errors"
	"fmt"
	"slices"
	"sort"
	"time"

	"github.com/wbbradley/hq/internal/model"
)

var (
	ErrNotFound   = errors.New("event not found")
	ErrWaitDenied = errors.New("question does not belong to this mailbox")
	ErrNoAnswer   = errors.New("no answer is available")
)

type Policy struct {
	InstallationID string
	RootKeyID      string
	HumanMailboxID string
	SchemaVersions []int
}

type Record struct {
	Event  SignedEvent
	Status ProjectionStatus
	Reason string
}

type MailboxProjection struct {
	ID                string
	Kind              string
	Label             string
	Harness           string
	ExternalSessionID string
	Contexts          []RepositoryContext
	Bindings          []MailboxBindingPayload
}

type NamedAgentProjection struct {
	Name              string
	MailboxID         string
	Retired           bool
	Harness           string
	ExternalSessionID string
	SelectedAt        int64
	SelectionEventID  string
	Sessions          map[string]AgentSessionProjection
}

type AgentSessionProjection struct {
	Harness           string
	ExternalSessionID string
	ThreadName        string
	NameUpdatedAt     int64
	NameEventID       string
	Context           RepositoryContext
	CreatedAt         int64
	LastSelectedAt    int64
}

type PeerProjection struct {
	InstallationID string
	SignerKeyID    string
	Name           string
	Relays         []string
	Trusted        bool
}

type MailboxShareProjection struct {
	MailboxID          string
	PeerInstallationID string
	Active             bool
}

type HumanAccountProjection struct {
	ID                    string
	CreatorInstallationID string
	CreatorSignerKeyID    string
	Label                 string
	CreationEventID       string
	Devices               map[string]HumanDeviceProjection
}

type HumanDeviceProjection struct {
	InstallationID string
	SignerKeyID    string
	Label          string
	Relays         []string
	GrantEventID   string
	AcceptEventID  string
	RevokeEventIDs []string
	Active         bool
	State          string
}

type MessageProjection struct {
	ID                string
	Type              Type
	Sender            MailboxAddress
	Recipient         MailboxAddress
	ThreadID          string
	Parents           []string
	Body              string
	Details           string
	Purpose           model.MessagePurpose
	Presentation      model.PresentationKind
	Correlation       model.MessageCorrelation
	TechnicalSections []model.TechnicalSection
	MessageID         string
	AudienceAccountID string
	ActorLabel        string
	Context           *RepositoryContext
	CreatedAt         time.Time
	Incomplete        bool
	Archived          bool
	ArchivedAt        time.Time
	Rejected          bool
	PeerReceived      bool
}

type CancellationRelation string

const (
	AnswerBeforeCancellation CancellationRelation = "answer-before-cancellation"
	AnswerAfterCancellation  CancellationRelation = "answer-after-cancellation"
	AnswerConcurrent         CancellationRelation = "concurrent"
)

type ThreadProjection struct {
	ID                 string
	MessageIDs         []string
	AnswerIDs          []string
	CancellationIDs    []string
	Answered           bool
	Cancelled          bool
	AnswerCancellation map[string]map[string]CancellationRelation
}

type State struct {
	Policy           Policy
	Records          map[string]Record
	Invalid          []Record
	Mailboxes        map[string]MailboxProjection
	NamedAgents      map[string]NamedAgentProjection
	Peers            map[string]PeerProjection
	Shares           map[string]MailboxShareProjection
	Accounts         map[string]HumanAccountProjection
	DefaultAccountID string
	Messages         map[string]MessageProjection
	Threads          map[string]ThreadProjection
	DisplayOrder     []string
}

// Reduce verifies and reduces a complete event set. Callers may pass the events
// in any order and may pass the same event more than once.
func Reduce(rawEvents [][]byte, policy Policy) State {
	state := State{
		Policy:      policy,
		Records:     make(map[string]Record),
		Mailboxes:   make(map[string]MailboxProjection),
		NamedAgents: make(map[string]NamedAgentProjection),
		Peers:       make(map[string]PeerProjection),
		Shares:      make(map[string]MailboxShareProjection),
		Accounts:    make(map[string]HumanAccountProjection),
		Messages:    make(map[string]MessageProjection),
		Threads:     make(map[string]ThreadProjection),
	}
	for _, raw := range rawEvents {
		schemas := policy.SchemaVersions
		if len(schemas) == 0 {
			schemas = []int{Schema1, Schema2}
		}
		inspection := InspectWithSchemas(raw, schemas)
		record := Record{Event: inspection.Event, Status: inspection.Status}
		if inspection.Err != nil {
			record.Reason = inspection.Err.Error()
		}
		if inspection.Status == StatusInvalid || inspection.Event.Nostr.ID == "" {
			state.Invalid = append(state.Invalid, record)
			continue
		}
		id := inspection.Event.ID()
		if existing, exists := state.Records[id]; !exists || string(record.Event.Wire) < string(existing.Event.Wire) {
			state.Records[id] = record
		}
	}

	state.classifyLocalControls()
	state.reducePeers()
	state.classifyAccountEvents()
	state.projectAccounts()
	state.classifyAccountSelections()
	state.projectDefaultAccount()
	state.classifyUnsupported()
	state.reduceShares()
	state.classifyDomainEvents()
	state.projectMailboxes()
	state.classifyNamedAgents()
	state.projectNamedAgents()
	state.projectMessages()
	state.applyMessageState()
	state.projectThreads()
	state.DisplayOrder = state.orderMessages()
	return state
}

func (s *State) classifyAccountEvents() {
	ids := sortedRecordIDs(s.Records)
	for _, id := range ids {
		record := s.Records[id]
		if record.Status != StatusProjected || !isAccountAuthorityType(record.Event.Content.Type) {
			continue
		}
		var reason string
		if isHumanDeviceType(record.Event.Content.Type) && (record.Event.Content.Sender == nil || record.Event.Content.Recipient == nil || record.Event.Content.Sender.MailboxID != s.Policy.HumanMailboxID || record.Event.Content.Recipient.MailboxID != s.Policy.HumanMailboxID) {
			reason = "human device event must use the reserved human mailbox"
		}
		switch record.Event.Content.Type {
		case TypeHumanAccountCreate:
			var payload HumanAccountPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if record.Event.Content.InstallationID != payload.CreatorInstallationID || record.Event.Nostr.PubKey != payload.CreatorSignerKeyID {
				reason = "human account creation signer does not match its creator"
			} else if s.accountCreationConflicts(id, payload) {
				reason = "human account ID has conflicting creation events"
			}
		case TypeHumanDeviceGrant:
			var payload HumanDevicePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if reason == "" && !s.accountCreatorMatches(payload.AccountID, payload.CreatorInstallationID, payload.CreatorSignerKeyID, record.Event.Content.Parents) {
				reason = "device grant has no matching account creation parent"
			}
		case TypeHumanDeviceAccept:
			var payload HumanDevicePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if reason == "" && !s.hasMatchingGrant(payload, record.Event.Content.Parents) {
				reason = "device acceptance has no matching grant parent"
			}
		case TypeHumanDeviceRevoke:
			var payload HumanDevicePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if reason == "" && !s.hasMatchingGrant(payload, record.Event.Content.Parents) {
				reason = "device revocation has no matching grant ancestor"
			}
		}
		if reason != "" {
			record.Status = StatusUnresolved
			record.Reason = reason
			s.Records[id] = record
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && isAccountAuthorityType(record.Event.Content.Type) && !s.parentsUsable(record.Event, make(map[string]bool)) {
			record.Status = StatusUnresolved
			record.Reason = "human account event has a missing or unusable causal parent"
			s.Records[id] = record
		}
	}
}

func (s *State) accountCreationConflicts(currentID string, current HumanAccountPayload) bool {
	for id, record := range s.Records {
		if id == currentID || record.Event.Content.Type != TypeHumanAccountCreate {
			continue
		}
		var other HumanAccountPayload
		_ = decodePayload(record.Event.Content.Payload, &other)
		if other.AccountID == current.AccountID {
			return true
		}
	}
	return false
}

func (s *State) accountCreatorMatches(accountID, installationID, signerKeyID string, parents []string) bool {
	for _, id := range parents {
		for candidateID, record := range s.Records {
			if record.Status != StatusProjected || record.Event.Content.Type != TypeHumanAccountCreate || !s.ancestor(candidateID, id) {
				continue
			}
			var payload HumanAccountPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if payload.AccountID == accountID && payload.CreatorInstallationID == installationID && payload.CreatorSignerKeyID == signerKeyID {
				return true
			}
		}
	}
	return false
}

func (s *State) hasMatchingGrant(payload HumanDevicePayload, parents []string) bool {
	for _, parent := range parents {
		for id, record := range s.Records {
			if record.Status != StatusProjected || record.Event.Content.Type != TypeHumanDeviceGrant || !s.ancestor(id, parent) {
				continue
			}
			var grant HumanDevicePayload
			_ = decodePayload(record.Event.Content.Payload, &grant)
			if sameHumanDevice(grant, payload) {
				return true
			}
		}
	}
	return false
}

func sameHumanDevice(a, b HumanDevicePayload) bool {
	return a.AccountID == b.AccountID && a.CreatorInstallationID == b.CreatorInstallationID && a.CreatorSignerKeyID == b.CreatorSignerKeyID &&
		a.InstallationID == b.InstallationID && a.SignerKeyID == b.SignerKeyID && a.Label == b.Label && slices.Equal(a.Relays, b.Relays) && slices.Equal(a.CreatorRelays, b.CreatorRelays)
}

func (s *State) projectAccounts() {
	for _, id := range sortedRecordIDs(s.Records) {
		record := s.Records[id]
		if record.Status != StatusProjected || record.Event.Content.Type != TypeHumanAccountCreate {
			continue
		}
		var payload HumanAccountPayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		s.Accounts[payload.AccountID] = HumanAccountProjection{
			ID: payload.AccountID, CreatorInstallationID: payload.CreatorInstallationID,
			CreatorSignerKeyID: payload.CreatorSignerKeyID, Label: payload.Label, CreationEventID: id,
			Devices: map[string]HumanDeviceProjection{payload.CreatorInstallationID: {
				InstallationID: payload.CreatorInstallationID, SignerKeyID: payload.CreatorSignerKeyID,
				Label: payload.Label, Active: true, State: "active",
			}},
		}
	}
	for accountID, account := range s.Accounts {
		groups := make(map[string][]Record)
		creator := account.Devices[account.CreatorInstallationID]
		for _, record := range s.Records {
			if record.Status != StatusProjected || !isHumanDeviceType(record.Event.Content.Type) {
				continue
			}
			var payload HumanDevicePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if payload.AccountID == accountID {
				groups[payload.InstallationID] = append(groups[payload.InstallationID], record)
				for _, relay := range payload.CreatorRelays {
					if !slices.Contains(creator.Relays, relay) {
						creator.Relays = append(creator.Relays, relay)
					}
				}
			}
		}
		sort.Strings(creator.Relays)
		account.Devices[account.CreatorInstallationID] = creator
		for installationID, records := range groups {
			device := HumanDeviceProjection{InstallationID: installationID}
			var grants, accepts, revokes []Record
			for _, record := range records {
				var payload HumanDevicePayload
				_ = decodePayload(record.Event.Content.Payload, &payload)
				switch record.Event.Content.Type {
				case TypeHumanDeviceGrant:
					grants = append(grants, record)
				case TypeHumanDeviceAccept:
					accepts = append(accepts, record)
					if device.AcceptEventID == "" || record.Event.ID() < device.AcceptEventID {
						device.AcceptEventID = record.Event.ID()
					}
				case TypeHumanDeviceRevoke:
					revokes = append(revokes, record)
					device.RevokeEventIDs = append(device.RevokeEventIDs, record.Event.ID())
				}
			}
			maximalGrants := s.maximal(grants)
			if len(maximalGrants) > 0 {
				grant := maximalGrants[0]
				var payload HumanDevicePayload
				_ = decodePayload(grant.Event.Content.Payload, &payload)
				device.GrantEventID, device.SignerKeyID, device.Label, device.Relays = grant.Event.ID(), payload.SignerKeyID, payload.Label, append([]string(nil), payload.Relays...)
			}
			terminals := append(append(append([]Record(nil), grants...), accepts...), revokes...)
			maxima := s.maximal(terminals)
			device.State = "pending"
			for _, record := range maxima {
				if record.Event.Content.Type == TypeHumanDeviceRevoke {
					device.State = "revoked"
					break
				}
			}
			if device.State != "revoked" && len(maxima) > 0 {
				device.State = "active"
				for _, record := range maxima {
					if record.Event.Content.Type != TypeHumanDeviceAccept {
						device.State = "pending"
						break
					}
				}
			}
			device.Active = device.State == "active"
			sort.Strings(device.RevokeEventIDs)
			account.Devices[installationID] = device
		}
		s.Accounts[accountID] = account
	}
}

func (s *State) classifyAccountSelections() {
	for id, record := range s.Records {
		if record.Status != StatusProjected || record.Event.Content.Type != TypeHumanAccountSelect {
			continue
		}
		var payload HumanAccountSelectionPayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		account, ok := s.Accounts[payload.AccountID]
		device, member := account.Devices[s.Policy.InstallationID]
		if !s.signedByLocalRoot(record.Event) || !ok || !member || !s.selectionHasMembershipParent(record.Event, account, device) {
			record.Status = StatusUnauthorized
			record.Reason = "human account selection lacks active local membership"
			s.Records[id] = record
		}
	}
}

func (s *State) selectionHasMembershipParent(selection SignedEvent, account HumanAccountProjection, device HumanDeviceProjection) bool {
	target := account.CreationEventID
	if s.Policy.InstallationID != account.CreatorInstallationID {
		target = device.AcceptEventID
	}
	for _, parent := range selection.Content.Parents {
		if target != "" && s.ancestor(target, parent) {
			return true
		}
	}
	return false
}

func (s *State) projectDefaultAccount() {
	var records []Record
	for _, record := range s.Records {
		if record.Status == StatusProjected && record.Event.Content.Type == TypeHumanAccountSelect {
			records = append(records, record)
		}
	}
	maxima := s.maximal(records)
	if len(maxima) != 1 {
		return
	}
	var payload HumanAccountSelectionPayload
	_ = decodePayload(maxima[0].Event.Content.Payload, &payload)
	account := s.Accounts[payload.AccountID]
	if device, ok := account.Devices[s.Policy.InstallationID]; ok && device.Active {
		s.DefaultAccountID = payload.AccountID
	}
}

func sortedRecordIDs(records map[string]Record) []string {
	ids := make([]string, 0, len(records))
	for id := range records {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	return ids
}

func (s *State) classifyUnsupported() {
	for id, record := range s.Records {
		if record.Status != StatusUnsupported {
			continue
		}
		keyID := record.Event.Nostr.PubKey
		authorized := keyID == s.Policy.RootKeyID
		if !authorized {
			for _, peer := range s.Peers {
				if peer.Trusted && peer.SignerKeyID == keyID {
					authorized = true
					break
				}
			}
		}
		if !authorized {
			record.Status = StatusUnauthorized
			record.Reason = "unsupported event signer is not trusted"
			s.Records[id] = record
		}
	}
}

func (s *State) classifyLocalControls() {
	for id, record := range s.Records {
		if record.Status != StatusProjected || !isControlType(record.Event.Content.Type) {
			continue
		}
		if !s.signedByLocalRoot(record.Event) {
			record.Status = StatusUnauthorized
			record.Reason = "control event is not signed by the local installation root"
			s.Records[id] = record
		}
	}
	memo := make(map[string]bool)
	var unresolved []string
	for id, record := range s.Records {
		if record.Status == StatusProjected && isControlType(record.Event.Content.Type) && !s.parentsUsable(record.Event, memo) {
			unresolved = append(unresolved, id)
		}
	}
	for _, id := range unresolved {
		record := s.Records[id]
		record.Status = StatusUnresolved
		record.Reason = "control event has a missing or unusable causal parent"
		s.Records[id] = record
	}
}

func (s *State) reducePeers() {
	groups := make(map[string][]Record)
	for _, record := range s.Records {
		if record.Status != StatusProjected || (record.Event.Content.Type != TypePeerTrust && record.Event.Content.Type != TypePeerDistrust) {
			continue
		}
		var payload PeerPayload
		if decodePayload(record.Event.Content.Payload, &payload) != nil {
			continue
		}
		groups[payload.InstallationID] = append(groups[payload.InstallationID], record)
	}
	for installationID, records := range groups {
		maxima := s.maximal(records)
		peer := PeerProjection{InstallationID: installationID, Trusted: len(maxima) > 0}
		for index, record := range maxima {
			if record.Event.Content.Type != TypePeerTrust {
				peer.Trusted = false
				continue
			}
			var payload PeerPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			if index == 0 {
				peer.SignerKeyID, peer.Name, peer.Relays = payload.SignerKeyID, payload.Name, append([]string(nil), payload.Relays...)
			} else if peer.SignerKeyID != payload.SignerKeyID {
				peer.Trusted = false
			}
		}
		s.Peers[installationID] = peer
	}
}

func (s *State) reduceShares() {
	groups := make(map[string][]Record)
	for _, record := range s.Records {
		if record.Status != StatusProjected || (record.Event.Content.Type != TypeMailboxShare && record.Event.Content.Type != TypeMailboxShareRevoke) {
			continue
		}
		var payload MailboxSharePayload
		if decodePayload(record.Event.Content.Payload, &payload) != nil {
			continue
		}
		key := shareKey(payload.MailboxID, payload.PeerInstallationID)
		groups[key] = append(groups[key], record)
	}
	for key, records := range groups {
		maxima := s.maximal(records)
		var payload MailboxSharePayload
		_ = decodePayload(maxima[0].Event.Content.Payload, &payload)
		share := MailboxShareProjection{MailboxID: payload.MailboxID, PeerInstallationID: payload.PeerInstallationID, Active: true}
		for _, record := range maxima {
			if record.Event.Content.Type != TypeMailboxShare {
				share.Active = false
			}
		}
		s.Shares[key] = share
	}
}

func (s *State) classifyDomainEvents() {
	for id, record := range s.Records {
		if record.Status != StatusProjected || isControlType(record.Event.Content.Type) || isAccountAuthorityType(record.Event.Content.Type) {
			continue
		}
		if !s.authorized(record.Event) {
			record.Status = StatusUnauthorized
			record.Reason = "event signer or mailbox route is not authorized"
			s.Records[id] = record
		}
	}
	memo := make(map[string]bool)
	var unresolved []string
	for id, record := range s.Records {
		if record.Status == StatusProjected && !isControlType(record.Event.Content.Type) && !isAccountAuthorityType(record.Event.Content.Type) && !s.parentsUsable(record.Event, memo) {
			unresolved = append(unresolved, id)
		}
	}
	for _, id := range unresolved {
		record := s.Records[id]
		record.Status = StatusUnresolved
		record.Reason = "event has a missing or unusable causal parent"
		s.Records[id] = record
	}
	for id, record := range s.Records {
		if record.Status == StatusProjected && !isControlType(record.Event.Content.Type) && !isAccountAuthorityType(record.Event.Content.Type) {
			if record.Event.Content.Type == TypeMessageArchive || record.Event.Content.Type == TypeMessageRestore || record.Event.Content.Type == TypeMessageReject {
				var payload TargetPayload
				_ = decodePayload(record.Event.Content.Payload, &payload)
				if !s.ancestor(payload.TargetEventID, id) {
					record.Status = StatusInvalid
					record.Reason = "message state target is not a causal ancestor"
					s.Records[id] = record
					continue
				}
			}
			if record.Event.Content.ThreadID != "" {
				root, ok := s.Records[record.Event.Content.ThreadID]
				if !ok || root.Status != StatusProjected || (root.Event.Content.Type != TypeQuestion && root.Event.Content.Type != TypeMessage) || !s.ancestor(record.Event.Content.ThreadID, id) {
					record.Status = StatusInvalid
					record.Reason = "thread ID is not a causal root of the event"
					s.Records[id] = record
				}
			}
		}
	}
}

func (s *State) signedByLocalRoot(event SignedEvent) bool {
	return event.Content.InstallationID == s.Policy.InstallationID && event.Nostr.PubKey == s.Policy.RootKeyID
}

func (s *State) authorized(event SignedEvent) bool {
	if event.Content.Scope == ScopeAccountAddressed {
		return s.authorizedAccountEvent(event)
	}
	if s.signedByLocalRoot(event) {
		return true
	}
	content := event.Content
	peer, ok := s.Peers[content.InstallationID]
	if !ok || !peer.Trusted || peer.SignerKeyID != event.Nostr.PubKey {
		return false
	}
	if content.Type == TypeThreadCancel {
		root, ok := s.Records[content.ThreadID]
		return ok && root.Status == StatusProjected && root.Event.Content.Sender != nil &&
			root.Event.Content.Sender.InstallationID == content.InstallationID && *root.Event.Content.Sender == *content.Sender
	}
	if content.Type != TypeQuestion && content.Type != TypeAnswer && content.Type != TypeMessage {
		return false
	}
	if content.Sender == nil || content.Recipient == nil || content.Sender.InstallationID != content.InstallationID || content.Recipient.InstallationID != s.Policy.InstallationID {
		return false
	}
	if content.Recipient.MailboxID == s.Policy.HumanMailboxID {
		return true
	}
	if content.Type == TypeAnswer {
		root, ok := s.Records[content.ThreadID]
		if ok && root.Status == StatusProjected && root.Event.Content.Type == TypeQuestion && root.Event.Content.Sender != nil && root.Event.Content.Recipient != nil &&
			*root.Event.Content.Sender == *content.Recipient && *root.Event.Content.Recipient == *content.Sender {
			return true
		}
	}
	share, ok := s.Shares[shareKey(content.Recipient.MailboxID, content.InstallationID)]
	return ok && share.Active
}

func (s *State) authorizedAccountEvent(item SignedEvent) bool {
	content := item.Content
	if content.Audience == nil || !s.accountMembershipAt(content.Audience.HumanAccountID, content.InstallationID, item.Nostr.PubKey, item.ID()) {
		return false
	}
	humanMailbox := s.Policy.HumanMailboxID
	switch content.Type {
	case TypeQuestion:
		return content.Sender != nil && content.Recipient == nil && content.Sender.InstallationID == content.InstallationID && content.Sender.MailboxID != humanMailbox
	case TypeMessage:
		return content.Sender != nil && content.Recipient != nil && content.Sender.InstallationID == content.InstallationID && content.Sender.MailboxID == humanMailbox && content.Recipient.MailboxID != humanMailbox
	case TypeAnswer:
		if content.Sender == nil || content.Recipient == nil || content.Sender.InstallationID != content.InstallationID || content.Sender.MailboxID != humanMailbox {
			return false
		}
		root, ok := s.Records[content.ThreadID]
		return ok && root.Status == StatusProjected && root.Event.Content.Type == TypeQuestion && root.Event.Content.Audience != nil && root.Event.Content.Audience.HumanAccountID == content.Audience.HumanAccountID && root.Event.Content.Sender != nil && *root.Event.Content.Sender == *content.Recipient
	case TypeThreadCancel:
		root, ok := s.Records[content.ThreadID]
		return ok && root.Status == StatusProjected && root.Event.Content.Type == TypeQuestion && root.Event.Content.Audience != nil && root.Event.Content.Audience.HumanAccountID == content.Audience.HumanAccountID && root.Event.Content.Sender != nil && content.Sender != nil && *root.Event.Content.Sender == *content.Sender
	case TypeMessageArchive, TypeMessageRestore, TypeMessageReject:
		if content.Sender == nil || content.Sender.InstallationID != content.InstallationID || content.Sender.MailboxID != humanMailbox {
			return false
		}
		var payload TargetPayload
		_ = decodePayload(content.Payload, &payload)
		target, ok := s.Records[payload.TargetEventID]
		return ok && target.Event.Content.Audience != nil && target.Event.Content.Audience.HumanAccountID == content.Audience.HumanAccountID
	case TypeProjectEvent:
		return true
	case TypeProjectCommand, TypeProjectResult:
		return content.Sender != nil && content.Recipient != nil && content.Sender.MailboxID == s.Policy.HumanMailboxID && content.Recipient.MailboxID == s.Policy.HumanMailboxID
	default:
		return false
	}
}

func (s *State) accountMembershipAt(accountID, installationID, signerKeyID, eventID string) bool {
	account, ok := s.Accounts[accountID]
	if !ok {
		return false
	}
	if installationID == account.CreatorInstallationID {
		return signerKeyID == account.CreatorSignerKeyID && s.ancestor(account.CreationEventID, eventID)
	}
	device, ok := account.Devices[installationID]
	if !ok || device.SignerKeyID != signerKeyID {
		return false
	}
	var facts []Record
	for _, record := range s.Records {
		if record.Status != StatusProjected || !isHumanDeviceType(record.Event.Content.Type) || !s.ancestor(record.Event.ID(), eventID) {
			continue
		}
		var payload HumanDevicePayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		if payload.AccountID == accountID && payload.InstallationID == installationID && payload.SignerKeyID == signerKeyID {
			facts = append(facts, record)
		}
	}
	maxima := s.maximal(facts)
	if len(maxima) == 0 {
		return false
	}
	for _, record := range maxima {
		if record.Event.Content.Type != TypeHumanDeviceAccept {
			return false
		}
	}
	return true
}

func (s *State) parentsUsable(event SignedEvent, memo map[string]bool) bool {
	for _, parent := range event.Content.Parents {
		if !s.causalEventUsable(parent, memo, make(map[string]bool)) {
			return false
		}
	}
	return true
}

func (s *State) causalEventUsable(id string, memo, visiting map[string]bool) bool {
	if usable, ok := memo[id]; ok {
		return usable
	}
	if visiting[id] {
		return false
	}
	visiting[id] = true
	record, ok := s.Records[id]
	if !ok || record.Status != StatusProjected {
		memo[id] = false
		return false
	}
	for _, parent := range record.Event.Content.Parents {
		if !s.causalEventUsable(parent, memo, visiting) {
			memo[id] = false
			return false
		}
	}
	delete(visiting, id)
	memo[id] = true
	return true
}

func (s *State) maximal(records []Record) []Record {
	result := make([]Record, 0, len(records))
	for _, candidate := range records {
		maximal := true
		for _, other := range records {
			if candidate.Event.ID() != other.Event.ID() && s.ancestor(candidate.Event.ID(), other.Event.ID()) {
				maximal = false
				break
			}
		}
		if maximal {
			result = append(result, candidate)
		}
	}
	sort.Slice(result, func(i, j int) bool { return result[i].Event.ID() < result[j].Event.ID() })
	return result
}

func (s *State) ancestor(ancestorID, eventID string) bool {
	seen := make(map[string]bool)
	var visit func(string) bool
	visit = func(id string) bool {
		if id == ancestorID {
			return true
		}
		if seen[id] {
			return false
		}
		seen[id] = true
		record, ok := s.Records[id]
		if !ok {
			return false
		}
		for _, parent := range record.Event.Content.Parents {
			if visit(parent) {
				return true
			}
		}
		return false
	}
	return visit(eventID)
}

func (s *State) projectMailboxes() {
	ids := make([]string, 0, len(s.Records))
	for id := range s.Records {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	for _, id := range ids {
		record := s.Records[id]
		if record.Status != StatusProjected {
			continue
		}
		if record.Event.Content.Type == TypeMailboxCreate {
			var payload MailboxPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			s.Mailboxes[payload.MailboxID] = MailboxProjection{ID: payload.MailboxID, Kind: payload.Kind, Label: payload.Label}
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeMailboxBind {
			var payload MailboxBindingPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			mailbox := s.Mailboxes[payload.MailboxID]
			mailbox.ID, mailbox.Harness, mailbox.ExternalSessionID = payload.MailboxID, payload.Harness, payload.ExternalSessionID
			mailbox.Bindings = append(mailbox.Bindings, payload)
			s.Mailboxes[payload.MailboxID] = mailbox
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeMailboxContext {
			var payload MailboxContextPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			mailbox := s.Mailboxes[payload.MailboxID]
			mailbox.ID = payload.MailboxID
			mailbox.Contexts = append(mailbox.Contexts, payload.Context)
			s.Mailboxes[payload.MailboxID] = mailbox
		}
	}
}

func (s *State) classifyNamedAgents() {
	names, mailboxes := make(map[string]string), make(map[string]string)
	for _, id := range sortedRecordIDs(s.Records) {
		record := s.Records[id]
		if record.Status != StatusProjected || record.Event.Content.Type != TypeAgentNameClaim {
			continue
		}
		var payload AgentNamePayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		mailbox, exists := s.Mailboxes[payload.MailboxID]
		reason := ""
		if !exists || mailbox.Kind != "agent" {
			reason = "agent name claim needs an existing agent mailbox"
		} else if existing, ok := names[payload.Name]; ok && existing != payload.MailboxID {
			reason = "agent name has conflicting mailbox claims"
		} else if existing, ok := mailboxes[payload.MailboxID]; ok && existing != payload.Name {
			reason = "agent mailbox has conflicting name claims"
		}
		if reason != "" {
			record.Status, record.Reason = StatusUnresolved, reason
			s.Records[id] = record
			continue
		}
		names[payload.Name], mailboxes[payload.MailboxID] = payload.MailboxID, payload.Name
	}
	for _, id := range sortedRecordIDs(s.Records) {
		record := s.Records[id]
		if record.Status != StatusProjected || (record.Event.Content.Type != TypeAgentRetire && record.Event.Content.Type != TypeAgentSessionSelect && record.Event.Content.Type != TypeAgentSessionRename) {
			continue
		}
		var name, mailboxID string
		if record.Event.Content.Type == TypeAgentRetire {
			var payload AgentNamePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			name, mailboxID = payload.Name, payload.MailboxID
		} else if record.Event.Content.Type == TypeAgentSessionSelect {
			var payload AgentSessionPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			name, mailboxID = payload.Name, payload.MailboxID
			matchedBinding := false
			for _, binding := range s.Mailboxes[mailboxID].Bindings {
				if binding.Harness == payload.Harness && binding.ExternalSessionID == payload.ExternalSessionID {
					matchedBinding = true
					break
				}
			}
			if !matchedBinding {
				record.Status, record.Reason = StatusUnresolved, "agent session selection has no matching mailbox binding"
				s.Records[id] = record
				continue
			}
		} else {
			var payload AgentSessionRenamePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			name, mailboxID = payload.Name, payload.MailboxID
			matchedSession := false
			for _, binding := range s.Mailboxes[mailboxID].Bindings {
				if binding.Harness == payload.Harness && binding.ExternalSessionID == payload.ExternalSessionID {
					matchedSession = true
					break
				}
			}
			if !matchedSession {
				record.Status, record.Reason = StatusUnresolved, "agent session rename has no matching mailbox binding"
				s.Records[id] = record
				continue
			}
		}
		if names[name] != mailboxID {
			record.Status, record.Reason = StatusUnresolved, "agent fact has no matching name claim"
			s.Records[id] = record
		}
	}
}

func (s *State) projectNamedAgents() {
	ids := sortedRecordIDs(s.Records)
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeAgentNameClaim {
			var payload AgentNamePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			s.NamedAgents[payload.Name] = NamedAgentProjection{Name: payload.Name, MailboxID: payload.MailboxID, Sessions: make(map[string]AgentSessionProjection)}
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeAgentRetire {
			var payload AgentNamePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			agent := s.NamedAgents[payload.Name]
			agent.Retired = true
			s.NamedAgents[payload.Name] = agent
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeAgentSessionSelect {
			var payload AgentSessionPayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			agent := s.NamedAgents[payload.Name]
			created := record.Event.Nostr.CreatedAt
			key := payload.Harness + "\x00" + payload.ExternalSessionID
			session := agent.Sessions[key]
			if session.ExternalSessionID == "" {
				session = AgentSessionProjection{Harness: payload.Harness, ExternalSessionID: payload.ExternalSessionID, Context: payload.Context, CreatedAt: created}
			}
			if created >= session.LastSelectedAt {
				session.Context, session.LastSelectedAt = payload.Context, created
			}
			agent.Sessions[key] = session
			if agent.SelectionEventID == "" || s.ancestor(agent.SelectionEventID, id) || (created > agent.SelectedAt && !s.ancestor(id, agent.SelectionEventID)) || (created == agent.SelectedAt && !s.ancestor(id, agent.SelectionEventID) && id > agent.SelectionEventID) {
				agent.Harness, agent.ExternalSessionID = payload.Harness, payload.ExternalSessionID
				agent.SelectedAt, agent.SelectionEventID = created, id
			}
			s.NamedAgents[payload.Name] = agent
		}
	}
	for _, id := range ids {
		record := s.Records[id]
		if record.Status == StatusProjected && record.Event.Content.Type == TypeAgentSessionRename {
			var payload AgentSessionRenamePayload
			_ = decodePayload(record.Event.Content.Payload, &payload)
			agent := s.NamedAgents[payload.Name]
			key := payload.Harness + "\x00" + payload.ExternalSessionID
			session := agent.Sessions[key]
			created := record.Event.Nostr.CreatedAt
			if session.NameEventID == "" || s.ancestor(session.NameEventID, id) || (created > session.NameUpdatedAt && !s.ancestor(id, session.NameEventID)) || (created == session.NameUpdatedAt && !s.ancestor(id, session.NameEventID) && id > session.NameEventID) {
				session.ThreadName, session.NameUpdatedAt, session.NameEventID = payload.ThreadName, created, id
				agent.Sessions[key] = session
				s.NamedAgents[payload.Name] = agent
			}
		}
	}
}

func (s *State) projectMessages() {
	for id, record := range s.Records {
		if record.Status != StatusProjected && record.Status != StatusUnresolved {
			continue
		}
		content := record.Event.Content
		if content.Type != TypeQuestion && content.Type != TypeAnswer && content.Type != TypeMessage {
			continue
		}
		payload, err := decodeTextPayload(content.Payload, content.Schema)
		if err != nil {
			continue
		}
		if content.Schema == Schema1 {
			payload = projectLegacySchema1Message(payload)
		}
		threadID := content.ThreadID
		if threadID == "" {
			threadID = id
		}
		recipient := content.Recipient
		audienceAccountID := ""
		if content.Audience != nil {
			audienceAccountID = content.Audience.HumanAccountID
			if content.Type == TypeQuestion {
				recipient = &MailboxAddress{InstallationID: s.Policy.InstallationID, MailboxID: s.Policy.HumanMailboxID}
			}
		}
		if content.Sender == nil || recipient == nil {
			continue
		}
		s.Messages[id] = MessageProjection{
			ID: id, Type: content.Type, Sender: *content.Sender, Recipient: *recipient,
			ThreadID: threadID, Parents: append([]string(nil), content.Parents...), Body: payload.Body,
			MessageID: payload.MessageID, AudienceAccountID: audienceAccountID, ActorLabel: payload.ActorLabel, Context: payload.Context, Details: payload.Details, Purpose: model.NormalizeMessagePurpose(payload.Purpose),
			Presentation: payload.Presentation, Correlation: payload.Correlation, TechnicalSections: cloneTechnicalSections(payload.TechnicalSections), CreatedAt: time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(),
			Incomplete: record.Status == StatusUnresolved,
		}
	}
	for id, message := range s.Messages {
		if message.Incomplete {
			continue
		}
		for _, parentID := range message.Parents {
			parent, ok := s.Messages[parentID]
			if ok && parent.Recipient == message.Sender && parent.Sender == message.Recipient {
				parent.PeerReceived = true
				s.Messages[parentID] = parent
			}
		}
		s.Messages[id] = message
	}
}

func (s *State) applyMessageState() {
	groups := make(map[string][]Record)
	for _, record := range s.Records {
		if record.Status != StatusProjected {
			continue
		}
		if record.Event.Content.Type != TypeMessageArchive && record.Event.Content.Type != TypeMessageRestore && record.Event.Content.Type != TypeMessageReject {
			continue
		}
		var payload TargetPayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		if _, ok := s.Messages[payload.TargetEventID]; ok {
			groups[payload.TargetEventID] = append(groups[payload.TargetEventID], record)
		}
	}
	for target, records := range groups {
		message, ok := s.Messages[target]
		if !ok {
			continue
		}
		for _, record := range records {
			if record.Event.Content.Type == TypeMessageReject {
				message.Rejected = true
				message.Archived = true
				if at := time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(); at.After(message.ArchivedAt) {
					message.ArchivedAt = at
				}
			}
		}
		if !message.Rejected {
			for _, record := range s.maximal(records) {
				if record.Event.Content.Type == TypeMessageArchive {
					message.Archived = true
					if at := time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(); at.After(message.ArchivedAt) {
						message.ArchivedAt = at
					}
				}
			}
		}
		s.Messages[target] = message
	}
}

func (s *State) projectThreads() {
	for id, message := range s.Messages {
		if message.Incomplete {
			continue
		}
		thread := s.Threads[message.ThreadID]
		thread.ID = message.ThreadID
		thread.MessageIDs = append(thread.MessageIDs, id)
		if message.Type == TypeAnswer {
			thread.AnswerIDs = append(thread.AnswerIDs, id)
			thread.Answered = true
		}
		s.Threads[message.ThreadID] = thread
	}
	for id, record := range s.Records {
		if record.Status != StatusProjected || record.Event.Content.Type != TypeThreadCancel {
			continue
		}
		thread := s.Threads[record.Event.Content.ThreadID]
		if thread.ID == "" {
			continue
		}
		thread.Cancelled = true
		thread.CancellationIDs = append(thread.CancellationIDs, id)
		s.Threads[thread.ID] = thread
	}
	for id, thread := range s.Threads {
		sort.Strings(thread.MessageIDs)
		sort.Strings(thread.AnswerIDs)
		sort.Strings(thread.CancellationIDs)
		thread.AnswerCancellation = make(map[string]map[string]CancellationRelation)
		for _, answerID := range thread.AnswerIDs {
			thread.AnswerCancellation[answerID] = make(map[string]CancellationRelation)
			for _, cancellationID := range thread.CancellationIDs {
				relation := AnswerConcurrent
				if s.ancestor(cancellationID, answerID) {
					relation = AnswerAfterCancellation
				} else if s.ancestor(answerID, cancellationID) {
					relation = AnswerBeforeCancellation
				}
				thread.AnswerCancellation[answerID][cancellationID] = relation
			}
		}
		s.Threads[id] = thread
	}
}

func (s *State) orderMessages() []string {
	remaining := make(map[string]int, len(s.Messages))
	children := make(map[string][]string)
	for id, message := range s.Messages {
		remaining[id] = 0
		for _, parent := range message.Parents {
			if _, ok := s.Messages[parent]; ok {
				remaining[id]++
				children[parent] = append(children[parent], id)
			}
		}
	}
	ready := make([]string, 0)
	for id, count := range remaining {
		if count == 0 {
			ready = append(ready, id)
		}
	}
	less := func(one, two string) bool {
		a, b := s.Messages[one], s.Messages[two]
		if a.CreatedAt.Equal(b.CreatedAt) {
			return a.ID < b.ID
		}
		return a.CreatedAt.Before(b.CreatedAt)
	}
	var ordered []string
	for len(ready) > 0 {
		sort.Slice(ready, func(i, j int) bool { return less(ready[i], ready[j]) })
		id := ready[0]
		ready = ready[1:]
		ordered = append(ordered, id)
		for _, child := range children[id] {
			remaining[child]--
			if remaining[child] == 0 {
				ready = append(ready, child)
			}
		}
	}
	return ordered
}

func (s State) Get(eventID string) (MessageProjection, error) {
	message, ok := s.Messages[eventID]
	if !ok {
		return MessageProjection{}, ErrNotFound
	}
	return message, nil
}

func (s State) Poll(mailbox MailboxAddress) []MessageProjection {
	var result []MessageProjection
	for _, id := range s.DisplayOrder {
		message := s.Messages[id]
		if message.Recipient == mailbox {
			result = append(result, message)
		}
	}
	return result
}

// AccountActionParents returns the current causal membership frontier for an
// active account device. Callers include these IDs in every account action.
func (s State) AccountActionParents(accountID, installationID string) ([]string, bool) {
	account, ok := s.Accounts[accountID]
	if !ok {
		return nil, false
	}
	if installationID == account.CreatorInstallationID {
		return []string{account.CreationEventID}, true
	}
	var facts []Record
	for _, record := range s.Records {
		if record.Status != StatusProjected || !isHumanDeviceType(record.Event.Content.Type) {
			continue
		}
		var payload HumanDevicePayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		if payload.AccountID == accountID && payload.InstallationID == installationID {
			facts = append(facts, record)
		}
	}
	maxima := s.maximal(facts)
	if len(maxima) == 0 {
		return nil, false
	}
	parents := make([]string, 0, len(maxima))
	for _, record := range maxima {
		if record.Event.Content.Type != TypeHumanDeviceAccept {
			return nil, false
		}
		parents = append(parents, record.Event.ID())
	}
	sort.Strings(parents)
	return parents, true
}

func (s State) Wait(questionID string, mailbox MailboxAddress) (MessageProjection, error) {
	question, ok := s.Messages[questionID]
	if !ok || question.Incomplete || question.Type != TypeQuestion {
		return MessageProjection{}, ErrNotFound
	}
	if question.Sender != mailbox {
		return MessageProjection{}, ErrWaitDenied
	}
	for _, id := range s.DisplayOrder {
		message := s.Messages[id]
		if message.Type == TypeAnswer && !message.Incomplete && message.ThreadID == questionID && s.ancestor(questionID, id) {
			return message, nil
		}
	}
	return MessageProjection{}, ErrNoAnswer
}

func isControlType(kind Type) bool {
	switch kind {
	case TypeInstallationCreate, TypeMailboxCreate, TypeMailboxBind, TypeMailboxContext, TypeAgentNameClaim, TypeAgentRetire, TypeAgentSessionSelect, TypeAgentSessionRename, TypePeerTrust, TypePeerDistrust, TypeMailboxShare, TypeMailboxShareRevoke, TypeHumanAccountSelect:
		return true
	default:
		return false
	}
}

func isAccountAuthorityType(kind Type) bool {
	return kind == TypeHumanAccountCreate || isHumanDeviceType(kind)
}

func isHumanDeviceType(kind Type) bool {
	return kind == TypeHumanDeviceGrant || kind == TypeHumanDeviceAccept || kind == TypeHumanDeviceRevoke
}

func shareKey(mailboxID, peerInstallationID string) string {
	return fmt.Sprintf("%s:%s", peerInstallationID, mailboxID)
}
