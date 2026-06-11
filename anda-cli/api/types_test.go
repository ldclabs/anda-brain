package api

import (
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

	out, err := marshalContentPart(anyPart)
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

func TestMessageContentTextAndFirstText(t *testing.T) {
	content := MessageContent{
		TextPart{Type: ContentPartText, Text: "hello"},
		ReasoningPart{Type: ContentPartReasoning, Text: "thinking"},
		TextPart{Type: ContentPartText, Text: "world"},
	}

	text, ok := content.Text()
	if !ok || text != "hello\nworld" {
		t.Fatalf("unexpected text aggregation: ok=%v text=%q", ok, text)
	}

	first, ok := content.FirstText()
	if !ok || first != "hello" {
		t.Fatalf("unexpected first text: ok=%v first=%q", ok, first)
	}
}

func TestKipCommandItemObjectWithoutParameters(t *testing.T) {
	var item KipCommandItem
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

func TestKipRequestSingleCommand(t *testing.T) {
	var req KipRequest
	if err := json.Unmarshal([]byte(`{"command":"DESCRIBE PRIMER"}`), &req); err != nil {
		t.Fatalf("unmarshal single-command request: %v", err)
	}
	if req.Command != "DESCRIBE PRIMER" || len(req.Commands) != 0 {
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
