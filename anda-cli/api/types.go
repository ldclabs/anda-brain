package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// RpcError represents an API error.
type RpcError struct {
	Message string `json:"message"`
	Data    any    `json:"data,omitempty"`
}

func (e *RpcError) Error() string {
	return e.Message
}

// HTTPError keeps the status and structured error payload, including wiki
// conflict retry data, while still rendering a useful CLI message.
type HTTPError struct {
	StatusCode int
	RPC        *RpcError
	Body       string
}

func (e *HTTPError) Error() string {
	if e.RPC != nil {
		if e.RPC.Data != nil {
			data, _ := json.Marshal(e.RPC.Data)
			return fmt.Sprintf("HTTP %d: %s (data: %s)", e.StatusCode, e.RPC.Message, data)
		}
		return fmt.Sprintf("HTTP %d: %s", e.StatusCode, e.RPC.Message)
	}
	return fmt.Sprintf("HTTP %d: %s", e.StatusCode, e.Body)
}

// RpcResponse is the generic RPC envelope.
type RpcResponse[T any] struct {
	Result     *T        `json:"result,omitempty"`
	Error      *RpcError `json:"error,omitempty"`
	NextCursor string    `json:"next_cursor,omitempty"`
	// RawResult is the result exactly as the server sent it, including members
	// T does not model yet.
	RawResult json.RawMessage `json:"-"`
}

func (r *RpcResponse[T]) UnmarshalJSON(data []byte) error {
	var envelope struct {
		Result     json.RawMessage `json:"result"`
		Error      *RpcError       `json:"error"`
		NextCursor string          `json:"next_cursor"`
	}
	if err := DecodeJSON(data, &envelope); err != nil {
		return err
	}
	*r = RpcResponse[T]{Error: envelope.Error, NextCursor: envelope.NextCursor}
	if len(envelope.Result) == 0 || string(envelope.Result) == "null" {
		return nil
	}
	r.RawResult = envelope.Result
	r.Result = new(T)
	return DecodeJSON(envelope.Result, r.Result)
}

type TokenScope string

const (
	TokenScopeRead  TokenScope = "read"
	TokenScopeWrite TokenScope = "write"
	TokenScopeAll   TokenScope = "*"
)

type InputContext struct {
	Counterparty string `json:"counterparty,omitempty"`
	Agent        string `json:"agent,omitempty"`
	Source       string `json:"source,omitempty"`
	Topic        string `json:"topic,omitempty"`
}

type MessageRole string

const (
	RoleSystem    MessageRole = "system"
	RoleUser      MessageRole = "user"
	RoleAssistant MessageRole = "assistant"
	RoleTool      MessageRole = "tool"
)

type Message struct {
	Role      MessageRole    `json:"role"`
	Content   MessageContent `json:"content"`
	Name      string         `json:"name,omitempty"`
	User      string         `json:"user,omitempty"`
	Timestamp *int64         `json:"timestamp,omitempty"`
}

type ContentPartType string

const (
	ContentPartText       ContentPartType = "Text"
	ContentPartReasoning  ContentPartType = "Reasoning"
	ContentPartFileData   ContentPartType = "FileData"
	ContentPartInlineData ContentPartType = "InlineData"
	ContentPartToolCall   ContentPartType = "ToolCall"
	ContentPartToolOutput ContentPartType = "ToolOutput"
	ContentPartAction     ContentPartType = "Action"
	ContentPartAny        ContentPartType = "Any"
)

type ContentPart interface {
	contentPartType() ContentPartType
}

var errInvalidContentPart = errors.New("invalid ContentPart")

// Each part encodes its wire type whatever its Type field holds, so a part
// built without Type still names its kind.

type TextPart struct {
	Type ContentPartType `json:"type"`
	Text string          `json:"text"`
}

func (TextPart) contentPartType() ContentPartType { return ContentPartText }

func (p TextPart) MarshalJSON() ([]byte, error) {
	type plain TextPart
	p.Type = ContentPartText
	return json.Marshal(plain(p))
}

