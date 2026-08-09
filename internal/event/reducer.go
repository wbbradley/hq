package event

import (
	"errors"
	"fmt"
	"sort"
	"time"
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
	ID      string
	Kind    string
	Label   string
	Harness string
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

type MessageProjection struct {
	ID           string
	Type         Type
	Sender       MailboxAddress
	Recipient    MailboxAddress
	ThreadID     string
	Parents      []string
	Body         string
	Details      string
	CreatedAt    time.Time
	Incomplete   bool
	Archived     bool
	Rejected     bool
	PeerReceived bool
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
	Policy       Policy
	Records      map[string]Record
	Invalid      []Record
	Mailboxes    map[string]MailboxProjection
	Peers        map[string]PeerProjection
	Shares       map[string]MailboxShareProjection
	Messages     map[string]MessageProjection
	Threads      map[string]ThreadProjection
	DisplayOrder []string
}

// Reduce verifies and reduces a complete event set. Callers may pass the events
// in any order and may pass the same event more than once.
func Reduce(rawEvents [][]byte, policy Policy) State {
	state := State{
		Policy:    policy,
		Records:   make(map[string]Record),
		Mailboxes: make(map[string]MailboxProjection),
		Peers:     make(map[string]PeerProjection),
		Shares:    make(map[string]MailboxShareProjection),
		Messages:  make(map[string]MessageProjection),
		Threads:   make(map[string]ThreadProjection),
	}
	for _, raw := range rawEvents {
		schemas := policy.SchemaVersions
		if len(schemas) == 0 {
			schemas = []int{SchemaVersion}
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
	state.classifyUnsupported()
	state.reduceShares()
	state.classifyDomainEvents()
	state.projectMailboxes()
	state.projectMessages()
	state.applyMessageState()
	state.projectThreads()
	state.DisplayOrder = state.orderMessages()
	return state
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
		if record.Status != StatusProjected || isControlType(record.Event.Content.Type) {
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
		if record.Status == StatusProjected && !isControlType(record.Event.Content.Type) && !s.parentsUsable(record.Event, memo) {
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
		if record.Status == StatusProjected && !isControlType(record.Event.Content.Type) {
			if record.Event.Content.Type == TypeMessageArchive || record.Event.Content.Type == TypeMessageReject {
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
			mailbox.ID, mailbox.Harness = payload.MailboxID, payload.Harness
			s.Mailboxes[payload.MailboxID] = mailbox
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
		var payload TextPayload
		_ = decodePayload(content.Payload, &payload)
		threadID := content.ThreadID
		if threadID == "" {
			threadID = id
		}
		s.Messages[id] = MessageProjection{
			ID: id, Type: content.Type, Sender: *content.Sender, Recipient: *content.Recipient,
			ThreadID: threadID, Parents: append([]string(nil), content.Parents...), Body: payload.Body,
			Details: payload.Details, CreatedAt: time.Unix(record.Event.Nostr.CreatedAt, 0).UTC(),
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
	for _, record := range s.Records {
		if record.Status != StatusProjected {
			continue
		}
		if record.Event.Content.Type != TypeMessageArchive && record.Event.Content.Type != TypeMessageReject {
			continue
		}
		var payload TargetPayload
		_ = decodePayload(record.Event.Content.Payload, &payload)
		message, ok := s.Messages[payload.TargetEventID]
		if !ok {
			continue
		}
		message.Archived = true
		if record.Event.Content.Type == TypeMessageReject {
			message.Rejected = true
		}
		s.Messages[payload.TargetEventID] = message
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
	case TypeInstallationCreate, TypeMailboxCreate, TypeMailboxBind, TypePeerTrust, TypePeerDistrust, TypeMailboxShare, TypeMailboxShareRevoke:
		return true
	default:
		return false
	}
}

func shareKey(mailboxID, peerInstallationID string) string {
	return fmt.Sprintf("%s:%s", peerInstallationID, mailboxID)
}
