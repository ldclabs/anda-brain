package api

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestExtendedClientRoutes(t *testing.T) {
	ctx := context.Background()
	version, start, end := uint64(9), uint64(0), uint64(12)
	limit, topK, sample := 3, 5, 2
	full := `{"result":{}}`
	list := `{"result":[],"next_cursor":"later"}`
	cases := []struct {
		name, method, target, response, bodyContains string
		call                                         func(*Client) error
	}{
		{"structured recall", "POST", "/v1/s1/recall_structured", full, `"budget"`, func(c *Client) error {
			_, e := c.RecallStructured(ctx, &RecallInput{Query: "q", Budget: &RecallBudget{}})
			return e
		}},
		{"probe", "POST", "/v1/s1/probe", full, `"query":"q"`, func(c *Client) error { _, e := c.Probe(ctx, &ProbeInput{Query: "q"}); return e }},
		{"pin", "POST", "/v1/s1/memory/pin", full, `"entity":"C-7"`, func(c *Client) error { _, e := c.PinMemory(ctx, &MemoryPinInput{Entity: "C-7"}); return e }},
		{"forget", "POST", "/v1/s1/memory/forget", full, `"dry_run":true`, func(c *Client) error {
			_, e := c.ForgetMemory(ctx, &MemoryForgetInput{Entities: []string{"C-7"}, DryRun: true})
			return e
		}},
		{"memory status", "GET", "/v1/s1/memory_status", full, "", func(c *Client) error { _, e := c.GetMemoryStatus(ctx); return e }},
		{"schema drafts", "GET", "/v1/s1/schema/drafts", full, "", func(c *Client) error { _, e := c.SchemaDrafts(ctx); return e }},
		{"schema promote", "POST", "/v1/s1/schema/promote", full, `"kind":"ConceptType"`, func(c *Client) error {
			_, e := c.PromoteDraftSymbol(ctx, &PromoteDraftInput{Kind: "ConceptType", From: "Human", To: "Person"})
			return e
		}},
		{"shadow", "POST", "/v1/s1/management/shadow_eval", full, `"policy"`, func(c *Client) error {
			_, e := c.ShadowEval(ctx, &ShadowEvalInput{Policy: MemoryPolicy{}, ReplaySample: &sample})
			return e
		}},
		{"wiki commit", "POST", "/v1/s1/wiki/docs", full, `"title":"T"`, func(c *Client) error {
			_, e := c.WikiCommit(ctx, &WikiCommitInput{Title: "T", Content: "# T"})
			return e
		}},
		{"wiki list", "GET", "/v1/s1/wiki/docs?limit=3&namespace=docs&status=active", list, "", func(c *Client) error {
			_, e := c.WikiListDocs(ctx, WikiListDocsQuery{Namespace: "docs", Status: "active", Limit: limit})
			return e
		}},
		{"wiki get", "GET", "/v1/s1/wiki/docs/7", full, "", func(c *Client) error { _, e := c.WikiGetDoc(ctx, 7); return e }},
		{"wiki read", "GET", "/v1/s1/wiki/docs/7/content?end=12&start=0&version=9", full, "", func(c *Client) error {
			_, e := c.WikiRead(ctx, 7, WikiReadQuery{Version: &version, Start: &start, End: &end})
			return e
		}},
		{"wiki versions", "GET", "/v1/s1/wiki/docs/7/versions?cursor=old&limit=3", list, "", func(c *Client) error { _, e := c.WikiVersions(ctx, 7, "old", limit); return e }},
		{"wiki archive", "POST", "/v1/s1/wiki/docs/7/archive", full, "", func(c *Client) error { _, e := c.WikiArchive(ctx, 7); return e }},
		{"wiki restore", "POST", "/v1/s1/wiki/docs/7/restore", full, "", func(c *Client) error { _, e := c.WikiRestore(ctx, 7); return e }},
		{"wiki search", "POST", "/v1/s1/wiki/search", full, `"top_k":5`, func(c *Client) error { _, e := c.WikiSearch(ctx, &WikiSearchInput{Query: "q", TopK: &topK}); return e }},
		{"wiki verify", "POST", "/v1/s1/wiki/verify", full, `"uri":"wiki://s1/7@9#0-12"`, func(c *Client) error {
			_, e := c.WikiVerify(ctx, &WikiVerifyInput{URI: "wiki://s1/7@9#0-12"})
			return e
		}},
		{"wiki events", "GET", "/v1/s1/wiki/events?doc_id=7&kind=WikiRead", list, "", func(c *Client) error {
			id := uint64(7)
			_, e := c.WikiEvents(ctx, WikiEventsQuery{Kind: "WikiRead", DocID: &id})
			return e
		}},
		{"wiki import", "POST", "/v1/s1/wiki/import", full, `"entries"`, func(c *Client) error {
			_, e := c.WikiImport(ctx, &WikiImportInput{Entries: []WikiBundleEntry{{Path: "a.md", Content: "# A"}}})
			return e
		}},
		{"wiki export", "GET", "/v1/s1/wiki/export?namespace=docs", full, "", func(c *Client) error { _, e := c.WikiExport(ctx, "docs"); return e }},
		{"wiki digest", "POST", "/v1/s1/wiki/digest", full, "", func(c *Client) error { _, e := c.WikiDigest(ctx); return e }},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			var method, target, body string
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				method, target = r.Method, r.URL.RequestURI()
				data, _ := io.ReadAll(r.Body)
				body = string(data)
				w.Header().Set("Content-Type", "application/json")
				_, _ = w.Write([]byte(tc.response))
			}))
			defer server.Close()
			if err := tc.call(NewClient(server.URL, "s1", "")); err != nil {
				t.Fatal(err)
			}
			if method != tc.method || target != tc.target {
				t.Fatalf("got %s %s, want %s %s", method, target, tc.method, tc.target)
			}
			if tc.bodyContains != "" && !strings.Contains(body, tc.bodyContains) {
				t.Fatalf("body %q missing %q", body, tc.bodyContains)
			}
		})
	}
}