type ReasoningPart struct {
	Type ContentPartType `json:"type"`
	Text string          `json:"text"`
}

func (ReasoningPart) contentPartType() ContentPartType { return ContentPartReasoning }

func (p ReasoningPart) MarshalJSON() ([]byte, error) {
	type plain ReasoningPart
	p.Type = ContentPartReasoning
	return json.Marshal(plain(p))
}

type FileDataPart struct {
	Type     ContentPartType `json:"type"`
	FileURI  string          `json:"fileUri"`
	MimeType *string         `json:"mimeType,omitempty"`
}

func (FileDataPart) contentPartType() ContentPartType { return ContentPartFileData }

func (p FileDataPart) MarshalJSON() ([]byte, error) {
	type plain FileDataPart
	p.Type = ContentPartFileData
	return json.Marshal(plain(p))
}

type InlineDataPart struct {
	Type     ContentPartType `json:"type"`
	MimeType string          `json:"mimeType"`
	Data     any             `json:"data"`
}

func (InlineDataPart) contentPartType() ContentPartType { return ContentPartInlineData }

func (p InlineDataPart) MarshalJSON() ([]byte, error) {
	type plain InlineDataPart
	p.Type = ContentPartInlineData
	return json.Marshal(plain(p))
}

type ToolCallPart struct {
	Type   ContentPartType `json:"type"`
	Name   string          `json:"name"`
	Args   any             `json:"args"`
	CallID *string         `json:"callId,omitempty"`
}

func (ToolCallPart) contentPartType() ContentPartType { return ContentPartToolCall }

func (p ToolCallPart) MarshalJSON() ([]byte, error) {
	type plain ToolCallPart
	p.Type = ContentPartToolCall
	return json.Marshal(plain(p))
}

type ToolOutputPart struct {
	Type     ContentPartType `json:"type"`
	Name     string          `json:"name"`
	Output   any             `json:"output"`
	IsError  *bool           `json:"isError,omitempty"`
	CallID   *string         `json:"callId,omitempty"`
	RemoteID *string         `json:"remoteId,omitempty"`
}

func (ToolOutputPart) contentPartType() ContentPartType { return ContentPartToolOutput }

func (p ToolOutputPart) MarshalJSON() ([]byte, error) {
	type plain ToolOutputPart
	p.Type = ContentPartToolOutput
	return json.Marshal(plain(p))
}

type ActionPart struct {
	Type       ContentPartType `json:"type"`
	Name       string          `json:"name"`
	Payload    any             `json:"payload"`
	Recipients []string        `json:"recipients,omitempty"`
	Signature  *string         `json:"signature,omitempty"`
}

func (ActionPart) contentPartType() ContentPartType { return ContentPartAction }

func (p ActionPart) MarshalJSON() ([]byte, error) {
	type plain ActionPart
	p.Type = ContentPartAction
	return json.Marshal(plain(p))
}

type AnyPart struct {
	Raw json.RawMessage
}

func (AnyPart) contentPartType() ContentPartType { return ContentPartAny }

func (p AnyPart) MarshalJSON() ([]byte, error) {
	if len(p.Raw) > 0 {
		return p.Raw, nil
	}
	return json.Marshal(map[string]any{"type": string(ContentPartAny)})
}

type MessageContent []ContentPart

func NewTextContentPart(text string) ContentPart {
	return TextPart{Type: ContentPartText, Text: text}
}

func MessageContentFromText(text string) MessageContent {
	return MessageContent{NewTextContentPart(text)}
}

