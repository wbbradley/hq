package codexbridge

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestFileLedgerPersistsDeliveryAndOutputState(t *testing.T) {
	path := filepath.Join(t.TempDir(), "bridge", "deliveries.json")
	ledger, err := OpenFileLedger(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := ledger.SetDelivery("thread-1", "message-1", DeliveryUncertain); err != nil {
		t.Fatal(err)
	}
	if err := ledger.SetDelivery("thread-1", "message-1", DeliveryAccepted); err != nil {
		t.Fatal(err)
	}
	if err := ledger.MarkOutputSent("thread-1", "item-1"); err != nil {
		t.Fatal(err)
	}

	reopened, err := OpenFileLedger(path)
	if err != nil {
		t.Fatal(err)
	}
	record, found, err := reopened.Delivery("thread-1", "message-1")
	if err != nil || !found || record.State != DeliveryAccepted || record.UpdatedAt.IsZero() {
		t.Fatalf("delivery = %#v, %t, %v", record, found, err)
	}
	sent, err := reopened.OutputSent("thread-1", "item-1")
	if err != nil || !sent {
		t.Fatalf("output sent = %t, %v", sent, err)
	}
	if runtime.GOOS != "windows" {
		info, err := os.Stat(path)
		if err != nil || info.Mode().Perm() != 0o600 {
			t.Fatalf("ledger mode = %v, %v", info.Mode().Perm(), err)
		}
	}
}

func TestFileLedgerRejectsCorruptAndIncompatibleState(t *testing.T) {
	tests := []struct {
		name string
		raw  string
		want string
	}{
		{name: "corrupt", raw: "{nope", want: "decode delivery ledger"},
		{name: "version", raw: `{"version":99,"deliveries":{},"outputs":{}}`, want: "version 99 is unsupported"},
		{name: "state", raw: `{"version":1,"deliveries":{"thread":{"message":{"state":"mystery","updated_at":"2026-08-20T00:00:00Z"}}},"outputs":{}}`, want: "contains invalid record"},
		{name: "output", raw: `{"version":1,"deliveries":{},"outputs":{"thread":{"item":false}}}`, want: "contains invalid output"},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "ledger.json")
			if err := os.WriteFile(path, []byte(test.raw), 0o600); err != nil {
				t.Fatal(err)
			}
			_, err := OpenFileLedger(path)
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v", err)
			}
		})
	}
}

func TestLedgerValidatesStateAndIdentifiers(t *testing.T) {
	ledger, err := OpenFileLedger(filepath.Join(t.TempDir(), "ledger.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := ledger.SetDelivery("thread", "message", DeliveryState("mystery")); err == nil {
		t.Fatal("invalid state was accepted")
	}
	if err := ledger.MarkOutputSent("", "item"); err == nil {
		t.Fatal("empty thread ID was accepted")
	}
}
