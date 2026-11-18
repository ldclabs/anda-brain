package api

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

const (
	DefaultBaseURL = "http://127.0.0.1:8042"
	DefaultTimeout = 120 * time.Second
)

// maxErrorBody bounds a non-JSON error body, such as a proxy's HTML page, in
// an error message.
const maxErrorBody = 1024

// Client is the HTTP client for the Anda Brain API.
type Client struct {
	BaseURL string
	SpaceID string
	Token   string
	// Shard is sent as the "Shard-Id" header when > 0, for sharded deployments.
	Shard      int
	HTTPClient *http.Client
}

// NewClient creates a new API client.
func NewClient(baseURL, spaceID, token string) *Client {
	baseURL = strings.TrimRight(baseURL, "/")
	return &Client{
		BaseURL: baseURL,
		SpaceID: spaceID,
		Token:   token,
		HTTPClient: &http.Client{
			Timeout: DefaultTimeout,
		},
	}
}

func (c *Client) spacePath(path string) string {
	return fmt.Sprintf("/v1/%s%s", url.PathEscape(c.SpaceID), path)
}

func (c *Client) doJSON(ctx context.Context, method, path string, body any) ([]byte, error) {
	var reqBody io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return nil, fmt.Errorf("marshal request: %w", err)
		}
		reqBody = bytes.NewReader(data)
	}

	reqURL := c.BaseURL + path
	req, err := http.NewRequestWithContext(ctx, method, reqURL, reqBody)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}

	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	req.Header.Set("Accept", "application/json")
	if c.Token != "" {
		req.Header.Set("Authorization", "Bearer "+c.Token)
	}
	if c.Shard > 0 {
		req.Header.Set("Shard-Id", strconv.Itoa(c.Shard))
	}

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("http request: %w", err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("read response: %w", err)
	}

	if resp.StatusCode >= 400 {
		return nil, newHTTPError(resp.StatusCode, respBody)
	}

	return respBody, nil
}

// newHTTPError reads the server's error envelope, {"error":{"message","data"}},
// keeping its structured data (a wiki conflict's current_version, say).
func newHTTPError(status int, body []byte) *HTTPError {
	var envelope struct {
		Error *RpcError `json:"error"`
	}
	if DecodeJSON(body, &envelope) == nil && envelope.Error != nil {
		return &HTTPError{StatusCode: status, RPC: envelope.Error}
	}
	text := strings.TrimSpace(string(body))
	if len(text) > maxErrorBody {
		text = strings.ToValidUTF8(text[:maxErrorBody], "") + "…"
	}
	return &HTTPError{StatusCode: status, Body: text}
}

func callRPC[T any](ctx context.Context, c *Client, method, path string, input any) (*RpcResponse[T], error) {
	data, err := c.doJSON(ctx, method, path, input)
	if err != nil {
		return nil, err
	}
	var response RpcResponse[T]
	if err := DecodeJSON(data, &response); err != nil {
		return nil, fmt.Errorf("decode response: %w", err)
	}
	return &response, nil
}

func withQuery(path string, values url.Values) string {
	if len(values) == 0 {
		return path
	}
	return path + "?" + values.Encode()
}

func setLimit(values url.Values, limit int) {
	if limit > 0 {
		values.Set("limit", strconv.Itoa(limit))
	}
}

// GetInfo returns service information.
func (c *Client) GetInfo(ctx context.Context) (*ServiceInfo, error) {
	data, err := c.doJSON(ctx, http.MethodGet, "/info", nil)
	if err != nil {
		return nil, err
	}
	var info ServiceInfo
	if err := DecodeJSON(data, &info); err != nil {
		return nil, fmt.Errorf("decode response: %w", err)
	}
	return &info, nil
}

// Formation submits a memory formation task.
func (c *Client) Formation(ctx context.Context, input *FormationInput) (*RpcResponse[AgentOutput], error) {
	return callRPC[AgentOutput](ctx, c, http.MethodPost, c.spacePath("/formation"), input)
}

// Recall queries memory with natural language.
func (c *Client) Recall(ctx context.Context, input *RecallInput) (*RpcResponse[AgentOutput], error) {
	return callRPC[AgentOutput](ctx, c, http.MethodPost, c.spacePath("/recall"), input)
}

func (c *Client) RecallStructured(ctx context.Context, input *RecallInput) (*RpcResponse[RecallOutput], error) {
	return callRPC[RecallOutput](ctx, c, http.MethodPost, c.spacePath("/recall_structured"), input)
}

func (c *Client) Probe(ctx context.Context, input *ProbeInput) (*RpcResponse[ProbeOutput], error) {
	return callRPC[ProbeOutput](ctx, c, http.MethodPost, c.spacePath("/probe"), input)
}

