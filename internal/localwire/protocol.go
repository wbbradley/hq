package localwire

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"
)

const (
	CurrentDomainVersion    = 6
	CurrentLifecycleVersion = 1
)

var (
	DomainVersions    = VersionRange{Min: CurrentDomainVersion, Max: CurrentDomainVersion}
	LifecycleVersions = VersionRange{Min: CurrentLifecycleVersion, Max: CurrentLifecycleVersion}
)

type HandshakeMode string

const (
	DomainMode    HandshakeMode = "domain"
	LifecycleMode HandshakeMode = "lifecycle"
)

type EnvelopeKind string

const (
	HandshakeKind    EnvelopeKind = "handshake"
	RequestKind      EnvelopeKind = "request"
	ResponseKind     EnvelopeKind = "response"
	ErrorKind        EnvelopeKind = "error"
	NotificationKind EnvelopeKind = "notification"
)

const (
	CodeInvalidRequest       = "invalid_request"
	CodeMethodNotFound       = "method_not_found"
	CodeInternal             = "internal"
	CodeIncompatibleVersion  = "incompatible_wire_version"
	CodeNotificationOverflow = "notification_overflow"
)

type VersionRange struct {
	Min int `json:"min"`
	Max int `json:"max"`
}

func (r VersionRange) Validate() error {
	if r.Min < 1 || r.Max < r.Min {
		return fmt.Errorf("invalid wire version range %d-%d", r.Min, r.Max)
	}
	return nil
}

func Negotiate(client, server VersionRange) (int, error) {
	if err := client.Validate(); err != nil {
		return 0, err
	}
	if err := server.Validate(); err != nil {
		return 0, err
	}
	lowestMaximum := min(client.Max, server.Max)
	if lowestMaximum < max(client.Min, server.Min) {
		return 0, NewIncompatibility(client, server)
	}
	return lowestMaximum, nil
}

type PeerMetadata struct {
	Build      string    `json:"build"`
	InstanceID string    `json:"instance_id,omitempty"`
	StartedAt  time.Time `json:"started_at,omitempty"`
}

type HandshakeRequest struct {
	Mode      HandshakeMode `json:"mode"`
	Supported VersionRange  `json:"supported"`
	Client    PeerMetadata  `json:"client"`
}

type HandshakeResponse struct {
	Mode      HandshakeMode `json:"mode"`
	Version   int           `json:"version"`
	Supported VersionRange  `json:"supported"`
	Server    PeerMetadata  `json:"server"`
}

type RPCError struct {
	Code    string          `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

func (e *RPCError) Error() string {
	if e == nil {
		return ""
	}
	return e.Message
}

type IncompatibilityData struct {
	Client    VersionRange `json:"client"`
	Server    VersionRange `json:"server"`
	StaleSide string       `json:"stale_side"`
	Action    string       `json:"action"`
}

type IncompatibilityError struct {
	Data IncompatibilityData
}

func NewIncompatibility(client, server VersionRange) *IncompatibilityError {
	data := IncompatibilityData{Client: client, Server: server}
	if client.Max < server.Min {
		data.StaleSide = "client"
		data.Action = "upgrade this HQ client"
	} else {
		data.StaleSide = "server"
		data.Action = "restart or upgrade the local HQ node"
	}
	return &IncompatibilityError{Data: data}
}

func (e *IncompatibilityError) Error() string {
	return fmt.Sprintf("incompatible local wire versions (client %d-%d, server %d-%d; stale %s): %s",
		e.Data.Client.Min, e.Data.Client.Max, e.Data.Server.Min, e.Data.Server.Max,
		e.Data.StaleSide, e.Data.Action)
}

func (e *IncompatibilityError) RPCError() *RPCError {
	raw, _ := json.Marshal(e.Data)
	return &RPCError{Code: CodeIncompatibleVersion, Message: e.Error(), Data: raw}
}

func incompatibilityFromRPC(rpcErr *RPCError) error {
	if rpcErr == nil || rpcErr.Code != CodeIncompatibleVersion {
		return rpcErr
	}
	var data IncompatibilityData
	if err := json.Unmarshal(rpcErr.Data, &data); err != nil {
		return rpcErr
	}
	return &IncompatibilityError{Data: data}
}

type Envelope struct {
	Kind           EnvelopeKind    `json:"kind"`
	Version        int             `json:"version,omitempty"`
	ID             string          `json:"id,omitempty"`
	Method         string          `json:"method,omitempty"`
	SubscriptionID string          `json:"subscription_id,omitempty"`
	Params         json.RawMessage `json:"params,omitempty"`
	Result         json.RawMessage `json:"result,omitempty"`
	Error          *RPCError       `json:"error,omitempty"`
}

func (e Envelope) Validate() error {
	if e.Kind != NotificationKind && e.SubscriptionID != "" {
		return errors.New("subscription_id is only valid on notifications")
	}
	switch e.Kind {
	case HandshakeKind:
		if e.ID != "" || e.Method != "" || e.Error != nil || (len(e.Params) == 0) == (len(e.Result) == 0) {
			return errors.New("handshake envelope must contain exactly one of params or result")
		}
	case RequestKind:
		if e.Version < 1 || e.ID == "" || strings.TrimSpace(e.Method) == "" || e.Error != nil || len(e.Result) != 0 {
			return errors.New("request envelope needs version, id, and method")
		}
	case ResponseKind:
		if e.Version < 1 || e.ID == "" || e.Method != "" || e.Error != nil || len(e.Params) != 0 {
			return errors.New("response envelope needs version and id")
		}
	case ErrorKind:
		if e.ID == "" || e.Method != "" || e.Error == nil || len(e.Params) != 0 || len(e.Result) != 0 {
			return errors.New("error envelope needs id and error")
		}
	case NotificationKind:
		if e.Version < 1 || e.ID != "" || strings.TrimSpace(e.Method) == "" || e.Error != nil || len(e.Result) != 0 {
			return errors.New("notification envelope needs version and method")
		}
	default:
		return fmt.Errorf("unknown envelope kind %q", e.Kind)
	}
	return nil
}

func marshalRaw(value any) (json.RawMessage, error) {
	if value == nil {
		return json.RawMessage("null"), nil
	}
	return json.Marshal(value)
}
