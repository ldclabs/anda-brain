package api

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
)

// MemoryInterfaceVersion is the KIP Memory Interface wire version.
const MemoryInterfaceVersion = "2.0"

// MaxAfter is the most processing receipts one recall may wait on.
const MaxAfter = 128

// MemoryScope names an authorized task and context handles (MI §3).
type MemoryScope struct {
	TaskRef     string   `json:"task_ref,omitempty"`
	ContextRefs []string `json:"context_refs,omitempty"`
}

// MemoryBudget bounds a request's output and deadline.
type MemoryBudget struct {
	MaxOutputTokens *int64 `json:"max_output_tokens,omitempty"`
	DeadlineMs      *int64 `json:"deadline_ms,omitempty"`
	Tokenizer       string `json:"tokenizer,omitempty"`
}

// MemoryRequest is one Memory Interface request: one intent per request.
type MemoryRequest struct {
	KipMemory      string          `json:"kip_memory"`
	RequestID      string          `json:"request_id,omitempty"`
	Operation      string          `json:"operation"`
	Scope          *MemoryScope    `json:"scope,omitempty"`
	Budget         *MemoryBudget   `json:"budget,omitempty"`
	IdempotencyKey string          `json:"idempotency_key,omitempty"`
	Requires       []string        `json:"requires,omitempty"`
	Input          json.RawMessage `json:"input"`
}

// MemoryReceipt is the immutable acknowledgement of a mutation intent.
type MemoryReceipt struct {
	ReceiptRef  string `json:"receipt_ref"`
	Operation   string `json:"operation"`
	SpaceID     string `json:"space_id"`
	AcceptedSeq uint64 `json:"accepted_seq"`
}

// MemoryProgress is a receipt's current processing phase.
type MemoryProgress struct {
	ReceiptRef   string          `json:"receipt_ref"`
	Phase        string          `json:"phase"`
	Disposition  string          `json:"disposition,omitempty"`
	ResolvedSeq  *uint64         `json:"resolved_seq,omitempty"`
	AvailableSeq *uint64         `json:"available_seq,omitempty"`
	Reason       string          `json:"reason,omitempty"`
	Error        json.RawMessage `json:"error,omitempty"`
}

// MemoryResponse is one Memory Interface response. A failed intent carries
// the full KIP error object, with its category, hint and retry advice.
type MemoryResponse struct {
	KipMemory string          `json:"kip_memory"`
	RequestID string          `json:"request_id,omitempty"`
	Operation string          `json:"operation"`
	Status    string          `json:"status"`
	Receipt   *MemoryReceipt  `json:"receipt,omitempty"`
	Progress  *MemoryProgress `json:"progress,omitempty"`
	Result    json.RawMessage `json:"result,omitempty"`
	Error     *KipError       `json:"error,omitempty"`
	Warnings  []string        `json:"warnings,omitempty"`
}

// MemoryCoverage is the part of a Briefing a session reads.
type MemoryCoverage struct {
	Complete        bool              `json:"complete"`
	Channels        map[string]string `json:"channels"`
	PendingReceipts []string          `json:"pending_receipts"`
	ActionEligible  bool              `json:"action_eligible"`
}

// MemoryBriefing is the part of a recall result a session reads; the full
// briefing is kept as returned.
type MemoryBriefing struct {
	Summary         string           `json:"summary"`
	BasisRef        string           `json:"basis_ref"`
	Coverage        MemoryCoverage   `json:"coverage"`
	After           []MemoryProgress `json:"after"`
	AttentionCursor string           `json:"attention_cursor,omitempty"`
}

// SourceOrder is a host-captured transport attestation of source order.
type SourceOrder struct {
	StreamRef           string   `json:"stream_ref"`
	EventRef            string   `json:"event_ref"`
	Ordinal             uint64   `json:"ordinal"`
	PredecessorReceipts []string `json:"predecessor_receipts,omitempty"`
}

// StageSourceInput stages observed messages for observe, revise or feedback.
type StageSourceInput struct {
	Messages       []Message    `json:"messages"`
	ObservedAt     string       `json:"observed_at,omitempty"`
	Kind           string       `json:"kind,omitempty"`
	Order          *SourceOrder `json:"order,omitempty"`
	IdempotencyKey string       `json:"idempotency_key"`
}

// StagedSourceRef is the handle a staged source is cited by.
type StagedSourceRef struct {
	SourceRef    string `json:"source_ref"`
	SourceDigest string `json:"source_digest"`
	CapturedAt   string `json:"captured_at"`
}

// StageMemorySource stages a captured source and returns its handle.
func (c *Client) StageMemorySource(ctx context.Context, input *StageSourceInput) (*RpcResponse[StagedSourceRef], error) {
	return callRPC[StagedSourceRef](ctx, c, http.MethodPost, c.spacePath("/memory/sources"), input)
}

// MemorySource reads a staged source the caller owns.
func (c *Client) MemorySource(ctx context.Context, sourceRef string) (*RpcResponse[json.RawMessage], error) {
	return callRPC[json.RawMessage](ctx, c, http.MethodGet, c.spacePath("/memory/sources/"+url.PathEscape(sourceRef)), nil)
}

// MemoryReceiptView reads a receipt's current progress.
func (c *Client) MemoryReceiptView(ctx context.Context, receiptRef string) (*RpcResponse[json.RawMessage], error) {
	return callRPC[json.RawMessage](ctx, c, http.MethodGet, c.spacePath("/memory/receipts/"+url.PathEscape(receiptRef)), nil)
}

// MemoryPlan reads a forget's ErasurePlan.
func (c *Client) MemoryPlan(ctx context.Context, planRef string) (*RpcResponse[json.RawMessage], error) {
	return callRPC[json.RawMessage](ctx, c, http.MethodGet, c.spacePath("/memory/plans/"+url.PathEscape(planRef)), nil)
}

// Memory sends one Memory Interface request. The response is the Memory
// Interface Response itself, not an RPC envelope; a failed intent is a
// response with status "failed" and an error, not a Go error.
func (c *Client) Memory(ctx context.Context, request *MemoryRequest) (*MemoryResponse, error) {
	if request == nil {
		return nil, fmt.Errorf("missing Memory Interface request")
	}
	if request.KipMemory == "" {
		withVersion := *request
		withVersion.KipMemory = MemoryInterfaceVersion
		request = &withVersion
	}
	data, err := c.doJSON(ctx, http.MethodPost, c.spacePath("/memory"), request)
	if err != nil {
		return nil, err
	}
	var response MemoryResponse
	if err := DecodeJSON(data, &response); err != nil {
		return nil, fmt.Errorf("decode memory response: %w", err)
	}
	return &response, nil
}