// Maintenance triggers maintenance task.
func (c *Client) Maintenance(ctx context.Context, input *MaintenanceInput) (*RpcResponse[AgentOutput], error) {
	return callRPC[AgentOutput](ctx, c, http.MethodPost, c.spacePath("/maintenance"), input)
}

// ExecuteKIPReadonly executes a KIP request in read-only mode.
func (c *Client) ExecuteKIPReadonly(ctx context.Context, input *KipRequest) (*KipResponse[any], error) {
	data, err := c.doJSON(ctx, http.MethodPost, c.spacePath("/execute_kip_readonly"), input)
	if err != nil {
		return nil, err
	}
	var resp KipResponse[any]
	if err := DecodeJSON(data, &resp); err != nil {
		return nil, fmt.Errorf("decode response: %w", err)
	}
	if resp.Status == "succeeded" && input != nil {
		expected := max(1, len(input.Operations))
		if len(resp.Results) != expected {
			return &resp, fmt.Errorf("KIP returned %d results for %d operations", len(resp.Results), expected)
		}
	}
	return &resp, nil
}

// GetOrInitUser gets or initializes a caller concept.
func (c *Client) GetOrInitUser(ctx context.Context, input *GetOrInitUserInput) (*RpcResponse[Concept], error) {
	return callRPC[Concept](ctx, c, http.MethodPost, c.spacePath("/get_or_init_user"), input)
}

// GetSpaceInfo returns space information.
func (c *Client) GetSpaceInfo(ctx context.Context) (*RpcResponse[SpaceInfo], error) {
	return callRPC[SpaceInfo](ctx, c, http.MethodGet, c.spacePath("/info"), nil)
}

func (c *Client) GetFormationStatus(ctx context.Context) (*RpcResponse[FormationStatus], error) {
	return callRPC[FormationStatus](ctx, c, http.MethodGet, c.spacePath("/formation_status"), nil)
}

func (c *Client) GetMemoryStatus(ctx context.Context) (*RpcResponse[MemoryStatus], error) {
	return callRPC[MemoryStatus](ctx, c, http.MethodGet, c.spacePath("/memory_status"), nil)
}

func (c *Client) PinMemory(ctx context.Context, input *MemoryPinInput) (*RpcResponse[MemoryPinOutput], error) {
	return callRPC[MemoryPinOutput](ctx, c, http.MethodPost, c.spacePath("/memory/pin"), input)
}

func (c *Client) ForgetMemory(ctx context.Context, input *MemoryForgetInput) (*RpcResponse[MemoryForgetReport], error) {
	return callRPC[MemoryForgetReport](ctx, c, http.MethodPost, c.spacePath("/memory/forget"), input)
}

// GetConversation returns a single conversation.
func (c *Client) GetConversation(ctx context.Context, conversationID uint64, collection string) (*RpcResponse[Conversation], error) {
	values := url.Values{}
	if collection != "" {
		values.Set("collection", collection)
	}
	path := c.spacePath(fmt.Sprintf("/conversations/%d", conversationID))
	return callRPC[Conversation](ctx, c, http.MethodGet, withQuery(path, values), nil)
}

// GetConversationDelta returns incremental conversation updates since the given offsets.
func (c *Client) GetConversationDelta(ctx context.Context, conversationID uint64, messagesOffset, artifactsOffset int, collection string) (*RpcResponse[ConversationDelta], error) {
	values := url.Values{}
	if messagesOffset > 0 {
		values.Set("messages_offset", strconv.Itoa(messagesOffset))
	}
	if artifactsOffset > 0 {
		values.Set("artifacts_offset", strconv.Itoa(artifactsOffset))
	}
	if collection != "" {
		values.Set("collection", collection)
	}
	path := c.spacePath(fmt.Sprintf("/conversations/%d/delta", conversationID))
	return callRPC[ConversationDelta](ctx, c, http.MethodGet, withQuery(path, values), nil)
}

// ListConversations lists conversations with pagination.
func (c *Client) ListConversations(ctx context.Context, cursor string, limit int, collection string) (*RpcResponse[[]Conversation], error) {
	values := url.Values{}
	if cursor != "" {
		values.Set("cursor", cursor)
	}
	setLimit(values, limit)
	if collection != "" {
		values.Set("collection", collection)
	}
	return callRPC[[]Conversation](ctx, c, http.MethodGet, withQuery(c.spacePath("/conversations"), values), nil)
}

// SchemaDrafts lists the Space's draft vocabulary (KIP §20.16).
func (c *Client) SchemaDrafts(ctx context.Context) (*RpcResponse[SchemaDrafts], error) {
	return callRPC[SchemaDrafts](ctx, c, http.MethodGet, c.spacePath("/schema/drafts"), nil)
}

