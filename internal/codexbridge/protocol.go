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
	ID     string `json:"id"`
	Status string `json:"status,omitempty"`
}

type TurnResponse struct {
	Turn Turn `json:"turn"`
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
