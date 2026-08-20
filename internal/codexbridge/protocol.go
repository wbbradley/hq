package codexbridge

import "encoding/json"

const TestedCodexVersion = "0.148.0"

const RequireStructuredHumanInput = "When progress requires an answer from the human, use the structured request_user_input tool."

type rpcEnvelope struct {
	JSONRPC string          `json:"jsonrpc,omitempty"`
	ID      json.RawMessage `json:"id,omitempty"`
	Method  string          `json:"method,omitempty"`
	Params  json.RawMessage `json:"params,omitempty"`
	Result  json.RawMessage `json:"result,omitempty"`
	Error   *RPCError       `json:"error,omitempty"`
}

type RPCError struct {
	Code    int             `json:"code"`
	Message string          `json:"message"`
	Data    json.RawMessage `json:"data,omitempty"`
}

func (e *RPCError) Error() string { return e.Message }

type ClientInfo struct {
	Name    string `json:"name"`
	Title   string `json:"title,omitempty"`
	Version string `json:"version"`
}

type InitializeCapabilities struct {
	ExperimentalAPI bool `json:"experimentalApi"`
}

type InitializeParams struct {
	ClientInfo   ClientInfo             `json:"clientInfo"`
	Capabilities InitializeCapabilities `json:"capabilities"`
}

type ThreadStartParams struct {
	CWD                   string `json:"cwd"`
	DeveloperInstructions string `json:"developerInstructions"`
}

type ThreadResumeParams struct {
	ThreadID string `json:"threadId"`
	CWD      string `json:"cwd,omitempty"`
}

type Thread struct {
	ID    string `json:"id"`
	CWD   string `json:"cwd,omitempty"`
	Turns []Turn `json:"turns,omitempty"`
}

type ThreadResponse struct {
	Thread Thread `json:"thread"`
}

type TextInput struct {
	Type string `json:"type"`
	Text string `json:"text"`
}

type TurnStartParams struct {
	ThreadID            string      `json:"threadId"`
	Input               []TextInput `json:"input"`
	ClientUserMessageID string      `json:"clientUserMessageId,omitempty"`
}

type Turn struct {
	ID     string       `json:"id"`
	Status string       `json:"status,omitempty"`
	Items  []ThreadItem `json:"items,omitempty"`
	Error  *TurnError   `json:"error,omitempty"`
}

type TurnError struct {
	Message           string          `json:"message"`
	CodexErrorInfo    json.RawMessage `json:"codexErrorInfo,omitempty"`
	AdditionalDetails string          `json:"additionalDetails,omitempty"`
}

type TurnResponse struct {
	Turn Turn `json:"turn"`
}

type ThreadItem struct {
	Type     string `json:"type"`
	ID       string `json:"id"`
	ClientID string `json:"clientId,omitempty"`
	Text     string `json:"text,omitempty"`
	Phase    string `json:"phase,omitempty"`
}

type ItemCompletedNotification struct {
	ThreadID string     `json:"threadId"`
	TurnID   string     `json:"turnId"`
	Item     ThreadItem `json:"item"`
}

type ThreadReadParams struct {
	ThreadID     string `json:"threadId"`
	IncludeTurns bool   `json:"includeTurns"`
}

type TurnSteerParams struct {
	ThreadID            string      `json:"threadId"`
	ExpectedTurnID      string      `json:"expectedTurnId"`
	Input               []TextInput `json:"input"`
	ClientUserMessageID string      `json:"clientUserMessageId,omitempty"`
}

type TurnSteerResponse struct {
	TurnID string `json:"turnId"`
}

type TurnNotification struct {
	ThreadID string `json:"threadId"`
	Turn     Turn   `json:"turn"`
}

type Notification struct {
	Method string
	Params json.RawMessage
}

type ServerRequest struct {
	ID     json.RawMessage
	Method string
	Params json.RawMessage
}

type RequestUserInputOption struct {
	Label       string `json:"label"`
	Description string `json:"description"`
}

type RequestUserInputQuestion struct {
	ID       string                   `json:"id"`
	Header   string                   `json:"header"`
	Question string                   `json:"question"`
	Options  []RequestUserInputOption `json:"options"`
	IsOther  bool                     `json:"isOther"`
	IsSecret bool                     `json:"isSecret"`
}

type RequestUserInputParams struct {
	ThreadID   string                     `json:"threadId"`
	TurnID     string                     `json:"turnId"`
	ItemID     string                     `json:"itemId"`
	IsBlocking bool                       `json:"isBlocking"`
	Questions  []RequestUserInputQuestion `json:"questions"`
}

type NetworkApprovalContext struct {
	Host     string `json:"host"`
	Protocol string `json:"protocol"`
}

type NetworkPolicyAmendment struct {
	Action string `json:"action"`
	Host   string `json:"host"`
}

type CommandApprovalParams struct {
	ThreadID                        string                  `json:"threadId"`
	TurnID                          string                  `json:"turnId"`
	ItemID                          string                  `json:"itemId"`
	ApprovalID                      string                  `json:"approvalId"`
	Command                         string                  `json:"command"`
	CWD                             string                  `json:"cwd"`
	Reason                          string                  `json:"reason"`
	CommandActions                  json.RawMessage         `json:"commandActions"`
	NetworkApprovalContext          *NetworkApprovalContext `json:"networkApprovalContext"`
	ProposedExecpolicyAmendment     []string                `json:"proposedExecpolicyAmendment"`
	ProposedNetworkPolicyAmendments []json.RawMessage       `json:"proposedNetworkPolicyAmendments"`
}

type FileChangeApprovalParams struct {
	ThreadID  string `json:"threadId"`
	TurnID    string `json:"turnId"`
	ItemID    string `json:"itemId"`
	Reason    string `json:"reason"`
	GrantRoot string `json:"grantRoot"`
}

type PermissionApprovalParams struct {
	ThreadID    string          `json:"threadId"`
	TurnID      string          `json:"turnId"`
	ItemID      string          `json:"itemId"`
	CWD         string          `json:"cwd"`
	Reason      string          `json:"reason"`
	Permissions json.RawMessage `json:"permissions"`
}

type MCPElicitationParams struct {
	ThreadID        string          `json:"threadId"`
	TurnID          string          `json:"turnId"`
	ServerName      string          `json:"serverName"`
	Mode            string          `json:"mode"`
	Message         string          `json:"message"`
	RequestedSchema json.RawMessage `json:"requestedSchema"`
	URL             string          `json:"url"`
	ElicitationID   string          `json:"elicitationId"`
}