// parseContentPart decodes a known part, which must carry its required
// members, and keeps anything else verbatim as an AnyPart.
func parseContentPart(raw json.RawMessage) (ContentPart, error) {
	trimmed := bytes.TrimSpace(raw)
	if len(trimmed) > 0 && trimmed[0] == '"' {
		var text string
		if err := json.Unmarshal(trimmed, &text); err != nil {
			return nil, err
		}
		return NewTextContentPart(text), nil
	}

	var fields map[string]json.RawMessage
	var partType ContentPartType
	if len(trimmed) == 0 || trimmed[0] != '{' ||
		json.Unmarshal(trimmed, &fields) != nil || json.Unmarshal(fields["type"], &partType) != nil {
		return AnyPart{Raw: append(json.RawMessage(nil), trimmed...)}, nil
	}

	switch partType {
	case ContentPartText:
		return decodeContentPart[TextPart](trimmed, fields, "text")
	case ContentPartReasoning:
		return decodeContentPart[ReasoningPart](trimmed, fields, "text")
	case ContentPartFileData:
		return decodeContentPart[FileDataPart](trimmed, fields, "fileUri")
	case ContentPartInlineData:
		return decodeContentPart[InlineDataPart](trimmed, fields, "mimeType", "data")
	case ContentPartToolCall:
		return decodeContentPart[ToolCallPart](trimmed, fields, "name", "args")
	case ContentPartToolOutput:
		return decodeContentPart[ToolOutputPart](trimmed, fields, "name", "output")
	case ContentPartAction:
		return decodeContentPart[ActionPart](trimmed, fields, "name", "payload")
	default:
		return AnyPart{Raw: append(json.RawMessage(nil), trimmed...)}, nil
	}
}

func decodeContentPart[P ContentPart](raw []byte, fields map[string]json.RawMessage, required ...string) (ContentPart, error) {
	for _, name := range required {
		if _, ok := fields[name]; !ok {
			return nil, errInvalidContentPart
		}
	}
	var part P
	if err := DecodeJSON(raw, &part); err != nil {
		return nil, errInvalidContentPart
	}
	return part, nil
}

func (c *MessageContent) UnmarshalJSON(data []byte) error {
	trimmed := bytes.TrimSpace(data)
	if len(trimmed) == 0 {
		*c = MessageContent{}
		return nil
	}

	if trimmed[0] == '"' {
		var text string
		if err := json.Unmarshal(trimmed, &text); err != nil {
			return err
		}
		*c = MessageContent{NewTextContentPart(text)}
		return nil
	}

	if trimmed[0] == '[' {
		var rawItems []json.RawMessage
		if err := json.Unmarshal(trimmed, &rawItems); err != nil {
			return err
		}
		items := make([]ContentPart, 0, len(rawItems))
		for _, raw := range rawItems {
			part, err := parseContentPart(raw)
			if err != nil {
				return err
			}
			items = append(items, part)
		}
		*c = MessageContent(items)
		return nil
	}

	return fmt.Errorf("message content must be a string or array")
}

func (c MessageContent) MarshalJSON() ([]byte, error) {
	if c == nil {
		return []byte("[]"), nil
	}
	return json.Marshal([]ContentPart(c))
}

type FormationInput struct {
	Messages []Message     `json:"messages"`
	Context  *InputContext `json:"context,omitempty"`
	// Timestamp is when the conversation happened, in millisecond UTC. Formed
	// claims take it as their asserted_at; omit it to use the receipt time.
	Timestamp string `json:"timestamp,omitempty"`
}

type RecallInput struct {
	Query   string        `json:"query"`
	Context *InputContext `json:"context,omitempty"`
	Budget  *RecallBudget `json:"budget,omitempty"`
}

type MaintenanceParameters struct {
	StaleEventThresholdDays  *int `json:"stale_event_threshold_days,omitempty"`
	UnconsolidatedMaxBacklog *int `json:"unconsolidated_max_backlog,omitempty"`
	OrphanMaxCount           *int `json:"orphan_max_count,omitempty"`
}

type MaintenanceInput struct {
	Trigger    string                 `json:"trigger,omitempty"`
	Scope      string                 `json:"scope,omitempty"`
	Timestamp  string                 `json:"timestamp"`
	Parameters *MaintenanceParameters `json:"parameters,omitempty"`
}

type AddSpaceTokenInput struct {
	Scope     TokenScope `json:"scope"`
	Name      string     `json:"name"`
	ExpiresAt *int64     `json:"expires_at,omitempty"`
	// Labels restricts the token to wiki content carrying these ACL labels
	// (plus unlabeled content). Nil = unrestricted. The server only accepts
	// labels on read-scoped tokens.
	Labels *[]string `json:"labels,omitempty"`
}

