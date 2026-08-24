package codexbridge

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"io"
	"strings"
	"testing"
	"time"
)

func TestClientCallCorrelatesResponse(t *testing.T) {
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	client := NewClient(context.Background(), clientInput, clientOutput, nil, nil)
	go func() {
		defer serverOutput.Close()
		line, _ := bufio.NewReader(serverInput).ReadBytes('\n')
		var request struct {
			ID     int64            `json:"id"`
			Method string           `json:"method"`
			Params InitializeParams `json:"params"`
		}
		if err := json.Unmarshal(line, &request); err != nil {
			t.Errorf("decode request: %v", err)
			return
		}
		var raw map[string]any
		_ = json.Unmarshal(line, &raw)
		if _, exists := raw["jsonrpc"]; exists {
			t.Errorf("app-server wire request included jsonrpc header: %s", line)
		}
		if request.Method != "initialize" || !request.Params.Capabilities.ExperimentalAPI {
			t.Errorf("request = %#v", request)
		}
		_, _ = serverOutput.Write([]byte(`{"jsonrpc":"2.0","id":1,"result":{"ok":true}}` + "\n"))
	}()
	var result struct {
		OK bool `json:"ok"`
	}
	params := InitializeParams{Capabilities: InitializeCapabilities{ExperimentalAPI: true}}
	if err := client.Call(context.Background(), "initialize", params, &result); err != nil {
		t.Fatal(err)
	}
	if !result.OK {
		t.Fatal("response was not decoded")
	}
}

func TestClientRejectsMalformedAndOversizedFrames(t *testing.T) {
	tests := []struct {
		name  string
		frame string
		limit int
		want  string
	}{
		{name: "malformed", frame: "{nope}\n", limit: 1024, want: "malformed"},
		{name: "wrong version", frame: `{"jsonrpc":"1.0","method":"notice"}` + "\n", limit: 1024, want: "jsonrpc must be omitted or 2.0"},
		{name: "oversized", frame: strings.Repeat("x", 65) + "\n", limit: 64, want: "exceeds 64 bytes"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			client := newClient(context.Background(), strings.NewReader(test.frame), io.Discard, nil, nil, test.limit)
			select {
			case <-client.Done():
			case <-time.After(time.Second):
				t.Fatal("transport did not stop")
			}
			if err := client.Err(); err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v", err)
			}
		})
	}
}

func TestUnknownNotificationDoesNotStopTransport(t *testing.T) {
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	client := NewClient(context.Background(), clientInput, clientOutput, nil, nil)
	go func() {
		defer serverOutput.Close()
		reader := bufio.NewReader(serverInput)
		_, _ = reader.ReadBytes('\n')
		_, _ = serverOutput.Write([]byte(`{"method":"future/additiveNotification","params":{"value":true}}` + "\n"))
		_, _ = serverOutput.Write([]byte(`{"id":1,"result":{"thread":{"id":"thread-after-notification"}}}` + "\n"))
	}()
	var response ThreadResponse
	if err := client.Call(context.Background(), "thread/start", ThreadStartParams{}, &response); err != nil {
		t.Fatal(err)
	}
	if response.Thread.ID != "thread-after-notification" {
		t.Fatalf("response = %#v", response)
	}
}

func TestCanceledCallDoesNotStopTransport(t *testing.T) {
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	client := NewClient(context.Background(), clientInput, clientOutput, nil, nil)
	firstReceived := make(chan struct{})
	releaseFirst := make(chan struct{})
	go func() {
		defer serverOutput.Close()
		reader := bufio.NewReader(serverInput)
		_, _ = reader.ReadBytes('\n')
		close(firstReceived)
		<-releaseFirst
		_, _ = serverOutput.Write([]byte(`{"id":1,"result":{"thread":{"id":"late-thread"}}}` + "\n"))
		_, _ = reader.ReadBytes('\n')
		_, _ = serverOutput.Write([]byte(`{"id":2,"result":{"thread":{"id":"live-thread"}}}` + "\n"))
	}()
	callContext, cancel := context.WithCancel(context.Background())
	callDone := make(chan error, 1)
	go func() {
		callDone <- client.Call(callContext, "thread/read", ThreadReadParams{ThreadID: "thread-1"}, nil)
	}()
	<-firstReceived
	cancel()
	if err := <-callDone; !errors.Is(err, context.Canceled) {
		t.Fatalf("canceled call error = %v", err)
	}
	close(releaseFirst)
	var response ThreadResponse
	if err := client.Call(context.Background(), "thread/read", ThreadReadParams{ThreadID: "thread-1"}, &response); err != nil {
		t.Fatal(err)
	}
	if response.Thread.ID != "live-thread" {
		t.Fatalf("response = %#v", response)
	}
}

type blockingRequestHandler struct {
	started chan struct{}
	release chan struct{}
}

func (h blockingRequestHandler) HandleRequest(context.Context, ServerRequest) (any, *RPCError, bool) {
	close(h.started)
	<-h.release
	return map[string]bool{"accepted": true}, nil, true
}

func TestServerRequestDoesNotBlockResponseReader(t *testing.T) {
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	handler := blockingRequestHandler{started: make(chan struct{}), release: make(chan struct{})}
	client := NewClient(context.Background(), clientInput, clientOutput, handler, nil)
	go func() {
		reader := bufio.NewReader(serverInput)
		_, _ = reader.ReadBytes('\n')
		_, _ = serverOutput.Write([]byte(`{"jsonrpc":"2.0","id":"approval-1","method":"approval/request","params":{}}` + "\n"))
		<-handler.started
		_, _ = serverOutput.Write([]byte(`{"jsonrpc":"2.0","id":1,"result":{"thread":{"id":"thread-1"}}}` + "\n"))
		_, _ = reader.ReadBytes('\n')
	}()
	var response ThreadResponse
	if err := client.Call(context.Background(), "thread/start", ThreadStartParams{}, &response); err != nil {
		t.Fatal(err)
	}
	if response.Thread.ID != "thread-1" {
		t.Fatalf("response = %#v", response)
	}
	close(handler.release)
}

func TestUnknownServerRequestGetsErrorAndStopsTransport(t *testing.T) {
	var output strings.Builder
	client := NewClient(context.Background(), strings.NewReader(`{"jsonrpc":"2.0","id":"future-1","method":"future/request","params":{}}`+"\n"), &output, nil, nil)
	<-client.Done()
	var compatibility *CompatibilityError
	if !errors.As(client.Err(), &compatibility) || compatibility.Method != "future/request" {
		t.Fatalf("error = %v", client.Err())
	}
	var response struct {
		ID    string    `json:"id"`
		Error *RPCError `json:"error"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(output.String())), &response); err != nil {
		t.Fatal(err)
	}
	if response.ID != "future-1" || response.Error == nil || response.Error.Code != -32601 {
		t.Fatalf("response = %#v", response)
	}
}

func TestCallPreservesRPCErrorDetails(t *testing.T) {
	serverInput, clientOutput := io.Pipe()
	clientInput, serverOutput := io.Pipe()
	client := NewClient(context.Background(), clientInput, clientOutput, nil, nil)
	go func() {
		_, _ = bufio.NewReader(serverInput).ReadBytes('\n')
		_, _ = serverOutput.Write([]byte(`{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"bad cwd","data":{"field":"cwd"}}}` + "\n"))
	}()
	err := client.Call(context.Background(), "thread/start", struct{}{}, nil)
	var rpcErr *RPCError
	if !errors.As(err, &rpcErr) || rpcErr.Code != -32602 || string(rpcErr.Data) != `{"field":"cwd"}` {
		t.Fatalf("error = %v", err)
	}
}
