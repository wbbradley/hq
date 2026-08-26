package store

import (
	"context"
	"fmt"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/wbbradley/hq/internal/event"
	"github.com/wbbradley/hq/internal/identity"
	"github.com/wbbradley/hq/internal/model"
)

func TestIndependentAppendWorkIsBoundedByAffectedClosure(t *testing.T) {
	type work struct {
		impacted int
		affected int
		updated  int
	}
	var results []work
	for _, historySize := range []int{16, 256} {
		t.Run(fmt.Sprintf("history-%d", historySize), func(t *testing.T) {
			database, agent := independentMessageHistory(t, historySize)
			var generation int
			if err := database.db.QueryRow(`SELECT generation FROM projection_metadata WHERE id=1`).Scan(&generation); err != nil {
				t.Fatal(err)
			}
			probe := message(conformanceMessageID(historySize+1), model.HumanMailboxID, agent.ID, "probe")
			if err := database.Create(context.Background(), probe); err != nil {
				t.Fatal(err)
			}
			var got work
			if err := database.db.QueryRow(`SELECT count(*) FROM impacted_canonical_events`).Scan(&got.impacted); err != nil {
				t.Fatal(err)
			}
			if err := database.db.QueryRow(`SELECT count(*) FROM affected_canonical_events`).Scan(&got.affected); err != nil {
				t.Fatal(err)
			}
			if err := database.db.QueryRow(`SELECT count(*) FROM event_reduction WHERE generation>?`, generation).Scan(&got.updated); err != nil {
				t.Fatal(err)
			}
			if got.impacted > 1 || got.affected > 4 || got.updated > 4 {
				t.Fatalf("independent append work = %#v; want one impacted fact and at most its bounded support closure", got)
			}
			results = append(results, got)
		})
	}
	if len(results) == 2 && results[0] != results[1] {
		t.Fatalf("independent append work changed with total history: small=%#v large=%#v", results[0], results[1])
	}
}

func TestCanonicalTypeStatusQueriesUseBoundedIndex(t *testing.T) {
	database := openStore(t, filepath.Join(t.TempDir(), "hq.db"))
	rows, err := database.db.Query(`EXPLAIN QUERY PLAN SELECT raw FROM canonical_events WHERE event_type IN (?,?) AND reduction_status=? ORDER BY created_at,event_id`, event.TypeProjectCommand, event.TypeProjectResult, event.StatusProjected)
	if err != nil {
		t.Fatal(err)
	}
	defer rows.Close()
	var details []string
	for rows.Next() {
		var id, parent, unused int
		var detail string
		if err := rows.Scan(&id, &parent, &unused, &detail); err != nil {
			t.Fatal(err)
		}
		details = append(details, detail)
	}
	plan := strings.Join(details, "\n")
	if !strings.Contains(plan, "canonical_events_by_type_status_time") || strings.Contains(plan, "SCAN canonical_events") {
		t.Fatalf("canonical type/status lookup is not history-bounded:\n%s", plan)
	}
}

func BenchmarkIndependentCanonicalAppend(b *testing.B) {
	for _, historySize := range []int{32, 512} {
		b.Run(fmt.Sprintf("history-%d", historySize), func(b *testing.B) {
			database, agent := independentMessageHistory(b, historySize)
			ctx := context.Background()
			b.ReportAllocs()
			b.ResetTimer()
			for index := range b.N {
				item := message(conformanceMessageID(historySize+index+1), model.HumanMailboxID, agent.ID, "benchmark")
				item.CreatedAt = time.Unix(1_900_000_000+int64(index), 0).UTC()
				if err := database.Create(ctx, item); err != nil {
					b.Fatal(err)
				}
			}
			b.StopTimer()
			var affected int
			if err := database.db.QueryRow(`SELECT count(*) FROM affected_canonical_events`).Scan(&affected); err != nil {
				b.Fatal(err)
			}
			b.ReportMetric(float64(affected), "affected/op")
		})
	}
}

type testOrBenchmark interface {
	Helper()
	Fatal(...any)
	Cleanup(func())
	TempDir() string
}

func independentMessageHistory(tb testOrBenchmark, historySize int) (*SQLite, model.Mailbox) {
	tb.Helper()
	path := filepath.Join(tb.TempDir(), "hq.db")
	keyPath, err := identity.KeyPath(path)
	if err != nil {
		tb.Fatal(err)
	}
	if _, err := identity.Initialize(keyPath, nil); err != nil {
		tb.Fatal(err)
	}
	database, err := Open(path)
	if err != nil {
		tb.Fatal(err)
	}
	tb.Cleanup(func() { _ = database.Close() })
	agent, err := database.ResolveMailbox(context.Background(), model.SessionIdentity{Harness: "codex", ExternalSessionID: "bounded-work"}, model.RepositoryContext{Directory: "/repo"})
	if err != nil {
		tb.Fatal(err)
	}
	for index := 1; index <= historySize; index++ {
		item := message(conformanceMessageID(index), model.HumanMailboxID, agent.ID, "history")
		item.CreatedAt = time.Unix(1_800_000_000+int64(index), 0).UTC()
		if err := database.Create(context.Background(), item); err != nil {
			tb.Fatal(err)
		}
	}
	return database, agent
}

func conformanceMessageID(index int) string {
	return fmt.Sprintf("019d0000-0000-7000-8000-%012x", index)
}