type RevokeSpaceTokenInput struct {
	// Token is the full token value to revoke. Leave empty when revoking by Name.
	Token string `json:"token,omitempty"`
	// Name revokes by the unique token name instead. This is the recovery path
	// when the full value was not saved at mint time: list_space_tokens only
	// echoes a display prefix.
	Name string `json:"name,omitempty"`
}

type UpdateSpaceInput struct {
	Name        *string `json:"name,omitempty"`
	Description *string `json:"description,omitempty"`
	Public      *bool   `json:"public,omitempty"`
	// WikiDigest enables/disables the WikiDigest background extraction
	// (disabled by default).
	WikiDigest *bool `json:"wiki_digest,omitempty"`
	// WikiAuditReads enables/disables read auditing for external wiki reads
	// (disabled by default).
	WikiAuditReads *bool `json:"wiki_audit_reads,omitempty"`
	// WikiACLDefaults maps namespace -> default ACL label for newly created
	// wiki documents. When present it replaces the whole map (a pointer to an
	// empty map clears all defaults; nil leaves the map unchanged).
	WikiACLDefaults *map[string]string `json:"wiki_acl_defaults,omitempty"`
	MemoryPolicy    *MemoryPolicy      `json:"memory_policy,omitempty"`
}

type ModelConfig struct {
	Family        string `json:"family"` // "gemini", "anthropic", "openai", "deepseek", "mimo" etc.
	Model         string `json:"model"`
	APIBase       string `json:"api_base"`
	APIKey        string `json:"api_key"`
	Disabled      *bool  `json:"disabled,omitempty"`
	Label         string `json:"label,omitempty"`
	Effort        string `json:"effort,omitempty"` // minimal, low, medium, high, max
	BearerAuth    bool   `json:"bearer_auth,omitempty"`
	Stream        bool   `json:"stream,omitempty"`
	ContextWindow int    `json:"context_window,omitempty"`
	MaxOutput     int    `json:"max_output,omitempty"`
}

type RestartFormationInput struct {
	Conversation *uint64 `json:"conversation,omitempty"`
}

type CreateOrUpdateSpaceInput struct {
	User    string `json:"user"`
	SpaceID string `json:"space_id"`
	Tier    int    `json:"tier"`
}

type GetOrInitUserInput struct {
	User string  `json:"user"`
	Name *string `json:"name,omitempty"`
}

type Concept struct {
	ID          string                    `json:"id,omitempty"`
	Kind        string                    `json:"kind,omitempty"`
	SpaceID     string                    `json:"space_id,omitempty"`
	SchemaRef   string                    `json:"schema_ref,omitempty"`
	Key         string                    `json:"key,omitempty"`
	Name        string                    `json:"name,omitempty"`
	CanonicalID string                    `json:"canonical_id,omitempty"`
	Aliases     []string                  `json:"aliases,omitempty"`
	Attributes  map[string]any            `json:"attributes,omitempty"`
	Facets      map[string]map[string]any `json:"facets,omitempty"`
	Retention   map[string]any            `json:"retention,omitempty"`
	System      map[string]any            `json:"_system,omitempty"`
	// Deprecated KIP 1.x fields retained for callers decoding older data.
	Type     string                     `json:"type,omitempty"`
	Metadata map[string]any             `json:"metadata,omitempty"`
	Extra    map[string]json.RawMessage `json:"-"`
}

// UnmarshalJSON preserves envelope fields added by newer KIP versions.
func (c *Concept) UnmarshalJSON(data []byte) error {
	type known Concept
	var value known
	if err := DecodeJSON(data, &value); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	for _, key := range []string{"id", "kind", "space_id", "schema_ref", "key", "name", "canonical_id", "aliases", "attributes", "facets", "retention", "_system", "type", "metadata"} {
		delete(fields, key)
	}
	*c = Concept(value)
	c.Extra = fields
	return nil
}

