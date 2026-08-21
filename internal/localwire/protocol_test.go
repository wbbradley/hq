package localwire

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestHandshakeFixturesRemainValid(t *testing.T) {
	tests := []struct {
		name string
		max  int
	}{
		{name: "exact-handshake.json", max: 1},
		{name: "range-handshake.json", max: 3},
	}
	for _, test := range tests {
		raw, err := os.ReadFile(filepath.Join("testdata", test.name))
		if err != nil {
			t.Fatal(err)
		}
		var envelope Envelope
		if err := json.Unmarshal(raw, &envelope); err != nil || envelope.Validate() != nil {
			t.Fatalf("fixture %s is invalid: %v", test.name, err)
		}
		var request HandshakeRequest
		if err := json.Unmarshal(envelope.Params, &request); err != nil || request.Mode != DomainMode || request.Supported.Max != test.max {
			t.Fatalf("fixture %s request = %#v, %v", test.name, request, err)
		}
	}
}

func TestNegotiateSelectsHighestMutualVersion(t *testing.T) {
	tests := []struct {
		name           string
		client, server VersionRange
		want           int
	}{
		{name: "exact", client: VersionRange{Min: 1, Max: 1}, server: VersionRange{Min: 1, Max: 1}, want: 1},
		{name: "range", client: VersionRange{Min: 1, Max: 4}, server: VersionRange{Min: 2, Max: 3}, want: 3},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			got, err := Negotiate(test.client, test.server)
			if err != nil || got != test.want {
				t.Fatalf("Negotiate() = %d, %v; want %d", got, err, test.want)
			}
		})
	}
}

func TestIncompatibilityIdentifiesStaleSideAndAction(t *testing.T) {
	tests := []struct {
		name           string
		client, server VersionRange
		stale, action  string
	}{
		{name: "old client new server", client: VersionRange{Min: 1, Max: 2}, server: VersionRange{Min: 3, Max: 4}, stale: "client", action: "upgrade this HQ client"},
		{name: "new client old server", client: VersionRange{Min: 3, Max: 4}, server: VersionRange{Min: 1, Max: 2}, stale: "server", action: "restart or upgrade the local HQ node"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			_, err := Negotiate(test.client, test.server)
			var incompatible *IncompatibilityError
			if !errors.As(err, &incompatible) {
				t.Fatalf("error = %v", err)
			}
			if incompatible.Data.StaleSide != test.stale || incompatible.Data.Action != test.action {
				t.Fatalf("data = %#v", incompatible.Data)
			}
			for _, text := range []string{"client", "server", test.stale, test.action} {
				if !strings.Contains(incompatible.Error(), text) {
					t.Fatalf("diagnostic %q does not contain %q", incompatible.Error(), text)
				}
			}
		})
	}
}
