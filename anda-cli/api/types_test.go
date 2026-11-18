package api

import (
	"bytes"
	"encoding/json"
	"testing"
)

func TestMessageContentUnmarshalString(t *testing.T) {
	var content MessageContent
	if err := json.Unmarshal([]byte(`"hello"`), &content); err != nil {
		t.Fatalf("unmarshal string content: %v", err)
	}
	if len(content) != 1 {
		t.Fatalf("expected 1 part, got %d", len(content))
	}
	textPart, ok := content[0].(TextPart)
	if !ok || textPart.Text != "hello" {
		t.Fatalf("unexpected first part: %+v", content[0])
	}
}

func TestMessageContentUnmarshalArrayTypedParts(t *testing.T) {
	raw := `[
		{"type":"Text","text":"hi"},
		{"type":"Reasoning","text":"think"},
		{"type":"ToolCall","name":"sum","args":{"x":1,"y":2},"callId":"c1"}
	]`

	var content MessageContent
	if err := json.Unmarshal([]byte(raw), &content); err != nil {
		t.Fatalf("unmarshal array content: %v", err)
	}
	if len(content) != 3 {
		t.Fatalf("expected 3 parts, got %d", len(content))
	}
	textPart, ok := content[0].(TextPart)
	if !ok || textPart.Text != "hi" {
		t.Fatalf("unexpected text part: %+v", content[0])
	}
	reasoningPart, ok := content[1].(ReasoningPart)
	if !ok || reasoningPart.Text != "think" {
		t.Fatalf("unexpected reasoning part: %+v", content[1])
	}
	toolCallPart, ok := content[2].(ToolCallPart)
	if !ok || toolCallPart.Name != "sum" || toolCallPart.CallID == nil || *toolCallPart.CallID != "c1" {
		t.Fatalf("unexpected toolcall part: %+v", content[2])
	}
}

func TestContentPartUnknownTypeGoesAny(t *testing.T) {
	part, err := parseContentPart([]byte(`{"type":"text/plain","data":"aGVsbG8="}`))
	if err != nil {
		t.Fatalf("unmarshal unknown type: %v", err)
	}
	anyPart, ok := part.(AnyPart)
	if !ok {
		t.Fatalf("expected AnyPart, got %T", part)
	}
	if len(anyPart.Raw) == 0 {
		t.Fatalf("expected raw payload in Any")
	}

	out, err := json.Marshal(anyPart)
	if err != nil {
		t.Fatalf("marshal any part: %v", err)
	}
	if string(out) != `{"type":"text/plain","data":"aGVsbG8="}` {
		t.Fatalf("unexpected any marshal output: %s", string(out))
	}
}

func TestContentPartKnownTypeInvalidPayloadFails(t *testing.T) {
	_, err := parseContentPart([]byte(`{"type":"Text"}`))
	if err == nil {
		t.Fatalf("expected error for invalid known ContentPart payload")
	}
}

func TestMessageContentMarshalFromText(t *testing.T) {
	content := MessageContentFromText("hello")
	b, err := json.Marshal(content)
	if err != nil {
		t.Fatalf("marshal message content: %v", err)
	}
	if string(b) != `[{"type":"Text","text":"hello"}]` {
		t.Fatalf("unexpected marshal output: %s", string(b))
	}
}

func TestMessageContentPartsAlwaysEncodeTheirType(t *testing.T) {
	callID := "c1"
	content := MessageContent{
		TextPart{Text: "hello"},
		&ReasoningPart{Text: "thinking"},
		ToolCallPart{Type: ContentPartText, Name: "sum", Args: map[string]int{"x": 1}, CallID: &callID},
		(*TextPart)(nil),
	}
	data, err := json.Marshal(content)
	if err != nil {
		t.Fatal(err)
	}
	want := `[{"type":"Text","text":"hello"},{"type":"Reasoning","text":"thinking"},` +
		`{"type":"ToolCall","name":"sum","args":{"x":1},"callId":"c1"},null]`
	if string(data) != want {
		t.Fatalf("got %s, want %s", data, want)
	}
}

