package architecture_test

import (
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
)

const concreteStoreImport = "github.com/wbbradley/hq/internal/store"

func TestClientProductionCodeHasNoConcreteStoragePath(t *testing.T) {
	repository := repositoryRoot(t)
	for _, relativeDirectory := range []string{"internal/cli", "internal/tui", "internal/codexbridge"} {
		files, err := filepath.Glob(filepath.Join(repository, relativeDirectory, "*.go"))
		if err != nil {
			t.Fatal(err)
		}
		for _, path := range files {
			if strings.HasSuffix(path, "_test.go") {
				continue
			}
			file := parseFile(t, path)
			for _, spec := range file.Imports {
				importPath, err := strconv.Unquote(spec.Path.Value)
				if err != nil {
					t.Fatal(err)
				}
				if importPath == concreteStoreImport || strings.Contains(importPath, "sqlite") {
					t.Errorf("%s imports concrete storage %q", relativePath(repository, path), importPath)
				}
			}
		}
	}
}

func TestOnlyNodeRuntimeOpensConcreteStore(t *testing.T) {
	repository := repositoryRoot(t)
	files, err := productionGoFiles(filepath.Join(repository, "internal"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		if strings.HasSuffix(path, "_test.go") {
			continue
		}
		file := parseFile(t, path)
		storeNames := map[string]bool{}
		for _, spec := range file.Imports {
			importPath, err := strconv.Unquote(spec.Path.Value)
			if err != nil {
				t.Fatal(err)
			}
			if importPath == concreteStoreImport {
				name := "store"
				if spec.Name != nil {
					name = spec.Name.Name
				}
				storeNames[name] = true
			}
		}
		ast.Inspect(file, func(node ast.Node) bool {
			selector, ok := node.(*ast.SelectorExpr)
			if !ok || selector.Sel.Name != "Open" {
				return true
			}
			identifier, ok := selector.X.(*ast.Ident)
			if !ok || !storeNames[identifier.Name] {
				return true
			}
			if relativePath(repository, path) != "internal/node/node.go" {
				t.Errorf("%s calls concrete store.Open outside the node runtime", relativePath(repository, path))
			}
			return true
		})
	}
}

func TestHarnessNeutralPackagesHaveNoCodexDependency(t *testing.T) {
	repository := repositoryRoot(t)
	for _, directory := range []string{"harness", "harnessbridge", "harnesssupervisor"} {
		files, err := productionGoFiles(filepath.Join(repository, "internal", directory))
		if err != nil {
			t.Fatal(err)
		}
		for _, path := range files {
			file := parseFile(t, path)
			for _, spec := range file.Imports {
				importPath, err := strconv.Unquote(spec.Path.Value)
				if err != nil {
					t.Fatal(err)
				}
				if strings.Contains(strings.ToLower(importPath), "codex") {
					t.Errorf("%s imports Codex dependency %q", relativePath(repository, path), importPath)
				}
			}
		}
	}
}

func TestHarnessBridgeContainsNoVendorProtocolNames(t *testing.T) {
	repository := repositoryRoot(t)
	files, err := productionGoFiles(filepath.Join(repository, "internal", "harnessbridge"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		raw, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		for _, forbidden := range []string{"thread/start", "thread/resume", "thread/read", "turn/start", "turn/steer", "clientUserMessageId", "ServerRequest", "RPCError"} {
			if strings.Contains(string(raw), forbidden) {
				t.Errorf("%s contains vendor protocol name %q", relativePath(repository, path), forbidden)
			}
		}
	}
}

func productionGoFiles(root string) ([]string, error) {
	var files []string
	err := filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !entry.IsDir() && strings.HasSuffix(path, ".go") && !strings.HasSuffix(path, "_test.go") {
			files = append(files, path)
		}
		return nil
	})
	return files, err
}

func repositoryRoot(t *testing.T) string {
	t.Helper()
	_, filename, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("resolve architecture test path")
	}
	return filepath.Clean(filepath.Join(filepath.Dir(filename), "..", ".."))
}

func parseFile(t *testing.T, path string) *ast.File {
	t.Helper()
	file, err := parser.ParseFile(token.NewFileSet(), path, nil, 0)
	if err != nil {
		t.Fatal(err)
	}
	return file
}

func relativePath(repository, path string) string {
	relative, err := filepath.Rel(repository, path)
	if err != nil {
		return path
	}
	return filepath.ToSlash(relative)
}
