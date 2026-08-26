package store

import (
	"context"
	"database/sql"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"reflect"
	"sort"
	"strings"
	"testing"

	"github.com/wbbradley/hq/internal/model"
)

// reductionConformanceTables is the explicit batch-reducer ownership
// boundary. Operational leases, receipts, relay attempts, runtime workflows,
// health observations, audit logs, and local TUI drafts are intentionally not
// part of the rebuild oracle.
var reductionConformanceTables = []string{
	"canonical_events", "causal_edges", "authority_dependencies", "unresolved_waiters", "event_resources",
	"aggregate_frontiers", "projection_support", "event_reduction",
	"mailboxes", "harness_bindings", "named_agents", "agent_sessions", "mailbox_contexts", "messages", "threads",
	"peers", "mailbox_access", "human_accounts", "human_account_devices", "human_account_default", "outbox",
	"projects", "project_events", "resources", "project_resources", "resource_claim_epochs", "project_assignment_epochs",
	"project_threads", "project_message_acceptances", "project_dispatch_records", "project_replicas", "harness_activities",
}

var reductionConformanceExcludedColumns = map[string]map[string]bool{
	"canonical_events":    {"raw": true},
	"aggregate_frontiers": {"generation": true},
	"projection_support":  {"generation": true},
	"event_reduction":     {"generation": true},
	"named_agents":        {"last_active_at": true},
	"outbox":              {"exact_canonical_bytes": true},
}

type reductionConformanceSnapshot map[string][]string

func captureReductionConformanceSnapshot(t *testing.T, database *SQLite) reductionConformanceSnapshot {
	t.Helper()
	snapshot := make(reductionConformanceSnapshot, len(reductionConformanceTables)+1)
	for _, table := range reductionConformanceTables {
		snapshot["table:"+table] = stableTableRows(t, database.db, table, reductionConformanceExcludedColumns[table])
	}
	captureConversationPages(t, database, snapshot)
	return snapshot
}

func stableTableRows(t *testing.T, database *sql.DB, table string, excluded map[string]bool) []string {
	t.Helper()
	columns, err := database.Query(`PRAGMA table_info(` + table + `)`)
	if err != nil {
		t.Fatalf("inspect %s columns: %v", table, err)
	}
	var names []string
	for columns.Next() {
		var position, notNull, primaryKey int
		var name, columnType string
		var defaultValue any
		if err := columns.Scan(&position, &name, &columnType, &notNull, &defaultValue, &primaryKey); err != nil {
			columns.Close()
			t.Fatalf("scan %s columns: %v", table, err)
		}
		if !excluded[name] {
			names = append(names, name)
		}
	}
	if err := columns.Close(); err != nil {
		t.Fatalf("close %s columns: %v", table, err)
	}
	if len(names) == 0 {
		t.Fatalf("conformance table %s has no comparable columns", table)
	}
	query := `SELECT ` + strings.Join(names, ",") + ` FROM ` + table + ` ORDER BY ` + strings.Join(names, ",")
	rows, err := database.Query(query)
	if err != nil {
		t.Fatalf("snapshot %s: %v", table, err)
	}
	defer rows.Close()
	var result []string
	for rows.Next() {
		values := make([]any, len(names))
		destinations := make([]any, len(names))
		for index := range values {
			destinations[index] = &values[index]
		}
		if err := rows.Scan(destinations...); err != nil {
			t.Fatalf("scan %s snapshot: %v", table, err)
		}
		for index, value := range values {
			if raw, ok := value.([]byte); ok {
				values[index] = "base64:" + base64.StdEncoding.EncodeToString(raw)
			}
		}
		raw, err := json.Marshal(values)
		if err != nil {
			t.Fatalf("encode %s snapshot: %v", table, err)
		}
		result = append(result, string(raw))
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("iterate %s snapshot: %v", table, err)
	}
	return result
}