func (c Concept) MarshalJSON() ([]byte, error) {
	type known Concept
	data, err := json.Marshal(known(c))
	if err != nil {
		return nil, err
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	for key, value := range c.Extra {
		if _, known := fields[key]; !known {
			fields[key] = value
		}
	}
	return json.Marshal(fields)
}

type SpaceTier struct {
	Tier      int   `json:"tier"`
	UpdatedAt int64 `json:"updated_at"`
}

type SpaceToken struct {
	Name      string     `json:"name"`
	Token     string     `json:"token"`
	Scope     TokenScope `json:"scope"`
	Usage     int        `json:"usage"`
	CreatedAt int64      `json:"created_at"`
	UpdatedAt int64      `json:"updated_at"`
	ExpiresAt *int64     `json:"expires_at,omitempty"`
	// Labels are the wiki ACL labels this token may read (nil = unrestricted).
	Labels *[]string `json:"labels,omitempty"`
}

type StorageStats map[string]any

type SpaceInfo struct {
	ID                     string        `json:"id"`
	Name                   string        `json:"name,omitempty"`
	Description            string        `json:"description,omitempty"`
	Owner                  string        `json:"owner"`
	DBStats                StorageStats  `json:"db_stats"`
	Concepts               int           `json:"concepts"`
	Propositions           int           `json:"propositions"`
	Conversations          int           `json:"conversations"`
	Public                 bool          `json:"public"`
	Tier                   SpaceTier     `json:"tier"`
	FormationUsage         Usage         `json:"formation_usage"`
	RecallUsage            Usage         `json:"recall_usage"`
	MaintenanceUsage       Usage         `json:"maintenance_usage"`
	FormationProcessedID   int64         `json:"formation_processed_id"`
	MaintenanceProcessedID int64         `json:"maintenance_processed_id"`
	MaintenanceAt          MaintenanceAt `json:"maintenance_at"`
	// MemoryInterface is the Memory Interface descriptor this Space serves.
	MemoryInterface json.RawMessage `json:"memory_interface,omitempty"`
	WikiDocs        *int            `json:"wiki_docs,omitempty"`
	WikiChunks      *int            `json:"wiki_chunks,omitempty"`
	WikiVersions    *int            `json:"wiki_versions,omitempty"`
	WikiQueries     *uint64         `json:"wiki_queries,omitempty"`
	WikiDigested    *uint64         `json:"wiki_digested,omitempty"`
	WikiStaleDocs   *uint64         `json:"wiki_stale_docs,omitempty"`
}

type FormationStatus struct {
	ID                     string        `json:"id"`
	Concepts               int           `json:"concepts"`
	Propositions           int           `json:"propositions"`
	Conversations          int           `json:"conversations"`
	FormationProcessing    bool          `json:"formation_processing"`
	MaintenanceProcessing  bool          `json:"maintenance_processing"`
	FormationProcessedID   int64         `json:"formation_processed_id"`
	MaintenanceProcessedID int64         `json:"maintenance_processed_id"`
	MaintenanceAt          MaintenanceAt `json:"maintenance_at"`
}

type MaintenanceAt struct {
	Daydream int64 `json:"daydream"`
	Full     int64 `json:"full"`
	Quick    int64 `json:"quick"`
	// Start time of the latest maintenance task in unix milliseconds, 0 if none started.
	StartAt int64 `json:"start_at"`
}

type Usage struct {
	InputTokens  uint64 `json:"input_tokens"`
	OutputTokens uint64 `json:"output_tokens"`
	CachedTokens uint64 `json:"cached_tokens"`
	Requests     uint64 `json:"requests"`
}

type AgentOutput struct {
	Thoughts     *string           `json:"thoughts,omitempty"`
	ToolsUsage   map[string]Usage  `json:"tools_usage,omitempty"`
	ToolCalls    []json.RawMessage `json:"tool_calls,omitempty"`
	ChatHistory  []Message         `json:"chat_history,omitempty"`
	Artifacts    []json.RawMessage `json:"artifacts,omitempty"`
	Session      *string           `json:"session,omitempty"`
	Content      string            `json:"content"`
	Conversation *uint64           `json:"conversation,omitempty"`
	FailedReason string            `json:"failed_reason,omitempty"`
	Usage        *Usage            `json:"usage,omitempty"`
	Model        string            `json:"model,omitempty"`
}

type ConversationStatus string

const (
	StatusSubmitted ConversationStatus = "submitted"
	StatusWorking   ConversationStatus = "working"
	StatusIdle      ConversationStatus = "idle"
	StatusCompleted ConversationStatus = "completed"
	StatusFailed    ConversationStatus = "failed"
	StatusCancelled ConversationStatus = "cancelled"
)

type Conversation struct {
	ID               uint64             `json:"_id"`
	User             string             `json:"user"`
	Label            *string            `json:"label,omitempty"`
	Thread           string             `json:"thread,omitempty"`
	Messages         []Message          `json:"messages"`
	Resources        []any              `json:"resources"`
	Artifacts        []any              `json:"artifacts"`
	Status           ConversationStatus `json:"status"`
	FailedReason     *string            `json:"failed_reason,omitempty"`
	Period           int                `json:"period"`
	CreatedAt        int64              `json:"created_at"`
	UpdatedAt        int64              `json:"updated_at"`
	Usage            Usage              `json:"usage"`
	SteeringMessages []string           `json:"steering_messages,omitempty"`
	FollowUpMessages []string           `json:"follow_up_messages,omitempty"`
	Child            *uint64            `json:"child,omitempty"`
	Extra            json.RawMessage    `json:"extra,omitempty"`
	Ancestors        []uint64           `json:"ancestors,omitempty"`
}

type ConversationDelta struct {
	ID           uint64             `json:"_id"`
	Messages     []json.RawMessage  `json:"messages"`
	Artifacts    []any              `json:"artifacts"`
	Status       ConversationStatus `json:"status"`
	Usage        Usage              `json:"usage"`
	FailedReason *string            `json:"failed_reason,omitempty"`
	UpdatedAt    int64              `json:"updated_at"`
	Child        *uint64            `json:"child,omitempty"`
}

type ServiceInfo struct {
	Name     string `json:"name"`
	Version  string `json:"version"`
	Sharding int    `json:"sharding"`
	// MemoryInterface is the Memory Interface descriptor template every Space
	// served here speaks.
	MemoryInterface json.RawMessage `json:"memory_interface,omitempty"`
	Description     string          `json:"description"`
}

type KipOperationObject struct {
	OpID           string          `json:"op_id,omitempty"`
	Language       string          `json:"language,omitempty"`
	Command        string          `json:"command,omitempty"`
	AST            json.RawMessage `json:"ast,omitempty"`
	Parameters     map[string]any  `json:"parameters,omitempty"`
	IdempotencyKey string          `json:"idempotency_key,omitempty"`
	Options        json.RawMessage `json:"options,omitempty"`
	Extensions     map[string]any  `json:"extensions,omitempty"`
}

type KipOperation struct {
	String *string
	Object *KipOperationObject
}

func (item *KipOperation) UnmarshalJSON(data []byte) error {
	trimmed := bytes.TrimSpace(data)
	if len(trimmed) == 0 {
		return fmt.Errorf("kip command item cannot be empty")
	}

	if trimmed[0] == '"' {
		var command string
		if err := json.Unmarshal(trimmed, &command); err != nil {
			return fmt.Errorf("invalid kip command string: %w", err)
		}
		command = strings.TrimSpace(command)
		if command == "" {
			return fmt.Errorf("kip command string cannot be empty")
		}
		item.String = &command
		item.Object = nil
		return nil
	}

	if trimmed[0] == '{' {
		var commandObject KipOperationObject
		decoder := json.NewDecoder(bytes.NewReader(trimmed))
		decoder.DisallowUnknownFields()
		decoder.UseNumber()
		if err := decoder.Decode(&commandObject); err != nil {
			return fmt.Errorf("invalid kip command object: %w", err)
		}
		commandObject.Command = strings.TrimSpace(commandObject.Command)
		if commandObject.Command == "" && len(commandObject.AST) == 0 {
			return fmt.Errorf("kip operation object requires command or ast")
		}
		item.Object = &commandObject
		item.String = nil
		return nil
	}

	return fmt.Errorf("kip command item must be string or object")
}

func (item KipOperation) MarshalJSON() ([]byte, error) {
	if item.String != nil {
		return json.Marshal(*item.String)
	}
	if item.Object != nil {
		return json.Marshal(item.Object)
	}
	return nil, fmt.Errorf("invalid kip command item")
}

type KipRequest struct {
	// Command and Operations are mutually exclusive. Batches require Execution.
	Command    string         `json:"command,omitempty"`
	Operations []KipOperation `json:"operations,omitempty"`
	Execution  *KipExecution  `json:"execution,omitempty"`
	Read       *KipRead       `json:"read,omitempty"`
	Parameters map[string]any `json:"parameters,omitempty"`
	DryRun     bool           `json:"dry_run,omitempty"`
}

type KipExecution struct {
	Mode           string         `json:"mode"` // independent, sequence, atomic
	OnError        string         `json:"on_error,omitempty"`
	Isolation      string         `json:"isolation,omitempty"`
	IdempotencyKey string         `json:"idempotency_key,omitempty"`
	Extensions     map[string]any `json:"extensions,omitempty"`
}

type KipRead struct {
	SnapshotToken string         `json:"snapshot_token,omitempty"`
	Extensions    map[string]any `json:"extensions,omitempty"`
}

type KipError struct {
	Code     string `json:"code"`
	Message  string `json:"message"`
	Category string `json:"category,omitempty"`
	Hint     string `json:"hint,omitempty"`
	Retry    any    `json:"retry,omitempty"`
	Details  any    `json:"details,omitempty"`
}

func (e *KipError) Error() string {
	if e == nil {
		return ""
	}
	if e.Code != "" {
		return fmt.Sprintf("%s: %s", e.Code, e.Message)
	}
	return e.Message
}

type KipOperationResult[T any] struct {
	OpID       string          `json:"op_id,omitempty"`
	Status     string          `json:"status"`
	Result     *T              `json:"result,omitempty"`
	Context    json.RawMessage `json:"context,omitempty"`
	Error      *KipError       `json:"error,omitempty"`
	Warnings   []any           `json:"warnings,omitempty"`
	NextCursor string          `json:"next_cursor,omitempty"`
	Receipt    json.RawMessage `json:"receipt,omitempty"`
	Extensions json.RawMessage `json:"extensions,omitempty"`
}

type KipResponse[T any] struct {
	Kip        string                  `json:"kip"`
	RequestID  string                  `json:"request_id,omitempty"`
	Status     string                  `json:"status"`
	Results    []KipOperationResult[T] `json:"results"`
	Execution  json.RawMessage         `json:"execution,omitempty"`
	Context    json.RawMessage         `json:"context,omitempty"`
	Snapshot   json.RawMessage         `json:"snapshot,omitempty"`
	Receipt    json.RawMessage         `json:"receipt,omitempty"`
	Warnings   []any                   `json:"warnings,omitempty"`
	NextCursor string                  `json:"next_cursor,omitempty"`
	Error      *KipError               `json:"error,omitempty"`
	Extensions json.RawMessage         `json:"extensions,omitempty"`
}

// Failure checks both KIP error levels; a 200 HTTP response can still fail.
func (r *KipResponse[T]) Failure() error {
	if r == nil {
		return fmt.Errorf("empty KIP response")
	}
	if r.Error != nil {
		return r.Error
	}
	for _, result := range r.Results {
		if result.Error != nil {
			return result.Error
		}
	}
	if r.Status != "succeeded" {
		return fmt.Errorf("KIP status: %s", r.Status)
	}
	if len(r.Results) == 0 {
		return fmt.Errorf("successful KIP response has no operation results")
	}
	for i, result := range r.Results {
		if result.Status != "succeeded" && result.Status != "no_effect" {
			return fmt.Errorf("KIP operation %d has unconfirmed status %q", i+1, result.Status)
		}
	}
	return nil
}