func TestKipOperationObjectWithoutParameters(t *testing.T) {
	var item KipOperation
	if err := json.Unmarshal([]byte(`{"command":"DESCRIBE PRIMER"}`), &item); err != nil {
		t.Fatalf("unmarshal command object without parameters: %v", err)
	}
	if item.Object == nil || item.Object.Command != "DESCRIBE PRIMER" {
		t.Fatalf("unexpected item: %+v", item)
	}

	// nil parameters must be omitted on the wire: the server rejects
	// an explicit "parameters": null.
	encoded, err := json.Marshal(item)
	if err != nil {
		t.Fatalf("marshal command item: %v", err)
	}
	if string(encoded) != `{"command":"DESCRIBE PRIMER"}` {
		t.Fatalf("unexpected encoding: %s", encoded)
	}
}

func TestRevokeSpaceTokenInputEncoding(t *testing.T) {
	byToken, err := json.Marshal(RevokeSpaceTokenInput{Token: "STabc"})
	if err != nil {
		t.Fatalf("marshal revoke by token: %v", err)
	}
	if string(byToken) != `{"token":"STabc"}` {
		t.Fatalf("unexpected revoke-by-token encoding: %s", byToken)
	}

	// Revoking by name must not send an empty "token": the server prefers
	// the name path only when token is absent/empty, and an explicit empty
	// token key is just noise.
	byName, err := json.Marshal(RevokeSpaceTokenInput{Name: "reader"})
	if err != nil {
		t.Fatalf("marshal revoke by name: %v", err)
	}
	if string(byName) != `{"name":"reader"}` {
		t.Fatalf("unexpected revoke-by-name encoding: %s", byName)
	}
}

func TestAddSpaceTokenInputLabelsEncoding(t *testing.T) {
	withLabels, err := json.Marshal(AddSpaceTokenInput{
		Scope:  TokenScopeRead,
		Name:   "hr-viewer",
		Labels: &[]string{"hr", "finance"},
	})
	if err != nil {
		t.Fatalf("marshal input with labels: %v", err)
	}
	if string(withLabels) != `{"scope":"read","name":"hr-viewer","labels":["hr","finance"]}` {
		t.Fatalf("unexpected encoding with labels: %s", withLabels)
	}

	// nil labels = unrestricted; the key must be omitted, not sent as null.
	unrestricted, err := json.Marshal(AddSpaceTokenInput{Scope: TokenScopeWrite, Name: "writer"})
	if err != nil {
		t.Fatalf("marshal input without labels: %v", err)
	}
	if string(unrestricted) != `{"scope":"write","name":"writer"}` {
		t.Fatalf("unexpected encoding without labels: %s", unrestricted)
	}
	unlabeled, err := json.Marshal(AddSpaceTokenInput{Scope: TokenScopeRead, Name: "public-viewer", Labels: &[]string{}})
	if err != nil {
		t.Fatal(err)
	}
	if string(unlabeled) != `{"scope":"read","name":"public-viewer","labels":[]}` {
		t.Fatalf("empty labels must stay distinct from unrestricted: %s", unlabeled)
	}
}

func TestSpaceTokenDecodesLabels(t *testing.T) {
	var st SpaceToken
	raw := `{"name":"hr-viewer","token":"STabc123…","scope":"read","usage":2,"created_at":1,"updated_at":2,"expires_at":null,"labels":["hr"]}`
	if err := json.Unmarshal([]byte(raw), &st); err != nil {
		t.Fatalf("unmarshal space token: %v", err)
	}
	if st.Labels == nil || len(*st.Labels) != 1 || (*st.Labels)[0] != "hr" {
		t.Fatalf("unexpected labels: %+v", st.Labels)
	}
	var unlabeled SpaceToken
	if err := json.Unmarshal([]byte(`{"name":"viewer","scope":"read","labels":[]}`), &unlabeled); err != nil {
		t.Fatal(err)
	}
	if unlabeled.Labels == nil || len(*unlabeled.Labels) != 0 {
		t.Fatalf("unlabeled-only restriction lost: %+v", unlabeled.Labels)
	}

	var unrestricted SpaceToken
	raw = `{"name":"writer","token":"STdef456…","scope":"write","usage":0,"created_at":1,"updated_at":1,"labels":null}`
	if err := json.Unmarshal([]byte(raw), &unrestricted); err != nil {
		t.Fatalf("unmarshal unrestricted token: %v", err)
	}
	if unrestricted.Labels != nil {
		t.Fatalf("expected nil labels for unrestricted token, got %+v", unrestricted.Labels)
	}
}

