package cmd

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"

	"github.com/ldclabs/anda-brain/anda-cli/api"
)

func TestParseMessagesInput_JSONArray(t *testing.T) {
	messages, err := parseMessagesInput(`[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi"}]`)
	if err != nil {
		t.Fatalf("parseMessagesInput returned error: %v", err)
	}
	if len(messages) != 2 {
		t.Fatalf("expected 2 messages, got %d", len(messages))
	}
	text, ok := messages[0].Content.FirstText()
	if messages[0].Role != "user" || !ok || text != "Hello" {
		t.Fatalf("unexpected first message: %+v", messages[0])
	}
}

func TestParseMessagesInput_JSONObject(t *testing.T) {
	messages, err := parseMessagesInput(`{"role":"user","content":"Only one"}`)
	if err != nil {
		t.Fatalf("parseMessagesInput returned error: %v", err)
	}
	if len(messages) != 1 {
		t.Fatalf("expected 1 message, got %d", len(messages))
	}
	text, ok := messages[0].Content.FirstText()
	if messages[0].Role != "user" || !ok || text != "Only one" {
		t.Fatalf("unexpected message: %+v", messages[0])
	}
}

func TestParseMessagesInput_PlainTextFallback(t *testing.T) {
	messages, err := parseMessagesInput("plain text input")
	if err != nil {
		t.Fatalf("parseMessagesInput returned error: %v", err)
	}
	if len(messages) != 1 {
		t.Fatalf("expected 1 message, got %d", len(messages))
	}
	text, ok := messages[0].Content.FirstText()
	if messages[0].Role != "user" || !ok || text != "plain text input" {
		t.Fatalf("unexpected message: %+v", messages[0])
	}
}

func TestParseMessagesInput_EmptyInput(t *testing.T) {
	_, err := parseMessagesInput("   \n\t  ")
	if err == nil {
		t.Fatalf("expected error for empty input")
	}
}

func TestParseMessagesInput_RejectsNonMessageJSON(t *testing.T) {
	// Valid JSON object that is not a message must not be silently
	// converted into an empty message or plain text.
	if _, err := parseMessagesInput(`{"foo":1}`); err == nil {
		t.Fatalf("expected error for JSON object without role/content")
	}
	if _, err := parseMessagesInput(`[{"role":"user"}]`); err == nil {
		t.Fatalf("expected error for message missing content")
	}
	if _, err := parseMessagesInput(`["just a string"]`); err == nil {
		t.Fatalf("expected error for JSON array of strings")
	}
}

func TestParseMessagesInput_PlainTextLooksLikeJSON(t *testing.T) {
	// Non-valid-JSON input starting with '[' (e.g. log lines) stays plain text.
	messages, err := parseMessagesInput("[2026-06-11] meeting notes")
	if err != nil {
		t.Fatalf("parseMessagesInput returned error: %v", err)
	}
	text, ok := messages[0].Content.FirstText()
	if messages[0].Role != "user" || !ok || text != "[2026-06-11] meeting notes" {
		t.Fatalf("unexpected message: %+v", messages[0])
	}
}

func TestResolveBatchSelector(t *testing.T) {
	sel, err := resolveBatchSelector("Skill.md", "")
	if err != nil {
		t.Fatalf("resolveBatchSelector by name returned error: %v", err)
	}
	if sel != "name:skill.md" {
		t.Fatalf("unexpected selector: %q", sel)
	}

	sel, err = resolveBatchSelector("", "md")
	if err != nil {
		t.Fatalf("resolveBatchSelector by extension returned error: %v", err)
	}
	if sel != "ext:.md" {
		t.Fatalf("unexpected selector: %q", sel)
	}

	if _, err := resolveBatchSelector("", ""); err == nil {
		t.Fatalf("expected error when selector is empty")
	}
	if _, err := resolveBatchSelector("Skill.md", "md"); err == nil {
		t.Fatalf("expected error when file name and extension are both set")
	}
}

func TestMatchesBatchSelector(t *testing.T) {
	tests := []struct {
		name string
		file string
		sel  string
		want bool
	}{
		{name: "name match lowercase", file: "skill.md", sel: "name:skill.md", want: true},
		{name: "name match uppercase", file: "SKILL.md", sel: "name:skill.md", want: true},
		{name: "name mismatch", file: "README.md", sel: "name:skill.md", want: false},
		{name: "ext match", file: "doc.MD", sel: "ext:.md", want: true},
		{name: "ext mismatch", file: "archive.txt", sel: "ext:.md", want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := matchesBatchSelector(tt.file, tt.sel)
			if got != tt.want {
				t.Fatalf("matchesBatchSelector(%q, %q) = %v, want %v", tt.file, tt.sel, got, tt.want)
			}
		})
	}
}