// PromoteDraftSymbol promotes one draft symbol onto an installed symbol of the
// same kind. It is a Schema migration and needs the Space's management token.
func (c *Client) PromoteDraftSymbol(ctx context.Context, input *PromoteDraftInput) (*RpcResponse[PromoteDraftOutput], error) {
	return callRPC[PromoteDraftOutput](ctx, c, http.MethodPost, c.spacePath("/schema/promote"), input)
}

// ListSpaceTokens lists space tokens (management).
func (c *Client) ListSpaceTokens(ctx context.Context) (*RpcResponse[[]SpaceToken], error) {
	return callRPC[[]SpaceToken](ctx, c, http.MethodGet, c.spacePath("/management/space_tokens"), nil)
}

// AddSpaceToken adds a space token (management).
func (c *Client) AddSpaceToken(ctx context.Context, input *AddSpaceTokenInput) (*RpcResponse[SpaceToken], error) {
	return callRPC[SpaceToken](ctx, c, http.MethodPost, c.spacePath("/management/add_space_token"), input)
}

// RevokeSpaceToken revokes a space token by its full value (management).
func (c *Client) RevokeSpaceToken(ctx context.Context, token string) (*RpcResponse[bool], error) {
	return c.revokeSpaceToken(ctx, RevokeSpaceTokenInput{Token: token})
}

// RevokeSpaceTokenByName revokes a space token by its unique name (management).
// This is the recovery path when the full token value was not saved at mint
// time: ListSpaceTokens only echoes a display prefix.
func (c *Client) RevokeSpaceTokenByName(ctx context.Context, name string) (*RpcResponse[bool], error) {
	return c.revokeSpaceToken(ctx, RevokeSpaceTokenInput{Name: name})
}

func (c *Client) revokeSpaceToken(ctx context.Context, input RevokeSpaceTokenInput) (*RpcResponse[bool], error) {
	return callRPC[bool](ctx, c, http.MethodPost, c.spacePath("/management/revoke_space_token"), input)
}

// UpdateSpace updates space information (management).
func (c *Client) UpdateSpace(ctx context.Context, input *UpdateSpaceInput) (*RpcResponse[bool], error) {
	return callRPC[bool](ctx, c, http.MethodPatch, c.spacePath("/management/update_space"), input)
}

func (c *Client) RestartFormation(ctx context.Context, input *RestartFormationInput) (*RpcResponse[bool], error) {
	return callRPC[bool](ctx, c, http.MethodPatch, c.spacePath("/management/restart_formation"), input)
}

func (c *Client) GetBYOK(ctx context.Context) (*RpcResponse[ModelConfig], error) {
	return callRPC[ModelConfig](ctx, c, http.MethodGet, c.spacePath("/management/space_byok"), nil)
}

func (c *Client) UpdateBYOK(ctx context.Context, input *ModelConfig) (*RpcResponse[bool], error) {
	return callRPC[bool](ctx, c, http.MethodPatch, c.spacePath("/management/space_byok"), input)
}

func (c *Client) ShadowEval(ctx context.Context, input *ShadowEvalInput) (*RpcResponse[ShadowReport], error) {
	return callRPC[ShadowReport](ctx, c, http.MethodPost, c.spacePath("/management/shadow_eval"), input)
}

// CreateSpace creates a space (admin).
func (c *Client) CreateSpace(ctx context.Context, input *CreateOrUpdateSpaceInput) (*RpcResponse[SpaceInfo], error) {
	return callRPC[SpaceInfo](ctx, c, http.MethodPost, "/admin/create_space", input)
}

// UpdateSpaceTier updates space tier (admin).
func (c *Client) UpdateSpaceTier(ctx context.Context, spaceID string, input *CreateOrUpdateSpaceInput) (*RpcResponse[SpaceTier], error) {
	path := fmt.Sprintf("/admin/%s/update_space_tier", url.PathEscape(spaceID))
	return callRPC[SpaceTier](ctx, c, http.MethodPost, path, input)
}

func (c *Client) WikiCommit(ctx context.Context, input *WikiCommitInput) (*RpcResponse[WikiCommitOutput], error) {
	return callRPC[WikiCommitOutput](ctx, c, http.MethodPost, c.spacePath("/wiki/docs"), input)
}

func (c *Client) WikiListDocs(ctx context.Context, query WikiListDocsQuery) (*RpcResponse[[]WikiDocInfo], error) {
	values := url.Values{}
	if query.Namespace != "" {
		values.Set("namespace", query.Namespace)
	}
	if query.Status != "" {
		values.Set("status", query.Status)
	}
	if query.Tag != "" {
		values.Set("tag", query.Tag)
	}
	if query.Cursor != "" {
		values.Set("cursor", query.Cursor)
	}
	setLimit(values, query.Limit)
	return callRPC[[]WikiDocInfo](ctx, c, http.MethodGet, withQuery(c.spacePath("/wiki/docs"), values), nil)
}