func TestUpdateSpaceInputWikiFieldsEncoding(t *testing.T) {
	enabled := true
	defaults := map[string]string{"internal": "staff"}
	withFields, err := json.Marshal(UpdateSpaceInput{
		WikiDigest:      &enabled,
		WikiAuditReads:  &enabled,
		WikiACLDefaults: &defaults,
	})
	if err != nil {
		t.Fatalf("marshal update-space input: %v", err)
	}
	want := `{"wiki_digest":true,"wiki_audit_reads":true,"wiki_acl_defaults":{"internal":"staff"}}`
	if string(withFields) != want {
		t.Fatalf("unexpected encoding: %s, want %s", withFields, want)
	}

	// A pointer to an empty map replaces (clears) the server-side map; it
	// must be sent as {} rather than omitted.
	empty := map[string]string{}
	clearDefaults, err := json.Marshal(UpdateSpaceInput{WikiACLDefaults: &empty})
	if err != nil {
		t.Fatalf("marshal clear-defaults input: %v", err)
	}
	if string(clearDefaults) != `{"wiki_acl_defaults":{}}` {
		t.Fatalf("unexpected clear-defaults encoding: %s", clearDefaults)
	}

	// Untouched optional fields stay off the wire (server Option semantics).
	unset, err := json.Marshal(UpdateSpaceInput{})
	if err != nil {
		t.Fatalf("marshal empty input: %v", err)
	}
	if string(unset) != `{}` {
		t.Fatalf("unexpected empty encoding: %s", unset)
	}
}

func TestModelConfigRoundTrip(t *testing.T) {
	raw := `{"family":"anthropic","model":"claude-opus-4-6","api_base":"https://api.anthropic.com/v1",` +
		`"api_key":"sk-xxx","disabled":false,"label":"primary","bearer_auth":true,"stream":true,` +
		`"context_window":200000,"max_output":8192}`

	var cfg ModelConfig
	if err := json.Unmarshal([]byte(raw), &cfg); err != nil {
		t.Fatalf("unmarshal model config: %v", err)
	}
	if cfg.Label != "primary" || !cfg.BearerAuth || !cfg.Stream {
		t.Fatalf("unexpected config: %+v", cfg)
	}
	if cfg.ContextWindow != 200000 || cfg.MaxOutput != 8192 {
		t.Fatalf("unexpected window/output: %+v", cfg)
	}

	// Server treats missing fields as defaults (serde default), so zero
	// values are omitted on the wire.
	minimal, err := json.Marshal(ModelConfig{
		Family:  "openai",
		Model:   "gpt-6",
		APIBase: "https://api.openai.com/v1",
		APIKey:  "sk-yyy",
	})
	if err != nil {
		t.Fatalf("marshal minimal config: %v", err)
	}
	want := `{"family":"openai","model":"gpt-6","api_base":"https://api.openai.com/v1","api_key":"sk-yyy"}`
	if string(minimal) != want {
		t.Fatalf("unexpected minimal encoding: %s, want %s", minimal, want)
	}
}

func TestKipRequestSingleCommand(t *testing.T) {
	var req KipRequest
	if err := json.Unmarshal([]byte(`{"command":"DESCRIBE PRIMER"}`), &req); err != nil {
		t.Fatalf("unmarshal single-command request: %v", err)
	}
	if req.Command != "DESCRIBE PRIMER" || len(req.Operations) != 0 {
		t.Fatalf("unexpected request: %+v", req)
	}

	encoded, err := json.Marshal(req)
	if err != nil {
		t.Fatalf("marshal request: %v", err)
	}
	if string(encoded) != `{"command":"DESCRIBE PRIMER"}` {
		t.Fatalf("unexpected encoding: %s", encoded)
	}
}

