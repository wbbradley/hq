// Package event is the stable facade for HQ canonical wire values and their
// pure domain reduction. Wire inspection/signing lives in eventwire, while
// eventstate owns the effect-free projection model.
package event

import (
	"encoding/json"
	"time"

	"github.com/wbbradley/hq/internal/eventstate"
	"github.com/wbbradley/hq/internal/eventwire"
	"github.com/wbbradley/hq/internal/model"
)

const (
	Kind                           = eventwire.Kind
	Schema3                        = eventwire.Schema3
	SchemaVersion                  = eventwire.SchemaVersion
	MessageSchemaVersion           = eventwire.MessageSchemaVersion
	MaxWireBytes                   = eventwire.MaxWireBytes
	MaxBodyBytes                   = eventwire.MaxBodyBytes
	MaxDetailBytes                 = eventwire.MaxDetailBytes
	MaxParents                     = eventwire.MaxParents
	MaxCorrelationProviderBytes    = eventwire.MaxCorrelationProviderBytes
	MaxCorrelationIDBytes          = eventwire.MaxCorrelationIDBytes
	MaxTechnicalSections           = eventwire.MaxTechnicalSections
	MaxTechnicalFieldsPerSection   = eventwire.MaxTechnicalFieldsPerSection
	MaxTechnicalFields             = eventwire.MaxTechnicalFields
	MaxTechnicalNamespaceBytes     = eventwire.MaxTechnicalNamespaceBytes
	MaxTechnicalKeyBytes           = eventwire.MaxTechnicalKeyBytes
	MaxTechnicalLabelBytes         = eventwire.MaxTechnicalLabelBytes
	MaxTechnicalValueBytes         = eventwire.MaxTechnicalValueBytes
	MaxTechnicalPayloadBytes       = eventwire.MaxTechnicalPayloadBytes
	MaxHarnessActivityTitleBytes   = eventwire.MaxHarnessActivityTitleBytes
	MaxHarnessActivityBodyBytes    = eventwire.MaxHarnessActivityBodyBytes
	MaxHarnessActivityRuntimeBytes = eventwire.MaxHarnessActivityRuntimeBytes

	TypeInstallationCreate   = eventwire.TypeInstallationCreate
	TypeMailboxCreate        = eventwire.TypeMailboxCreate
	TypeMailboxBind          = eventwire.TypeMailboxBind
	TypeMailboxContext       = eventwire.TypeMailboxContext
	TypeAgentNameClaim       = eventwire.TypeAgentNameClaim
	TypeAgentRetire          = eventwire.TypeAgentRetire
	TypeAgentSessionSelect   = eventwire.TypeAgentSessionSelect
	TypeAgentSessionRename   = eventwire.TypeAgentSessionRename
	TypeQuestion             = eventwire.TypeQuestion
	TypeAnswer               = eventwire.TypeAnswer
	TypeMessage              = eventwire.TypeMessage
	TypeThreadCancel         = eventwire.TypeThreadCancel
	TypeMessageArchive       = eventwire.TypeMessageArchive
	TypeMessageRestore       = eventwire.TypeMessageRestore
	TypeMessageReject        = eventwire.TypeMessageReject
	TypePeerBindingSet       = eventwire.TypePeerBindingSet
	TypePeerBindingBlock     = eventwire.TypePeerBindingBlock
	TypeMailboxAccessGrant   = eventwire.TypeMailboxAccessGrant
	TypeMailboxAccessRevoke  = eventwire.TypeMailboxAccessRevoke
	TypeMailboxAccessObserve = eventwire.TypeMailboxAccessObserve
	TypeHumanAccountCreate   = eventwire.TypeHumanAccountCreate
	TypeHumanAccountSelect   = eventwire.TypeHumanAccountSelect
	TypeHumanDeviceGrant     = eventwire.TypeHumanDeviceGrant
	TypeHumanDeviceAccept    = eventwire.TypeHumanDeviceAccept
	TypeHumanDeviceRevoke    = eventwire.TypeHumanDeviceRevoke
	TypeProjectEvent         = eventwire.TypeProjectEvent
	TypeProjectCommand       = eventwire.TypeProjectCommand
	TypeProjectResult        = eventwire.TypeProjectResult
	TypeHarnessActivity      = eventwire.TypeHarnessActivity

	ScopeInstallationPrivate = eventwire.ScopeInstallationPrivate
	ScopePeerAddressed       = eventwire.ScopePeerAddressed
	ScopeAccountAddressed    = eventwire.ScopeAccountAddressed
	ScopePublic              = eventwire.ScopePublic

	StatusProjected    = eventwire.StatusProjected
	StatusUnresolved   = eventwire.StatusUnresolved
	StatusUnsupported  = eventwire.StatusUnsupported
	StatusInvalid      = eventwire.StatusInvalid
	StatusUnauthorized = eventwire.StatusUnauthorized

	AnswerBeforeCancellation = eventstate.AnswerBeforeCancellation
	AnswerAfterCancellation  = eventstate.AnswerAfterCancellation
	AnswerConcurrent         = eventstate.AnswerConcurrent
)