func TestFindBatchFilesRecursiveByName(t *testing.T) {
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, "a", "b"))
	mustMkdirAll(t, filepath.Join(root, "x"))

	mustWriteFile(t, filepath.Join(root, "Skill.md"), "root")
	mustWriteFile(t, filepath.Join(root, "a", "SKILL.md"), "nested")
	mustWriteFile(t, filepath.Join(root, "a", "b", "skill.md"), "deep")
	mustWriteFile(t, filepath.Join(root, "x", "README.md"), "ignore")

	files, err := findBatchFiles(root, "name:skill.md", nil)
	if err != nil {
		t.Fatalf("findBatchFiles returned error: %v", err)
	}
	if len(files) != 3 {
		t.Fatalf("expected 3 skill files, got %d: %v", len(files), files)
	}
}

func TestFindBatchFilesRecursiveByExtension(t *testing.T) {
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, "a", "b"))
	mustMkdirAll(t, filepath.Join(root, "x"))

	mustWriteFile(t, filepath.Join(root, "Skill.md"), "root")
	mustWriteFile(t, filepath.Join(root, "a", "SKILL.MD"), "nested")
	mustWriteFile(t, filepath.Join(root, "a", "b", "note.md"), "deep")
	mustWriteFile(t, filepath.Join(root, "x", "README.txt"), "ignore")

	files, err := findBatchFiles(root, "ext:.md", nil)
	if err != nil {
		t.Fatalf("findBatchFiles returned error: %v", err)
	}
	if len(files) != 3 {
		t.Fatalf("expected 3 markdown files, got %d: %v", len(files), files)
	}
}

func TestFindBatchFilesSkipsHiddenAndExcluded(t *testing.T) {
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, ".git"))
	mustMkdirAll(t, filepath.Join(root, "docs"))

	mustWriteFile(t, filepath.Join(root, "keep.json"), "{}")
	mustWriteFile(t, filepath.Join(root, "docs", "data.json"), "{}")
	mustWriteFile(t, filepath.Join(root, ".hidden.json"), "{}")
	mustWriteFile(t, filepath.Join(root, ".git", "config.json"), "{}")
	mustWriteFile(t, filepath.Join(root, defaultBatchReportFileName), "{}")
	excluded := filepath.Join(root, "report.json")
	mustWriteFile(t, excluded, "{}")

	files, err := findBatchFiles(root, "ext:.json", map[string]bool{excluded: true, excluded + ".tmp": true})
	if err != nil {
		t.Fatalf("findBatchFiles returned error: %v", err)
	}
	if len(files) != 2 {
		t.Fatalf("expected 2 files, got %d: %v", len(files), files)
	}
	for _, f := range files {
		name := filepath.Base(f)
		if name != "keep.json" && name != "data.json" {
			t.Fatalf("unexpected file matched: %s", f)
		}
	}
}

func TestFindBatchFilesHiddenRootDirIsWalked(t *testing.T) {
	parent := t.TempDir()
	root := filepath.Join(parent, ".claude")
	mustMkdirAll(t, root)
	mustWriteFile(t, filepath.Join(root, "Skill.md"), "content")

	files, err := findBatchFiles(root, "name:skill.md", nil)
	if err != nil {
		t.Fatalf("findBatchFiles returned error: %v", err)
	}
	if len(files) != 1 {
		t.Fatalf("expected 1 file under hidden root, got %d: %v", len(files), files)
	}
}

func TestChecklistLoadSaveAndRootOrSelectorMismatch(t *testing.T) {
	root := t.TempDir()
	reportPath := filepath.Join(root, ".formation-batch-checklist.json")

	checklist, err := loadFileFormationChecklist(reportPath, root, "name:skill.md")
	if err != nil {
		t.Fatalf("load checklist returned error: %v", err)
	}
	if checklist.RootDir != root {
		t.Fatalf("unexpected root dir: %q", checklist.RootDir)
	}
	if checklist.Selector != "name:skill.md" {
		t.Fatalf("unexpected selector: %q", checklist.Selector)
	}

	checklist.Entries["Skill.md"] = &fileFormationChecklistEntry{
		Path:      "Skill.md",
		Status:    batchStatusSucceeded,
		Attempts:  1,
		UpdatedAt: "2026-01-01T00:00:00Z",
	}

	if err := saveFileFormationChecklist(reportPath, checklist); err != nil {
		t.Fatalf("save checklist returned error: %v", err)
	}

	reloaded, err := loadFileFormationChecklist(reportPath, root, "name:skill.md")
	if err != nil {
		t.Fatalf("reload checklist returned error: %v", err)
	}
	if len(reloaded.Entries) != 1 {
		t.Fatalf("expected 1 checklist entry, got %d", len(reloaded.Entries))
	}
	if reloaded.Entries["Skill.md"].Status != batchStatusSucceeded {
		t.Fatalf("unexpected entry status: %q", reloaded.Entries["Skill.md"].Status)
	}

	otherRoot := filepath.Join(root, "other")
	mustMkdirAll(t, otherRoot)
	if _, err := loadFileFormationChecklist(reportPath, otherRoot, "name:skill.md"); err == nil {
		t.Fatalf("expected root mismatch error")
	}
	if _, err := loadFileFormationChecklist(reportPath, root, "ext:.md"); err == nil {
		t.Fatalf("expected selector mismatch error")
	}
}