func TestKipBatchRequestAndResponse(t *testing.T) {
	var request KipRequest
	if err := json.Unmarshal([]byte(`{"operations":["DESCRIBE PRIMER",{"op_id":"schema","command":"DESCRIBE SCHEMA ENVIRONMENT"}],"execution":{"mode":"independent"}}`), &request); err != nil {
		t.Fatalf("unmarshal KIP 2.0 batch: %v", err)
	}
	if len(request.Operations) != 2 || request.Execution == nil || request.Execution.Mode != "independent" {
		t.Fatalf("batch request lost operations or mode: %+v", request)
	}
	encoded, err := json.Marshal(request)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(encoded, []byte(`"operations"`)) || bytes.Contains(encoded, []byte(`"commands"`)) {
		t.Fatalf("wrong batch wire shape: %s", encoded)
	}

	var response KipResponse[any]
	if err := json.Unmarshal([]byte(`{"kip":"2.0","status":"succeeded","results":[{"status":"succeeded","result":{"value":1},"next_cursor":"next"}]}`), &response); err != nil {
		t.Fatalf("unmarshal KIP 2.0 response: %v", err)
	}
	if err := response.Failure(); err != nil {
		t.Fatalf("unexpected failure: %v", err)
	}
	if len(response.Results) != 1 || response.Results[0].NextCursor != "next" || response.Results[0].Result == nil {
		t.Fatalf("response lost operation result: %+v", response)
	}

	if err := json.Unmarshal([]byte(`{"kip":"2.0","status":"failed","results":[{"status":"failed","error":{"code":"SyntaxError","message":"bad command"}}]}`), &response); err != nil {
		t.Fatal(err)
	}
	if response.Failure() == nil {
		t.Fatal("operation-level failure was not detected")
	}
	response.Status = "succeeded"
	response.Results = []KipOperationResult[any]{{Status: "succeeded"}, {Status: "no_effect"}}
	if err := response.Failure(); err != nil {
		t.Fatal(err)
	}
	response.Results[1].Status = "skipped"
	if response.Failure() == nil {
		t.Fatal("a skipped operation cannot confirm a successful batch")
	}
}

func TestCurrentSpaceAndConceptFieldsSurviveDecode(t *testing.T) {
	var concept Concept
	input := []byte(`{"id":"C-7","kind":"concept","schema_ref":"kip://profiles/cognitive-memory@2.0.0/Person","key":"user-1","name":"Alice","governance":{"authority_class":"descriptive"}}`)
	if err := json.Unmarshal(input, &concept); err != nil {
		t.Fatal(err)
	}
	if concept.SchemaRef == "" || concept.Key != "user-1" {
		t.Fatalf("identity lost: %+v", concept)
	}
	encoded, err := json.Marshal(concept)
	if err != nil {
		t.Fatal(err)
	}
	for _, field := range []string{`"schema_ref"`, `"key"`, `"governance"`} {
		if !bytes.Contains(encoded, []byte(field)) {
			t.Fatalf("%s lost on output: %s", field, encoded)
		}
	}

	var info SpaceInfo
	if err := json.Unmarshal([]byte(`{"id":"s1","wiki_docs":7,"wiki_chunks":11,"wiki_versions":8,"wiki_queries":3,"wiki_digested":6,"wiki_stale_docs":2}`), &info); err != nil {
		t.Fatal(err)
	}
	if info.WikiDocs == nil || *info.WikiDocs != 7 || info.WikiDigested == nil || *info.WikiDigested != 6 || info.WikiStaleDocs == nil || *info.WikiStaleDocs != 2 {
		t.Fatalf("wiki stats lost: %+v", info)
	}
	var empty SpaceInfo
	if err := json.Unmarshal([]byte(`{"id":"empty","wiki_docs":0}`), &empty); err != nil {
		t.Fatal(err)
	}
	encodedInfo, err := json.Marshal(empty)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(encodedInfo, []byte(`"wiki_docs":0`)) {
		t.Fatalf("zero-valued wiki stats disappeared: %s", encodedInfo)
	}
	var config ModelConfig
	if err := json.Unmarshal([]byte(`{"family":"anthropic","model":"m","api_base":"u","api_key":"k","effort":"high"}`), &config); err != nil {
		t.Fatal(err)
	}
	if config.Effort != "high" {
		t.Fatalf("effort lost: %+v", config)
	}
}

func TestMemoryPolicyOmissionsUseServerDefaults(t *testing.T) {
	maxRounds := uint32(8)
	data, err := json.Marshal(UpdateSpaceInput{MemoryPolicy: &MemoryPolicy{RecallMaxRounds: &maxRounds}})
	if err != nil {
		t.Fatal(err)
	}
	if string(data) != `{"memory_policy":{"recall_max_rounds":8}}` {
		t.Fatalf("unspecified policy members must remain omitted: %s", data)
	}
}

func TestWikiCommitCanClearTagsAndMetadata(t *testing.T) {
	tags := []string{}
	metadata := map[string]any{}
	data, err := json.Marshal(WikiCommitInput{Title: "T", Content: "# T", Tags: &tags, Metadata: &metadata})
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(data, []byte(`"tags":[]`)) || !bytes.Contains(data, []byte(`"metadata":{}`)) {
		t.Fatalf("clear operations were omitted: %s", data)
	}
}
