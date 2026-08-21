package localwire

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"sync"
	"time"
)

const DefaultMaximumFrameBytes = 1 << 20

type Codec struct {
	reader       *bufio.Reader
	writer       io.Writer
	maximumFrame int
	writeTimeout time.Duration
	writeMu      sync.Mutex
}

func NewCodec(input io.Reader, output io.Writer, maximumFrame int) *Codec {
	return NewCodecWithTimeout(input, output, maximumFrame, 0)
}

func NewCodecWithTimeout(input io.Reader, output io.Writer, maximumFrame int, writeTimeout time.Duration) *Codec {
	if maximumFrame <= 0 {
		maximumFrame = DefaultMaximumFrameBytes
	}
	return &Codec{reader: bufio.NewReader(input), writer: output, maximumFrame: maximumFrame, writeTimeout: writeTimeout}
}

func (c *Codec) Read() (Envelope, error) {
	frame, err := c.readFrame()
	if err != nil {
		return Envelope{}, err
	}
	decoder := json.NewDecoder(bytes.NewReader(frame))
	decoder.DisallowUnknownFields()
	var envelope Envelope
	if err := decoder.Decode(&envelope); err != nil {
		return Envelope{}, fmt.Errorf("malformed local-wire frame: %w", err)
	}
	if decoder.Decode(&struct{}{}) != io.EOF {
		return Envelope{}, errors.New("malformed local-wire frame: trailing JSON value")
	}
	if err := envelope.Validate(); err != nil {
		return Envelope{}, fmt.Errorf("invalid local-wire envelope: %w", err)
	}
	return envelope, nil
}

func (c *Codec) Write(envelope Envelope) error {
	if err := envelope.Validate(); err != nil {
		return fmt.Errorf("invalid local-wire envelope: %w", err)
	}
	raw, err := json.Marshal(envelope)
	if err != nil {
		return err
	}
	if len(raw)+1 > c.maximumFrame {
		return fmt.Errorf("local-wire frame exceeds %d bytes", c.maximumFrame)
	}
	raw = append(raw, '\n')
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if deadlineWriter, ok := c.writer.(interface{ SetWriteDeadline(time.Time) error }); ok && c.writeTimeout > 0 {
		if err := deadlineWriter.SetWriteDeadline(time.Now().Add(c.writeTimeout)); err != nil {
			return err
		}
		defer deadlineWriter.SetWriteDeadline(time.Time{})
	}
	_, err = c.writer.Write(raw)
	return err
}

func (c *Codec) readFrame() ([]byte, error) {
	var frame []byte
	for {
		part, err := c.reader.ReadSlice('\n')
		frame = append(frame, part...)
		if len(frame) > c.maximumFrame {
			return nil, fmt.Errorf("local-wire frame exceeds %d bytes", c.maximumFrame)
		}
		switch {
		case err == nil:
			frame = bytes.TrimSuffix(frame, []byte{'\n'})
			if len(bytes.TrimSpace(frame)) == 0 {
				return nil, errors.New("empty local-wire frame")
			}
			return frame, nil
		case errors.Is(err, bufio.ErrBufferFull):
			continue
		case errors.Is(err, io.EOF) && len(frame) > 0:
			return nil, errors.New("unterminated local-wire frame")
		case errors.Is(err, io.EOF):
			return nil, io.EOF
		default:
			return nil, fmt.Errorf("read local-wire frame: %w", err)
		}
	}
}