func TestShouldProcessBatchEntry(t *testing.T) {
	if shouldProcessBatchEntry(nil, false) != true {
		t.Fatalf("nil entry should be processed")
	}
	if shouldProcessBatchEntry(&fileFormationChecklistEntry{Status: batchStatusSucceeded}, true) != false {
		t.Fatalf("succeeded entry should be skipped")
	}
	if shouldProcessBatchEntry(&fileFormationChecklistEntry{Status: batchStatusFailed}, false) != false {
		t.Fatalf("failed entry should be skipped when retry disabled")
	}
	if shouldProcessBatchEntry(&fileFormationChecklistEntry{Status: batchStatusFailed}, true) != true {
		t.Fatalf("failed entry should be processed when retry enabled")
	}
}

func TestRunFileFormationBatchDryRun(t *testing.T) {
	root := t.TempDir()
	mustMkdirAll(t, filepath.Join(root, "a"))
	mustWriteFile(t, filepath.Join(root, "a", "Skill.md"), "plain text")

	reportPath := filepath.Join(root, "report.json")
	err := runFileFormationBatch(context.Background(), nil, fileFormationBatchOptions{
		RootDir:    root,
		FileName:   "Skill.md",
		ReportPath: reportPath,
		DryRun:     true,
	})
	if err != nil {
		t.Fatalf("runFileFormationBatch dry-run returned error: %v", err)
	}

	checklist, err := loadFileFormationChecklist(reportPath, root, "name:skill.md")
	if err != nil {
		t.Fatalf("load checklist returned error: %v", err)
	}
	entry, ok := checklist.Entries[filepath.Join("a", "Skill.md")]
	if !ok {
		t.Fatalf("expected checklist entry for a/Skill.md")
	}
	if entry.Attempts != 0 {
		t.Fatalf("dry-run should not increment attempts, got %d", entry.Attempts)
	}
	if entry.Status != batchStatusPending {
		t.Fatalf("dry-run should keep pending status, got %q", entry.Status)
	}
}

func TestRunFileFormationBatchPreservesInputContext(t *testing.T) {
	root := t.TempDir()
	mustWriteFile(t, filepath.Join(root, "Skill.md"), "plain text memory")

	var received []api.FormationInput
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var input api.FormationInput
		if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
			t.Errorf("decode formation input: %v", err)
		}
		received = append(received, input)
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"result":{"content":"ok","conversation":7}}`))
	}))
	defer server.Close()

	client := api.NewClient(server.URL, "test_space", "token")
	err := runFileFormationBatch(context.Background(), client, fileFormationBatchOptions{
		RootDir:    root,
		FileName:   "Skill.md",
		ReportPath: filepath.Join(root, "report.json"),
		InputContext: &api.InputContext{
			Counterparty: "u1",
			Agent:        "agent1",
			Topic:        "topic1",
		},
	})
	if err != nil {
		t.Fatalf("runFileFormationBatch returned error: %v", err)
	}

	if len(received) != 1 {
		t.Fatalf("expected 1 formation request, got %d", len(received))
	}
	got := received[0].Context
	if got == nil {
		t.Fatalf("expected context to be sent")
	}
	if got.Counterparty != "u1" || got.Agent != "agent1" || got.Topic != "topic1" {
		t.Fatalf("input context not preserved: %+v", got)
	}
	if got.Source == "" {
		t.Fatalf("expected per-file source to be set")
	}
}

func TestRunFileFormationBatchNilContext(t *testing.T) {
	root := t.TempDir()
	mustWriteFile(t, filepath.Join(root, "Skill.md"), "plain text memory")

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		_, _ = w.Write([]byte(`{"result":{"content":"ok","conversation":8}}`))
	}))
	defer server.Close()

	client := api.NewClient(server.URL, "test_space", "token")
	err := runFileFormationBatch(context.Background(), client, fileFormationBatchOptions{
		RootDir:    root,
		FileName:   "Skill.md",
		ReportPath: filepath.Join(root, "report.json"),
	})
	if err != nil {
		t.Fatalf("runFileFormationBatch with nil context returned error: %v", err)
	}
}

func mustWriteFile(t *testing.T, path, content string) {
	t.Helper()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("write file %q: %v", path, err)
	}
}

func mustMkdirAll(t *testing.T, path string) {
	t.Helper()
	if err := os.MkdirAll(path, 0o755); err != nil {
		t.Fatalf("mkdir %q: %v", path, err)
	}
}