type Type = eventwire.Type
type Scope = eventwire.Scope
type ProjectionStatus = eventwire.ProjectionStatus
type MailboxAddress = eventwire.MailboxAddress
type Origin = eventwire.Origin
type Audience = eventwire.Audience
type Content = eventwire.Content
type TextPayload = eventwire.TextPayload
type HarnessActivityPayload = eventwire.HarnessActivityPayload
type RepositoryContext = eventwire.RepositoryContext
type InstallationPayload = eventwire.InstallationPayload
type MailboxPayload = eventwire.MailboxPayload
type MailboxBindingPayload = eventwire.MailboxBindingPayload
type MailboxContextPayload = eventwire.MailboxContextPayload
type AgentNamePayload = eventwire.AgentNamePayload
type AgentSessionPayload = eventwire.AgentSessionPayload
type AgentSessionRenamePayload = eventwire.AgentSessionRenamePayload
type TargetPayload = eventwire.TargetPayload
type PeerPayload = eventwire.PeerPayload
type MailboxAccessPayload = eventwire.MailboxAccessPayload
type MailboxAccessObservationPayload = eventwire.MailboxAccessObservationPayload
type HumanAccountPayload = eventwire.HumanAccountPayload
type HumanAccountSelectionPayload = eventwire.HumanAccountSelectionPayload
type HumanDevicePayload = eventwire.HumanDevicePayload
type ProjectEventPayload = eventwire.ProjectEventPayload
type ProjectCommandPayload = eventwire.ProjectCommandPayload
type ProjectCommandResultPayload = eventwire.ProjectCommandResultPayload
type SecretKey = eventwire.SecretKey
type NostrEvent = eventwire.NostrEvent
type SignedEvent = eventwire.SignedEvent
type Inspection = eventwire.Inspection

type Policy = eventstate.Policy
type Record = eventstate.Record
type MailboxProjection = eventstate.MailboxProjection
type NamedAgentProjection = eventstate.NamedAgentProjection
type AgentSessionProjection = eventstate.AgentSessionProjection
type PeerProjection = eventstate.PeerProjection
type MailboxAccessProjection = eventstate.MailboxAccessProjection
type HumanAccountProjection = eventstate.HumanAccountProjection
type HumanDeviceProjection = eventstate.HumanDeviceProjection
type MessageProjection = eventstate.MessageProjection
type HarnessActivityProjection = eventstate.HarnessActivityProjection
type CancellationRelation = eventstate.CancellationRelation
type ThreadProjection = eventstate.ThreadProjection
type State = eventstate.State

var (
	ErrNotFound   = eventstate.ErrNotFound
	ErrWaitDenied = eventstate.ErrWaitDenied
	ErrNoAnswer   = eventstate.ErrNoAnswer
)

func SecretKeyFromHex(value string) (SecretKey, error)  { return eventwire.SecretKeyFromHex(value) }
func MustSecretKeyFromHex(value string) SecretKey       { return eventwire.MustSecretKeyFromHex(value) }
func MarshalPayload(value any) (json.RawMessage, error) { return eventwire.MarshalPayload(value) }
func Sign(content Content, createdAt time.Time, secret SecretKey) (SignedEvent, error) {
	return eventwire.Sign(content, createdAt, secret)
}
func Inspect(raw []byte) Inspection                  { return eventwire.Inspect(raw) }
func Reduce(rawEvents [][]byte, policy Policy) State { return eventstate.Reduce(rawEvents, policy) }

// decodePayload remains private compatibility for package tests. Production
// payload decoding is owned by the wire validator and pure reducer.
func decodePayload(raw json.RawMessage, target any) error { return json.Unmarshal(raw, target) }

func validateHarnessActivityPayload(raw json.RawMessage) error {
	return eventwire.ValidateHarnessActivityPayload(raw)
}

func validateTechnicalSections(sections []model.TechnicalSection) error {
	return eventwire.ValidateTechnicalSections(sections)
}

func validateMessageCorrelation(correlation model.MessageCorrelation) error {
	return eventwire.ValidateMessageCorrelation(correlation)
}
