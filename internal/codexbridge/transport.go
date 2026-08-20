package codexbridge

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sync"
)

const defaultMaximumFrameBytes = 4 << 20

type CompatibilityError struct {
	Method string
}

func (e *CompatibilityError) Error() string {
	return fmt.Sprintf("Codex app-server sent unsupported request %q; update hq or use the tested Codex %s protocol", e.Method, TestedCodexVersion)
}

type RequestHandler interface {
	HandleRequest(context.Context, ServerRequest) (result any, rpcErr *RPCError, handled bool)
}

type NotificationHandler interface {
	HandleNotification(context.Context, Notification)
}

type callResult struct {
	result json.RawMessage
	err    error
}

type Client struct {
	ctx           context.Context
	reader        *bufio.Reader
	writer        io.Writer
	maximumFrame  int
	requests      RequestHandler
	notifications NotificationHandler

	writeMu        sync.Mutex
	stateMu        sync.Mutex
	nextID         int64
	pending        map[int64]chan callResult
	done           chan struct{}
	doneErr        error
	stop           sync.Once
	requestMu      sync.Mutex
	acceptRequests bool
	requestWG      sync.WaitGroup
}

func NewClient(ctx context.Context, input io.Reader, output io.Writer, requests RequestHandler, notifications NotificationHandler) *Client {
	return newClient(ctx, input, output, requests, notifications, defaultMaximumFrameBytes)
}

func newClient(ctx context.Context, input io.Reader, output io.Writer, requests RequestHandler, notifications NotificationHandler, maximumFrame int) *Client {
	client := &Client{
		ctx: ctx, reader: bufio.NewReader(input), writer: output, maximumFrame: maximumFrame,
		requests: requests, notifications: notifications, pending: make(map[int64]chan callResult), done: make(chan struct{}), acceptRequests: true,
	}
	go client.readLoop()
	return client
}

func (c *Client) Call(ctx context.Context, method string, params, destination any) error {
	c.stateMu.Lock()
	if c.doneErr != nil {
		err := c.doneErr
		c.stateMu.Unlock()
		return err
	}
	c.nextID++
	id := c.nextID
	response := make(chan callResult, 1)
	c.pending[id] = response
	c.stateMu.Unlock()

	message := struct {
		ID     int64  `json:"id"`
		Method string `json:"method"`
		Params any    `json:"params"`
	}{ID: id, Method: method, Params: params}
	if err := c.write(message); err != nil {
		c.removePending(id)
		c.finish(fmt.Errorf("write %s request: %w", method, err))
		return err
	}
	select {
	case reply := <-response:
		if reply.err != nil {
			return fmt.Errorf("%s: %w", method, reply.err)
		}
		if destination == nil || len(reply.result) == 0 || bytes.Equal(reply.result, []byte("null")) {
			return nil
		}
		if err := json.Unmarshal(reply.result, destination); err != nil {
			return fmt.Errorf("decode %s response: %w", method, err)
		}
		return nil
	case <-ctx.Done():
		c.removePending(id)
		return ctx.Err()
	case <-c.done:
		c.removePending(id)
		return c.Err()
	}
}

func (c *Client) Notify(method string, params any) error {
	message := struct {
		Method string `json:"method"`
		Params any    `json:"params"`
	}{Method: method, Params: params}
	if err := c.write(message); err != nil {
		wrapped := fmt.Errorf("write %s notification: %w", method, err)
		c.finish(wrapped)
		return wrapped
	}
	return nil
}

func (c *Client) Done() <-chan struct{} { return c.done }

func (c *Client) Err() error {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	return c.doneErr
}