func TestExecuteKIPReadonlyDecodesBothResultLevels(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		if strings.Contains(string(data), `"commands"`) || !strings.Contains(string(data), `"operations"`) {
			t.Errorf("sent old KIP request shape: %s", data)
		}
		_, _ = w.Write([]byte(`{"kip":"2.0","status":"failed","results":[{"status":"failed","error":{"code":"InvalidRequestEnvelope","message":"bad batch"}}]}`))
	}))
	defer server.Close()
	command := "DESCRIBE PRIMER"
	response, err := NewClient(server.URL, "s1", "").ExecuteKIPReadonly(context.Background(), &KipRequest{Operations: []KipOperation{{String: &command}}})
	if err != nil {
		t.Fatal(err)
	}
	if len(response.Results) != 1 || response.Failure() == nil {
		t.Fatalf("KIP result or operation-level error lost: %+v", response)
	}
}

func TestExecuteKIPReadonlyKeepsIncompleteBatchForInspection(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"kip":"2.0","status":"succeeded","results":[{"status":"succeeded"}]}`))
	}))
	defer server.Close()
	command := "DESCRIBE PRIMER"
	request := &KipRequest{Operations: []KipOperation{{String: &command}, {String: &command}}, Execution: &KipExecution{Mode: "independent"}}
	response, err := NewClient(server.URL, "s", "").ExecuteKIPReadonly(context.Background(), request)
	if err == nil || response == nil || len(response.Results) != 1 {
		t.Fatalf("expected the partial response and a result-count error, got %+v, %v", response, err)
	}
}

func TestMemoryDefaultsWireVersionWithoutChangingCallerRequest(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var sent MemoryRequest
		if err := json.NewDecoder(r.Body).Decode(&sent); err != nil || sent.KipMemory != MemoryInterfaceVersion {
			t.Errorf("wire version: %q, %v", sent.KipMemory, err)
		}
		_, _ = w.Write([]byte(`{"kip_memory":"2.0","operation":"recall","status":"succeeded"}`))
	}))
	defer server.Close()
	client := NewClient(server.URL, "s", "")
	request := &MemoryRequest{Operation: "recall", Input: json.RawMessage(`{"mode":"attention"}`)}
	if _, err := client.Memory(context.Background(), request); err != nil || request.KipMemory != "" {
		t.Fatalf("caller request changed or send failed: %+v, %v", request, err)
	}
	if _, err := client.Memory(context.Background(), nil); err == nil {
		t.Fatal("accepted a nil request")
	}
}
