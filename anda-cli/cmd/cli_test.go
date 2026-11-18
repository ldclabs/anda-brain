package cmd

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"github.com/ldclabs/cose/key"
	"github.com/ldclabs/cose/key/ed25519"
	"github.com/spf13/cobra"
)

// Each CLI invocation gets a fresh command tree and flag state, as in the real
// executable. This also exercises exit status without os.Exit in command code.
func TestCLIProcess(t *testing.T) {
	if os.Getenv("ANDA_CLI_TEST_HELPER") != "1" {
		return
	}
	for i, arg := range os.Args {
		if arg == "--" {
			rootCmd.SetArgs(os.Args[i+1:])
			break
		}
	}
	if err := Execute(); err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		os.Exit(1)
	}
	os.Exit(0)
}

func runCLI(t *testing.T, env map[string]string, args ...string) (string, string, error) {
	t.Helper()
	command := exec.Command(os.Args[0], append([]string{"-test.run=^TestCLIProcess$", "--"}, args...)...)
	command.Dir = t.TempDir()
	for _, value := range os.Environ() {
		if !strings.HasPrefix(value, "ANDA_") {
			command.Env = append(command.Env, value)
		}
	}
	command.Env = append(command.Env, "ANDA_CLI_TEST_HELPER=1")
	for name, value := range env {
		command.Env = append(command.Env, name+"="+value)
	}
	var out, stderr bytes.Buffer
	command.Stdout, command.Stderr = &out, &stderr
	err := command.Run()
	return out.String(), stderr.String(), err
}

func TestCLISecretsNeverAppearInHelp(t *testing.T) {
	secrets := map[string]string{"ANDA_TOKEN": "TEST_TOKEN_SECRET", "ANDA_CWT_KEY": "TEST_PRIVATE_SECRET", "ANDA_BYOK_API_KEY": "TEST_PROVIDER_SECRET"}
	for _, args := range [][]string{{"--help"}, {"cwt", "--help"}, {"management", "update-byok", "--help"}, {"recall"}} {
		out, stderr, _ := runCLI(t, secrets, args...)
		for _, secret := range secrets {
			if strings.Contains(out+stderr, secret) {
				t.Fatalf("%v exposed a secret", args)
			}
		}
	}
}

func TestCLISecretEnvironmentAndFlagPrecedence(t *testing.T) {
	auth := make(chan string, 3)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		auth <- r.Header.Get("Authorization")
		_, _ = w.Write([]byte(`{"name":"brain","version":"0.13.2","sharding":0}`))
	}))
	defer server.Close()
	for _, tc := range []struct {
		args []string
		want string
	}{
		{nil, "Bearer env-token"}, {[]string{"--token", "flag-token"}, "Bearer flag-token"}, {[]string{"--token", ""}, ""},
	} {
		args := append([]string{"--base-url", server.URL, "status"}, tc.args...)
		_, stderr, err := runCLI(t, map[string]string{"ANDA_TOKEN": "env-token"}, args...)
		if err != nil {
			t.Fatalf("status: %v %s", err, stderr)
		}
		if got := <-auth; got != tc.want {
			t.Fatalf("authorization=%q, want %q", got, tc.want)
		}
	}
	private, err := ed25519.GenerateKey()
	if err != nil {
		t.Fatal(err)
	}
	data, err := key.MarshalCBOR(private)
	if err != nil {
		t.Fatal(err)
	}
	encoded := base64.RawURLEncoding.EncodeToString(data)
	out, stderr, err := runCLI(t, map[string]string{"ANDA_CWT_KEY": encoded}, "cwt", "--subject", "2vxsx-fae", "--audience", "s1", "--json")
	if err != nil || !strings.Contains(out, `"token"`) {
		t.Fatalf("environment key not used: %v %s", err, stderr)
	}
}

func TestCLIBYOKSecretEnvironment(t *testing.T) {
	bodies := make(chan string, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		bodies <- string(data)
		_, _ = io.WriteString(w, `{"result":true}`)
	}))
	defer server.Close()
	_, stderr, err := runCLI(t, map[string]string{"ANDA_BYOK_API_KEY": "test-provider-key"},
		"--base-url", server.URL, "--space-id", "s1", "management", "update-byok",
		"--family", "openai", "--model", "test", "--api-base", "https://example.invalid/v1")
	if err != nil {
		t.Fatalf("BYOK: %v %s", err, stderr)
	}
	if body := <-bodies; !strings.Contains(body, `"api_key":"test-provider-key"`) {
		t.Fatalf("environment key not sent: %s", body)
	}
}

func TestCLIWikiInputRejectsFlagOverrides(t *testing.T) {
	for _, flag := range []string{"file", "title", "namespace", "slug", "tags", "acl-label", "source-uri", "message", "doc-id", "parent-version", "clear-tags"} {
		value := "1"
		if flag == "clear-tags" {
			value = "false"
		}
		_, stderr, err := runCLI(t, nil, "--space-id", "s1", "wiki", "commit", "--input", `{"title":"T","content":"# T"}`, "--"+flag+"="+value)
		if err == nil || !strings.Contains(stderr, "mutually exclusive") {
			t.Fatalf("%s silently accepted: %v %s", flag, err, stderr)
		}
	}
}

