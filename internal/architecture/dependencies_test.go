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

func TestHarnessGenericLayersHaveNoCodexAdapterDependency(t *testing.T) {
	repository := repositoryRoot(t)
	for _, directory := range []string{"harness", "harnessbridge", "harnesssupervisor", "domain", "store", "domainrpc", "hqclient", "cli", "tui"} {
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
				if strings.Contains(strings.ToLower(importPath), "/codex") {
					t.Errorf("%s imports Codex adapter or protocol dependency %q", relativePath(repository, path), importPath)
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

func TestMessagePayloadWritersDoNotEmbedStructuralDetails(t *testing.T) {
	repository := repositoryRoot(t)
	files, err := productionGoFiles(filepath.Join(repository, "internal"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		file := parseFile(t, path)
		ast.Inspect(file, func(node ast.Node) bool {
			literal, ok := node.(*ast.CompositeLit)
			if !ok || !isTextPayloadType(literal.Type) {
				return true
			}
			for _, element := range literal.Elts {
				field, ok := element.(*ast.KeyValueExpr)
				if !ok {
					continue
				}
				name, nameOK := field.Key.(*ast.Ident)
				value, valueOK := field.Value.(*ast.BasicLit)
				if !nameOK || name.Name != "Details" || !valueOK || value.Kind != token.STRING {
					continue
				}
				text, err := strconv.Unquote(value.Value)
				if err != nil {
					t.Fatal(err)
				}
				for _, prefix := range []string{"Kind:", "Phase:", "Harness provider:", "Harness session:", "Harness operation:", "Harness item:", "Harness request:", "Project assignment:", "Project thread:"} {
					if strings.Contains(text, prefix) {
						t.Errorf("%s embeds structural prefix %q in a message payload Details literal", relativePath(repository, path), prefix)
					}
				}
			}
			return true
		})
	}
}

func TestTUIPresentationDoesNotRecognizeStructuralDetailsPrefixes(t *testing.T) {
	repository := repositoryRoot(t)
	files, err := productionGoFiles(filepath.Join(repository, "internal", "tui"))
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range files {
		file := parseFile(t, path)
		ast.Inspect(file, func(node ast.Node) bool {
			call, ok := node.(*ast.CallExpr)
			if !ok || len(call.Args) < 2 {
				return true
			}
			selector, ok := call.Fun.(*ast.SelectorExpr)
			if !ok {
				return true
			}
			packageName, packageOK := selector.X.(*ast.Ident)
			prefix, prefixOK := call.Args[1].(*ast.BasicLit)
			if !packageOK || packageName.Name != "strings" || (selector.Sel.Name != "HasPrefix" && selector.Sel.Name != "CutPrefix" && selector.Sel.Name != "TrimPrefix") || !prefixOK || prefix.Kind != token.STRING {
				return true
			}
			value, err := strconv.Unquote(prefix.Value)
			if err != nil {
				t.Fatal(err)
			}
			for _, structural := range []string{"Kind:", "Phase:", "Harness provider:", "Harness session:", "Harness operation:", "Harness item:", "Harness request:", "HQ message:", "HQ mailbox:"} {
				if value == structural {
					t.Errorf("%s recognizes historical structural-details prefix %q", relativePath(repository, path), structural)
				}
			}
			return true
		})
	}
}

func isTextPayloadType(expression ast.Expr) bool {
	switch value := expression.(type) {
	case *ast.Ident:
		return value.Name == "TextPayload"
	case *ast.SelectorExpr:
		return value.Sel.Name == "TextPayload"
	default:
		return false
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