func (c *Client) WikiGetDoc(ctx context.Context, docID uint64) (*RpcResponse[WikiDocOutput], error) {
	return callRPC[WikiDocOutput](ctx, c, http.MethodGet, c.spacePath(fmt.Sprintf("/wiki/docs/%d", docID)), nil)
}

func (c *Client) WikiRead(ctx context.Context, docID uint64, query WikiReadQuery) (*RpcResponse[WikiReadOutput], error) {
	values := url.Values{}
	if query.Version != nil {
		values.Set("version", strconv.FormatUint(*query.Version, 10))
	}
	if query.Anchor != "" {
		values.Set("anchor", query.Anchor)
	}
	if query.Start != nil {
		values.Set("start", strconv.FormatUint(*query.Start, 10))
	}
	if query.End != nil {
		values.Set("end", strconv.FormatUint(*query.End, 10))
	}
	path := c.spacePath(fmt.Sprintf("/wiki/docs/%d/content", docID))
	return callRPC[WikiReadOutput](ctx, c, http.MethodGet, withQuery(path, values), nil)
}

func (c *Client) WikiVersions(ctx context.Context, docID uint64, cursor string, limit int) (*RpcResponse[[]WikiVersionInfo], error) {
	values := url.Values{}
	if cursor != "" {
		values.Set("cursor", cursor)
	}
	setLimit(values, limit)
	path := c.spacePath(fmt.Sprintf("/wiki/docs/%d/versions", docID))
	return callRPC[[]WikiVersionInfo](ctx, c, http.MethodGet, withQuery(path, values), nil)
}

func (c *Client) wikiSetArchived(ctx context.Context, docID uint64, archive bool) (*RpcResponse[WikiDocInfo], error) {
	action := "restore"
	if archive {
		action = "archive"
	}
	path := c.spacePath(fmt.Sprintf("/wiki/docs/%d/%s", docID, action))
	return callRPC[WikiDocInfo](ctx, c, http.MethodPost, path, nil)
}

func (c *Client) WikiArchive(ctx context.Context, docID uint64) (*RpcResponse[WikiDocInfo], error) {
	return c.wikiSetArchived(ctx, docID, true)
}

func (c *Client) WikiRestore(ctx context.Context, docID uint64) (*RpcResponse[WikiDocInfo], error) {
	return c.wikiSetArchived(ctx, docID, false)
}

func (c *Client) WikiSearch(ctx context.Context, input *WikiSearchInput) (*RpcResponse[WikiSearchOutput], error) {
	return callRPC[WikiSearchOutput](ctx, c, http.MethodPost, c.spacePath("/wiki/search"), input)
}

func (c *Client) WikiVerify(ctx context.Context, input *WikiVerifyInput) (*RpcResponse[WikiVerifyOutput], error) {
	return callRPC[WikiVerifyOutput](ctx, c, http.MethodPost, c.spacePath("/wiki/verify"), input)
}

func (c *Client) WikiEvents(ctx context.Context, query WikiEventsQuery) (*RpcResponse[[]WikiEventInfo], error) {
	values := url.Values{}
	if query.Kind != "" {
		values.Set("kind", query.Kind)
	}
	if query.DocID != nil {
		values.Set("doc_id", strconv.FormatUint(*query.DocID, 10))
	}
	if query.Cursor != "" {
		values.Set("cursor", query.Cursor)
	}
	setLimit(values, query.Limit)
	return callRPC[[]WikiEventInfo](ctx, c, http.MethodGet, withQuery(c.spacePath("/wiki/events"), values), nil)
}

func (c *Client) WikiImport(ctx context.Context, input *WikiImportInput) (*RpcResponse[WikiImportOutput], error) {
	return callRPC[WikiImportOutput](ctx, c, http.MethodPost, c.spacePath("/wiki/import"), input)
}

func (c *Client) WikiExport(ctx context.Context, namespace string) (*RpcResponse[WikiExportOutput], error) {
	values := url.Values{}
	if namespace != "" {
		values.Set("namespace", namespace)
	}
	return callRPC[WikiExportOutput](ctx, c, http.MethodGet, withQuery(c.spacePath("/wiki/export"), values), nil)
}

func (c *Client) WikiDigest(ctx context.Context) (*RpcResponse[WikiDigestReport], error) {
	return callRPC[WikiDigestReport](ctx, c, http.MethodPost, c.spacePath("/wiki/digest"), nil)
}