func TestCLIBusinessFailuresPreserveJSON(t *testing.T) {
	cases := []struct {
		args           []string
		payload, field string
		failed         bool
	}{
		{[]string{"recall", "query"}, `{"content":"partial","failed_reason":"provider failed"}`, `"failed_reason"`, true},
		{[]string{"recall", "--structured", "query"}, `{"answer":"","found":false,"failed_reason":"provider failed","usage":{}}`, `"failed_reason"`, true},
		{[]string{"recall", "--structured", "query"}, `{"answer":"no match","found":false,"usage":{}}`, `"found": false`, false},
		{[]string{"memory", "forget", "C-7"}, `{"dry_run":false,"entities":[{"entity":"C-7","existed":true,"error":"protected"}]}`, `"protected"`, true},
		{[]string{"wiki", "digest"}, `{"digested":1,"failed":2,"usage":{}}`, `"failed": 2`, true},
		{[]string{"recall", "--structured", "query"}, `{"answer":"ok","found":true,"usage":{},"recall_receipt":{"id":"r1","digest":"d1","scope":{"space_id":"s1","space_instance":"i1"}}}`, `"recall_receipt"`, false},
	}
	for _, tc := range cases {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { fmt.Fprintf(w, `{"result":%s}`, tc.payload) }))
		out, stderr, err := runCLI(t, nil, append([]string{"--base-url", server.URL, "--space-id", "s1"}, tc.args...)...)
		server.Close()
		if (err != nil) != tc.failed || !json.Valid([]byte(out)) || !strings.Contains(out, tc.field) {
			t.Fatalf("%v: %v out=%s stderr=%s", tc.args, err, out, stderr)
		}
	}
}

func TestCLIWikiExportImportsWithoutEditing(t *testing.T) {
	requests := make(chan string, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodGet {
			_, _ = w.Write([]byte(`{"result":{"namespace":"docs","entries":[{"path":"a.md","content":"# A"}],"docs":1}}`))
			return
		}
		data, _ := io.ReadAll(r.Body)
		requests <- string(data)
		_, _ = w.Write([]byte(`{"result":{"created":1,"docs":[]}}`))
	}))
	defer server.Close()
	prefix := []string{"--base-url", server.URL, "--space-id", "s1", "wiki"}
	out, stderr, err := runCLI(t, nil, append(prefix, "export")...)
	if err != nil {
		t.Fatalf("export: %v %s", err, stderr)
	}
	path := filepath.Join(t.TempDir(), "bundle.json")
	mustWriteFile(t, path, out)
	_, stderr, err = runCLI(t, nil, append(prefix, "import", "--input", "@"+path)...)
	if err != nil {
		t.Fatalf("import: %v %s", err, stderr)
	}
	if got := <-requests; strings.Contains(got, `"docs":`) || !strings.Contains(got, `"namespace":"docs"`) {
		t.Fatalf("bad import: %s", got)
	}
	if _, err := readWikiImport(`{"entries":[],"docz":1}`); err == nil {
		t.Fatal("unknown import field accepted")
	}
}

func TestCLIFormationAndKIPPreservePayloads(t *testing.T) {
	bodies := make(chan string, 4)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		bodies <- string(data)
		if strings.HasSuffix(r.URL.Path, "execute_kip_readonly") {
			_, _ = w.Write([]byte(`{"kip":"2.0","status":"succeeded","results":[{"status":"succeeded","result":{}}]}`))
			return
		}
		_, _ = w.Write([]byte(`{"result":{"content":"","conversation":7}}`))
	}))
	defer server.Close()
	prefix := []string{"--base-url", server.URL, "--space-id", "s1"}
	for _, request := range []string{
		`{"command":"DESCRIBE PRIMER","parameters":{"id":9007199254740993}}`,
		`{"operations":[{"command":"DESCRIBE PRIMER","parameters":{"id":9007199254740993}}]}`,
	} {
		_, stderr, err := runCLI(t, nil, append(prefix, "execute-kip-readonly", "--request", request)...)
		if err != nil {
			t.Fatalf("KIP: %v %s", err, stderr)
		}
		if body := <-bodies; !strings.Contains(body, "9007199254740993") {
			t.Fatalf("rounded parameter: %s", body)
		}
	}
	// A bare command is sent as a command request, as the server reads it.
	if _, stderr, err := runCLI(t, nil, append(prefix, "execute-kip-readonly", "--request", "DESCRIBE PRIMER")...); err != nil {
		t.Fatalf("bare KIP command: %v %s", err, stderr)
	}
	if body := <-bodies; body != `{"command":"DESCRIBE PRIMER"}` {
		t.Fatalf("bare KIP command sent as %s", body)
	}
	_, stderr, err := runCLI(t, nil, append(prefix, "formation", "--messages", `[{"role":"tool","content":[{"type":"ToolOutput","name":"lookup","output":{"id":9007199254740993},"isError":true}]}]`)...)
	if err != nil {
		t.Fatalf("formation: %v %s", err, stderr)
	}
	if body := <-bodies; !strings.Contains(body, "9007199254740993") || !strings.Contains(body, `"isError":true`) {
		t.Fatalf("tool result changed: %s", body)
	}
	path := filepath.Join(t.TempDir(), "chinese.txt")
	mustWriteFile(t, path, strings.Repeat("中", 100001))
	_, stderr, err = runCLI(t, nil, append(prefix, "formation", "--file", path)...)
	if err != nil {
		t.Fatalf("client rejected server-sized input: %v %s", err, stderr)
	}
	if body := <-bodies; strings.Count(body, "中") != 100001 {
		t.Fatal("large text changed")
	}
}