func captureConversationPages(t *testing.T, database *SQLite, snapshot reductionConformanceSnapshot) {
	t.Helper()
	ctx := context.Background()
	filter := model.ConversationFilter{IncludeSent: true, IncludeArchived: true, Limit: 2}
	var summaries []model.ConversationSummary
	for {
		page, err := database.ListConversations(ctx, filter)
		if err != nil {
			t.Fatalf("list conversation conformance page: %v", err)
		}
		summaries = append(summaries, page.Conversations...)
		if page.NextCursor == "" {
			break
		}
		filter.Cursor = page.NextCursor
	}
	snapshot["api:conversations"] = jsonRows(t, summaries)
	for _, summary := range summaries {
		historyFilter := model.ConversationHistoryFilter{Key: summary.Key, Limit: 2}
		var entries []any
		for {
			page, err := database.ListConversationEntries(ctx, historyFilter)
			if err != nil {
				t.Fatalf("list conversation entry conformance page for %#v: %v", summary.Key, err)
			}
			for _, entry := range page.Entries {
				entries = append(entries, entry)
			}
			if page.NextCursor == "" {
				break
			}
			historyFilter.Cursor = page.NextCursor
		}
		key, err := json.Marshal(summary.Key)
		if err != nil {
			t.Fatalf("encode conversation key: %v", err)
		}
		snapshot["api:entries:"+string(key)] = jsonRows(t, entries)
	}
}

func jsonRows[T any](t *testing.T, values []T) []string {
	t.Helper()
	rows := make([]string, 0, len(values))
	for _, value := range values {
		raw, err := json.Marshal(value)
		if err != nil {
			t.Fatalf("encode conformance API row: %v", err)
		}
		rows = append(rows, string(raw))
	}
	return rows
}

func firstReductionConformanceDifference(incremental, rebuilt reductionConformanceSnapshot) string {
	keys := make([]string, 0, len(incremental)+len(rebuilt))
	seen := make(map[string]bool)
	for key := range incremental {
		seen[key] = true
		keys = append(keys, key)
	}
	for key := range rebuilt {
		if !seen[key] {
			keys = append(keys, key)
		}
	}
	sort.Strings(keys)
	for _, key := range keys {
		left, right := incremental[key], rebuilt[key]
		if reflect.DeepEqual(left, right) {
			continue
		}
		limit := min(len(left), len(right))
		for index := 0; index < limit; index++ {
			if left[index] != right[index] {
				return fmt.Sprintf("%s row %d\nincremental: %s\nrebuilt:     %s", key, index, left[index], right[index])
			}
		}
		return fmt.Sprintf("%s row count incremental=%d rebuilt=%d\nfirst extra incremental=%q\nfirst extra rebuilt=%q", key, len(left), len(right), firstExtra(left, limit), firstExtra(right, limit))
	}
	return ""
}

func firstExtra(rows []string, index int) string {
	if index >= len(rows) {
		return ""
	}
	return rows[index]
}

func assertIncrementalMatchesBatchRebuild(t *testing.T, database *SQLite) {
	t.Helper()
	incremental := captureReductionConformanceSnapshot(t, database)
	if err := database.Rebuild(context.Background()); err != nil {
		t.Fatalf("batch rebuild oracle: %v", err)
	}
	rebuilt := captureReductionConformanceSnapshot(t, database)
	if difference := firstReductionConformanceDifference(incremental, rebuilt); difference != "" {
		t.Fatalf("incremental reduction diverged from batch rebuild:\n%s", difference)
	}
}

func TestReductionConformanceDifferenceIdentifiesFirstRow(t *testing.T) {
	incremental := reductionConformanceSnapshot{"table:messages": {`["message-a"]`, `["message-c"]`}}
	rebuilt := reductionConformanceSnapshot{"table:messages": {`["message-a"]`, `["message-b"]`}}
	difference := firstReductionConformanceDifference(incremental, rebuilt)
	if !strings.Contains(difference, "table:messages row 1") || !strings.Contains(difference, "message-c") || !strings.Contains(difference, "message-b") {
		t.Fatalf("difference diagnostic = %q", difference)
	}
}
