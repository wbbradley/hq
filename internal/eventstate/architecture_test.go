package eventstate

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestFunctionalCoreHasNoEffectfulImports(t *testing.T) {
	for _, directory := range []string{".", "../reduction"} {
		entries, err := os.ReadDir(directory)
		if err != nil {
			t.Fatal(err)
		}
		for _, entry := range entries {
			if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") || strings.HasSuffix(entry.Name(), "_test.go") {
				continue
			}
			path := filepath.Join(directory, entry.Name())
			file, err := parser.ParseFile(token.NewFileSet(), path, nil, parser.ImportsOnly)
			if err != nil {
				t.Fatal(err)
			}
			for _, imported := range file.Imports {
				path := strings.Trim(imported.Path.Value, `"`)
				for _, forbidden := range []string{"database/sql", "/internal/store", "/internal/domainrpc", "/internal/tui", "/internal/localwire", "/internal/nostrwire"} {
					if path == forbidden || strings.Contains(path, forbidden) {
						t.Errorf("%s imports effectful package %s", path, imported.Path.Value)
					}
				}
			}
		}
	}
}