func TestCLIRejectsInvalidArguments(t *testing.T) {
	for _, tc := range []struct {
		args    []string
		message string
	}{
		{[]string{"status", "extra"}, "unknown command"},
		{[]string{"cwt", "--scope", "invalid"}, "invalid --scope"},
		{[]string{"conversations", "list", "--limit=-1"}, "--limit must be non-negative"},
		{[]string{"wiki", "list", "--limit=-1"}, "--limit must be non-negative"},
		{[]string{"formation", "--batch-ext", "md", "--messages", "hello"}, "requires --batch-dir"},
		{[]string{"status", "--shard=-1"}, "--shard must be non-negative"},
		{[]string{"status", "--timeout=0"}, "--timeout must be positive"},
	} {
		_, stderr, err := runCLI(t, nil, append([]string{"--space-id", "s1"}, tc.args...)...)
		if err == nil || !strings.Contains(stderr, tc.message) {
			t.Fatalf("%v: %v %s", tc.args, err, stderr)
		}
	}
}

func TestCLIHelpAndCompletionNeedNoSpace(t *testing.T) {
	for _, args := range [][]string{{"help", "recall"}, {"__complete", "wiki", ""}, {"completion", "zsh"}, {"keygen"}} {
		out, stderr, err := runCLI(t, nil, args...)
		if err != nil || strings.Contains(stderr, "--space-id") || out == "" {
			t.Fatalf("%v: %v %s", args, err, stderr)
		}
	}
	if out, _, _ := runCLI(t, nil, "__complete", "wiki", ""); !strings.Contains(out, "commit") {
		t.Fatalf("completion lost subcommands: %s", out)
	}
	if _, stderr, err := runCLI(t, nil, "info"); err == nil || !strings.Contains(stderr, "--space-id") {
		t.Fatalf("a Space command ran without a Space: %v %s", err, stderr)
	}
}

func TestCLIOutputKeepsUnmodeledServerFields(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/info" {
			_, _ = io.WriteString(w, `{"name":"brain","version":"0.13.2","sharding":0,"memory_interface":{"kip_memory":"2.0"}}`)
			return
		}
		_, _ = io.WriteString(w, `{"result":{"id":"s1","memory_interface":{"kip_memory":"2.0"},"future_field":9007199254740993}}`)
	}))
	defer server.Close()
	prefix := []string{"--base-url", server.URL, "--space-id", "s1"}
	out, stderr, err := runCLI(t, nil, append(prefix, "status")...)
	if err != nil || !strings.Contains(out, `"memory_interface"`) {
		t.Fatalf("status: %v out=%s stderr=%s", err, out, stderr)
	}
	out, stderr, err = runCLI(t, nil, append(prefix, "info")...)
	if err != nil || !strings.Contains(out, `"memory_interface"`) || !strings.Contains(out, `"future_field": 9007199254740993`) {
		t.Fatalf("info: %v out=%s stderr=%s", err, out, stderr)
	}
}

func TestCLIRevokeTokenWithoutMatchFails(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"result":false}`)
	}))
	defer server.Close()
	out, stderr, err := runCLI(t, nil, "--base-url", server.URL, "--space-id", "s1", "management", "revoke-token", "--name", "missing")
	if err == nil || !strings.Contains(stderr, "nothing was revoked") || !strings.Contains(out, "false") {
		t.Fatalf("unmatched revoke reported success: %v out=%s stderr=%s", err, out, stderr)
	}
}

func TestCLIServerErrorEnvelopeIsReadable(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusConflict)
		_, _ = io.WriteString(w, `{"error":{"message":"commit conflict","data":{"current_version":12}}}`)
	}))
	defer server.Close()
	_, stderr, err := runCLI(t, nil, "--base-url", server.URL, "--space-id", "s1", "wiki", "commit", "--input", `{"title":"T","content":"# T"}`)
	if err == nil || !strings.Contains(stderr, `HTTP 409: commit conflict (data: {"current_version":12})`) {
		t.Fatalf("error envelope not read: %v %s", err, stderr)
	}
}

type failingWriter struct{}

func (failingWriter) Write([]byte) (int, error) { return 0, io.ErrClosedPipe }
func TestOutputWriteFailureIsReturned(t *testing.T) {
	command := &cobra.Command{}
	command.SetOut(failingWriter{})
	if err := printJSON(command, map[string]int{"ok": 1}); err != io.ErrClosedPipe {
		t.Fatalf("write failure lost: %v", err)
	}
}