func (c *Client) readLoop() {
	for {
		frame, err := c.readFrame()
		if err != nil {
			c.finish(err)
			return
		}
		var envelope rpcEnvelope
		if err := json.Unmarshal(frame, &envelope); err != nil {
			c.finish(fmt.Errorf("malformed app-server JSON-RPC frame: %w", err))
			return
		}
		if envelope.JSONRPC != "" && envelope.JSONRPC != "2.0" {
			c.finish(errors.New("invalid app-server JSON-RPC envelope: jsonrpc must be omitted or 2.0"))
			return
		}
		if envelope.Method != "" && len(envelope.ID) != 0 {
			if c.requests == nil {
				c.rejectUnsupportedRequest(envelope)
				return
			}
			c.requestMu.Lock()
			if !c.acceptRequests {
				c.requestMu.Unlock()
				_ = c.writeResponse(envelope.ID, nil, &RPCError{Code: -32800, Message: "client is shutting down"})
				continue
			}
			c.requestWG.Add(1)
			c.requestMu.Unlock()
			go func() {
				defer c.requestWG.Done()
				c.handleServerRequest(envelope)
			}()
			continue
		}
		if envelope.Method != "" {
			if c.notifications != nil {
				c.notifications.HandleNotification(c.ctx, Notification{Method: envelope.Method, Params: envelope.Params})
			}
			continue
		}
		if len(envelope.ID) == 0 {
			c.finish(errors.New("invalid app-server JSON-RPC envelope: missing method and id"))
			return
		}
		var id int64
		if err := json.Unmarshal(envelope.ID, &id); err != nil {
			c.finish(fmt.Errorf("invalid app-server response id: %w", err))
			return
		}
		c.stateMu.Lock()
		response := c.pending[id]
		delete(c.pending, id)
		c.stateMu.Unlock()
		if response == nil {
			continue
		}
		if envelope.Error != nil {
			response <- callResult{err: envelope.Error}
		} else {
			response <- callResult{result: envelope.Result}
		}
	}
}

func (c *Client) StopRequestsAndWait() {
	c.requestMu.Lock()
	c.acceptRequests = false
	c.requestMu.Unlock()
	c.requestWG.Wait()
}

func (c *Client) handleServerRequest(envelope rpcEnvelope) {
	request := ServerRequest{ID: envelope.ID, Method: envelope.Method, Params: envelope.Params}
	result, rpcErr, handled := c.requests.HandleRequest(c.ctx, request)
	if !handled {
		c.rejectUnsupportedRequest(envelope)
		return
	}
	if err := c.writeResponse(envelope.ID, result, rpcErr); err != nil {
		c.finish(fmt.Errorf("write %s response: %w", envelope.Method, err))
	}
}

func (c *Client) rejectUnsupportedRequest(envelope rpcEnvelope) {
	rpcErr := &RPCError{Code: -32601, Message: "unsupported app-server request: " + envelope.Method}
	_ = c.writeResponse(envelope.ID, nil, rpcErr)
	c.finish(&CompatibilityError{Method: envelope.Method})
}

func (c *Client) writeResponse(id json.RawMessage, result any, rpcErr *RPCError) error {
	message := struct {
		ID     json.RawMessage `json:"id"`
		Result any             `json:"result,omitempty"`
		Error  *RPCError       `json:"error,omitempty"`
	}{ID: id, Result: result, Error: rpcErr}
	return c.write(message)
}

func (c *Client) write(message any) error {
	raw, err := json.Marshal(message)
	if err != nil {
		return err
	}
	raw = append(raw, '\n')
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	_, err = c.writer.Write(raw)
	return err
}

func (c *Client) readFrame() ([]byte, error) {
	var frame []byte
	for {
		part, err := c.reader.ReadSlice('\n')
		frame = append(frame, part...)
		if len(frame) > c.maximumFrame {
			return nil, fmt.Errorf("app-server JSON-RPC frame exceeds %d bytes", c.maximumFrame)
		}
		switch {
		case err == nil:
			return bytes.TrimSuffix(frame, []byte{'\n'}), nil
		case errors.Is(err, bufio.ErrBufferFull):
			continue
		case errors.Is(err, io.EOF) && len(frame) > 0:
			return frame, nil
		case errors.Is(err, io.EOF):
			return nil, errors.New("Codex app-server closed its protocol stream")
		default:
			return nil, fmt.Errorf("read app-server JSON-RPC: %w", err)
		}
	}
}

func (c *Client) removePending(id int64) {
	c.stateMu.Lock()
	delete(c.pending, id)
	c.stateMu.Unlock()
}

func (c *Client) finish(err error) {
	c.stop.Do(func() {
		if err == nil {
			err = errors.New("Codex app-server transport stopped")
		}
		c.stateMu.Lock()
		c.doneErr = err
		pending := c.pending
		c.pending = make(map[int64]chan callResult)
		c.stateMu.Unlock()
		for _, response := range pending {
			response <- callResult{err: err}
		}
		close(c.done)
	})
}
