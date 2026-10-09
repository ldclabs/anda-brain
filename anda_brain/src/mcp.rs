use anda_core::{
    BoxError, Principal,
    model::{ContentPart, Message},
};
use anda_engine::unix_ms;
use axum::extract::OriginalUri;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, InitializeResult, ServerCapabilities},
    schemars::JsonSchema,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[cfg(feature = "wiki")]
use crate::wiki::{
    WikiCommitInput, WikiError, WikiReadInput, WikiSearchInput, WikiSearchMode, WikiSelector,
    WikiVerifyInput,
};
use crate::{
    agents::SELF_USER_ID,
    authz::{self, AuthzError, AuthzMode, Caller},
    payload::{StringOr, extract_bearer_token, extract_shard_id},
    space::{AppState, Space},
    types::{
        FormationInput, InputContext, MaintenanceInput, MaintenanceParameters, MaintenanceScope,
        RecallInput, TokenScope,
    },
};

#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub space_id: String,
    pub auth_token: Option<String>,
    pub auto_create_space: bool,
    pub auto_create_tier: u32,
    pub dynamic_space_from_path: bool,
    pub remote_path_prefix: String,
}

impl McpServerConfig {
    pub fn stdio(space_id: String, auth_token: Option<String>) -> Self {
        Self {
            space_id,
            auth_token,
            auto_create_space: false,
            auto_create_tier: 1,
            dynamic_space_from_path: false,
            remote_path_prefix: "/mcp".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct McpHttpServerConfig {
    pub path_prefix: String,
    pub auto_create_space: bool,
    pub auto_create_tier: u32,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub stateful_mode: bool,
    pub json_response: bool,
    pub sse_keep_alive_secs: Option<u64>,
}

#[derive(Clone)]
pub struct AndaBrainMcpServer {
    app: AppState,
    config: McpServerConfig,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Clone)]
struct McpAccess {
    space_id: String,
    auth_token: String,
    sharding: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum McpMessageContent {
    Text(String),
    Parts(Vec<Value>),
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpMessage {
    /// Message role: "system", "user", "assistant", or "tool".
    pub role: String,
    /// Message body. Pass a string for plain text, or an array of Anda content parts.
    pub content: McpMessageContent,
    /// Optional participant or tool name.
    pub name: Option<String>,
    /// Optional sender principal ID.
    pub user: Option<String>,
    /// Optional Unix timestamp in milliseconds.
    pub timestamp: Option<u64>,
}

impl McpMessage {
    fn into_message(self) -> Result<Message, ErrorData> {
        let content = match self.content {
            McpMessageContent::Text(text) => vec![ContentPart::Text { text }],
            McpMessageContent::Parts(parts) => parts
                .into_iter()
                .map(|part| serde_json::from_value::<ContentPart>(part).map_err(invalid_params))
                .collect::<Result<Vec<_>, _>>()?,
        };

        let user = match self.user {
            Some(user) => Some(Principal::from_text(&user).map_err(invalid_params)?),
            None => None,
        };

        Ok(Message {
            role: self.role,
            content,
            name: self.name,
            user,
            timestamp: self.timestamp,
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RememberConversationInput {
    pub messages: Vec<McpMessage>,
    pub context: Option<InputContext>,
    /// Optional RFC 3339 observation time. Offsets/fractions are normalized;
    /// an invalid string falls back to the durable conversation receipt time.
    pub timestamp: Option<String>,
}

impl RememberConversationInput {
    fn into_formation_input(self) -> Result<FormationInput, ErrorData> {
        Ok(FormationInput {
            messages: self
                .messages
                .into_iter()
                .map(McpMessage::into_message)
                .collect::<Result<Vec<_>, _>>()?,
            context: self.context,
            timestamp: self.timestamp,
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RecallMemoryInput {
    pub query: String,
    pub context: Option<InputContext>,
    pub budget: Option<crate::recall_budget::RecallBudget>,
}

impl From<RecallMemoryInput> for RecallInput {
    fn from(input: RecallMemoryInput) -> Self {
        Self {
            query: input.query,
            context: input.context,
            budget: input.budget,
        }
    }
}

/// One Memory Interface request (`kip-memory.schema.json#/$defs/Request`).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MemoryInterfaceInput {
    /// The request object: `kip_memory`, `operation`, `input`, and for a
    /// mutation an `idempotency_key`; optional `scope`, `budget`, `requires`.
    pub request: Value,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MemoryReceiptInput {
    /// A `receipt_ref` a Memory Interface mutation returned.
    pub receipt_ref: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RunMaintenanceInput {
    /// Maintenance trigger label. Defaults to "on_demand".
    pub trigger: Option<String>,
    /// Maintenance scope. Defaults to "daydream".
    pub scope: Option<MaintenanceScope>,
    /// Optional canonical UTC timestamp (`YYYY-MM-DDTHH:mm:ss.SSSZ`).
    pub timestamp: Option<String>,
    pub parameters: Option<MaintenanceParameters>,
}

impl From<RunMaintenanceInput> for MaintenanceInput {
    fn from(input: RunMaintenanceInput) -> Self {
        Self {
            trigger: input.trigger.unwrap_or_else(|| "on_demand".to_string()),
            scope: input.scope.unwrap_or_default(),
            timestamp: input.timestamp,
            parameters: input.parameters,
            formation_id: 0,
            // Runtime-filled by `Space::maintenance`, like `formation_id`.
            assessment: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetOrInitUserToolInput {
    /// Stable user principal or external user identifier used by Brain.
    pub user: String,
    /// Optional display name to attach when the user is first created.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RespondAttentionToolInput {
    pub id: String,
    pub response: crate::runtime_api::AttentionResponse,
}

/// One entry of [`ExecuteKipReadonlyInput::commands`].
///
/// A bare string or a command with its own parameter bindings, so a caller
/// batching three reads does not have to spell out three objects to do it.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum McpKipCommandItem {
    Simple(String),
    WithParams {
        command: String,
        #[serde(default)]
        parameters: Map<String, Value>,
    },
}

impl From<McpKipCommandItem> for anda_kip::Operation {
    fn from(command: McpKipCommandItem) -> Self {
        match command {
            McpKipCommandItem::Simple(command) => anda_kip::Operation::new(command),
            McpKipCommandItem::WithParams {
                command,
                parameters,
            } => anda_kip::Operation::new(command).with_parameters(parameters),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExecuteKipReadonlyInput {
    /// A single KIP command. Mutually exclusive with commands.
    pub command: Option<String>,
    /// Batch KIP commands. KQL/META commands are allowed; write commands are rejected.
    #[serde(default)]
    pub commands: Vec<McpKipCommandItem>,
    /// Shared placeholder parameters for command strings.
    #[serde(default)]
    pub parameters: Map<String, Value>,
    /// Validate syntax and logic without executing.
    #[serde(default)]
    pub dry_run: bool,
}

impl ExecuteKipReadonlyInput {
    fn into_request(self) -> Result<anda_kip::Request, ErrorData> {
        let operations: Vec<anda_kip::Operation> = match (self.command, self.commands) {
            (Some(command), commands) if commands.is_empty() => {
                vec![anda_kip::Operation::new(command)]
            }
            (None, commands) if !commands.is_empty() => {
                commands.into_iter().map(Into::into).collect()
            }
            (Some(_), _) => {
                return Err(ErrorData::invalid_params(
                    "pass either command or commands, not both",
                    None,
                ));
            }
            (None, _) => {
                return Err(ErrorData::invalid_params(
                    "pass a command or a commands batch",
                    None,
                ));
            }
        };

        // A KIP 2.0 request with more than one operation must say how they
        // relate; these are reads, so they are independent of each other.
        // There is no `readonly` flag to set: the read-only path decides by
        // what each command parses to, which no envelope field can talk past.
        let execution = (operations.len() > 1)
            .then(|| anda_kip::Execution::new(anda_kip::ExecutionMode::Independent));

        Ok(anda_kip::Request {
            operations,
            execution,
            parameters: (!self.parameters.is_empty()).then_some(self.parameters),
            options: self.dry_run.then(|| anda_kip::RequestOptions {
                dry_run: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListConversationsInput {
    /// Conversation collection: "formation" (default), "recall", or "maintenance".
    pub collection: Option<String>,
    pub cursor: Option<String>,
    /// Page size. Values are clamped to 1..=100.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GetConversationInput {
    pub conversation_id: u64,
    /// Conversation collection: "formation" (default), "recall", or "maintenance".
    pub collection: Option<String>,
    /// Return only incremental messages/artifacts when true.
    pub delta: Option<bool>,
    /// Offset used when delta is true.
    pub messages_offset: Option<usize>,
    /// Offset used when delta is true.
    pub artifacts_offset: Option<usize>,
}

/// MCP clients hand these schemas to their models, and model providers limit
/// schema combinators: Anthropic rejects them at the top level of a tool and
/// strict modes reject them anywhere. schemars derives them for
/// `Option<Struct>` (`anyOf` with null), untagged enums (`anyOf` of distinct
/// types) and tagged enums (`oneOf` of objects), so each tool's schema is
/// flattened once. Serde still checks the arguments when it reads them.
fn flatten_combinators(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            for key in ["anyOf", "oneOf", "allOf"] {
                if let Some(Value::Array(branches)) = map.remove(key) {
                    merge_branches(map, branches);
                }
            }
            map.values_mut().for_each(flatten_combinators);
        }
        Value::Array(items) => items.iter_mut().for_each(flatten_combinators),
        _ => {}
    }
}

fn merge_branches(map: &mut Map<String, Value>, branches: Vec<Value>) {
    // An optional value is left out rather than sent as null.
    let mut branches: Vec<Map<String, Value>> = branches
        .into_iter()
        .filter(|branch| branch.get("type") != Some(&json!("null")))
        .filter_map(|branch| match branch {
            Value::Object(branch) => Some(branch),
            _ => None,
        })
        .collect();
    if branches.len() > 1
        && branches
            .iter()
            .all(|b| b.get("type") == Some(&json!("object")))
    {
        return merge_object_variants(map, branches);
    }
    if branches.len() > 1 {
        // Variants of distinct types: one schema with a type list.
        let types = branches.iter_mut().filter_map(|b| b.remove("type"));
        map.insert("type".into(), Value::Array(types.collect()));
    }
    for (key, value) in branches.into_iter().flatten() {
        map.entry(key).or_insert(value);
    }
}

/// Tagged variants side by side: the tag's values become one enum, only the
/// fields every variant needs stay required, and the description says which
/// other fields go with each tag.
fn merge_object_variants(map: &mut Map<String, Value>, variants: Vec<Map<String, Value>>) {
    let required_of = |variant: &Map<String, Value>| -> Vec<String> {
        let required = variant.get("required").and_then(Value::as_array);
        required
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(String::from))
            .collect()
    };
    let common: Vec<String> = required_of(&variants[0])
        .into_iter()
        .filter(|name| variants.iter().all(|v| required_of(v).contains(name)))
        .collect();
    let mut properties = Map::new();
    let mut notes = Vec::new();
    for variant in &variants {
        let mut tag = None;
        if let Some(Value::Object(fields)) = variant.get("properties") {
            for (name, field) in fields {
                let Some(value) = field.get("const") else {
                    properties
                        .entry(name.clone())
                        .or_insert_with(|| field.clone());
                    continue;
                };
                tag = Some(format!("{name} {value}"));
                let entry = properties.entry(name.clone()).or_insert_with(|| {
                    let mut field = field.clone();
                    if let Some(field) = field.as_object_mut() {
                        field.remove("const");
                        field.insert("enum".into(), json!([]));
                    }
                    field
                });
                if let Some(values) = entry["enum"].as_array_mut() {
                    values.push(value.clone());
                }
            }
        }
        let extra: Vec<String> = required_of(variant)
            .into_iter()
            .filter(|name| !common.contains(name))
            .collect();
        if let Some(tag) = tag {
            let mut note = format!("With {tag}");
            if !extra.is_empty() {
                note.push_str(&format!(", also send {}", extra.join(", ")));
            }
            note.push('.');
            if let Some(description) = variant.get("description").and_then(Value::as_str) {
                note = format!("{note} {description}");
            }
            notes.push(note);
        }
    }
    let closed = variants
        .iter()
        .all(|v| v.get("additionalProperties") == Some(&json!(false)));
    map.insert("type".into(), json!("object"));
    map.insert("properties".into(), Value::Object(properties));
    map.insert("required".into(), json!(common));
    if closed {
        map.insert("additionalProperties".into(), json!(false));
    }
    if !notes.is_empty() {
        let description = map.get("description").and_then(Value::as_str);
        let text = description.into_iter().map(String::from).chain(notes);
        map.insert(
            "description".into(),
            json!(text.collect::<Vec<_>>().join(" ")),
        );
    }
}

impl AndaBrainMcpServer {
    pub fn new(app: AppState, config: McpServerConfig) -> Self {
        let mut tool_router = Self::tool_router();
        #[cfg(feature = "wiki")]
        tool_router.merge(Self::wiki_tool_router());
        for route in tool_router.map.values_mut() {
            let mut schema = Value::Object((*route.attr.input_schema).clone());
            flatten_combinators(&mut schema);
            if let Value::Object(schema) = schema {
                route.attr.input_schema = Arc::new(schema);
            }
        }
        Self {
            app,
            config,
            tool_router,
        }
    }

    /// One permit per synchronous LLM-driving tool call (recall,
    /// maintenance), drawn from the `AppState`'s shared LLM budget — the
    /// same semaphore that caps the HTTP LLM routes, so the MCP channel
    /// cannot bypass that cap. Fails fast like the HTTP 429 load-shed
    /// instead of queueing unbounded work.
    fn acquire_llm_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, ErrorData> {
        self.app.llm_request_semaphore().try_acquire().map_err(|_| {
            ErrorData::invalid_request(
                "too many concurrent model-driven requests, retry later",
                None,
            )
        })
    }

    fn access_from_context(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<McpAccess, ErrorData> {
        if !self.config.dynamic_space_from_path {
            return Ok(McpAccess {
                space_id: self.config.space_id.clone(),
                auth_token: self.config.auth_token.clone().unwrap_or_default(),
                sharding: None,
            });
        }

        let parts = context.extensions.get::<http::request::Parts>().ok_or_else(|| {
            ErrorData::invalid_request(
                "HTTP MCP request context is missing; use the stdio mcp subcommand for local fixed-space mode",
                None,
            )
        })?;
        let path = parts
            .extensions
            .get::<OriginalUri>()
            .map(|uri| uri.path())
            .unwrap_or_else(|| parts.uri.path());

        Ok(McpAccess {
            space_id: space_id_from_mcp_path(path, &self.config.remote_path_prefix)?,
            auth_token: extract_bearer_token(&parts.headers),
            sharding: Some(extract_shard_id(&parts.headers)),
        })
    }

    /// `owner` is the verified caller when one exists (remote auto-create
    /// requires a write CWT); the local stdio mode falls back to
    /// `SELF_USER_ID`. Recording the real principal keeps
    /// `get_space_info.owner` truthful.
    async fn load_configured_space(
        &self,
        space_id: &str,
        owner: Principal,
    ) -> Result<Arc<Space>, ErrorData> {
        match self.app.load_space(space_id, false).await {
            Ok(space) => Ok(space),
            Err(load_err) if self.config.auto_create_space => {
                let create_result = self
                    .app
                    .admin_create_space(
                        SELF_USER_ID,
                        owner,
                        space_id.to_string(),
                        self.config.auto_create_tier,
                        unix_ms(),
                    )
                    .await;

                match create_result {
                    Ok(_) => self
                        .app
                        .load_space(space_id, false)
                        .await
                        .map_err(internal_error),
                    Err(create_err) if create_err.to_string().contains("already exists") => self
                        .app
                        .load_space(space_id, false)
                        .await
                        .map_err(internal_error),
                    Err(create_err) => Err(internal_error(format!(
                        "failed to load space after auto-create fallback: load error: {load_err}; create error: {create_err}"
                    ))),
                }
            }
            Err(err) => Err(internal_error(format!(
                "failed to load memory space '{}': {err}. Create it first or start MCP with --mcp-auto-create-space",
                space_id
            ))),
        }
    }

    async fn load_authorized_space(
        &self,
        scope: TokenScope,
        access: &McpAccess,
    ) -> Result<Arc<Space>, ErrorData> {
        Ok(self
            .load_authorized_space_with_token(scope, access)
            .await?
            .0)
    }

    /// Like [`Self::load_authorized_space`] but also returns the verified
    /// [`Caller`] (CWT / space token) so wiki tools can resolve the caller's
    /// ACL view and audit actor (PRD §8.2): CWT holders are unrestricted,
    /// space tokens carry their labels, and an anonymous public-space reader
    /// is neither — the caller must map that to the unlabeled-only view.
    ///
    /// A thin wrapper over the shared [`authz::authorize`] (read tools get
    /// the public-read fallback, write tools always require a credential);
    /// only auto-create stays MCP-specific: when the space fails to load and
    /// auto-create is enabled, a verified write CWT may create it, then
    /// authorization is retried once against the fresh space.
    async fn load_authorized_space_with_token(
        &self,
        scope: TokenScope,
        access: &McpAccess,
    ) -> Result<(Arc<Space>, Caller), ErrorData> {
        let mode = if scope == TokenScope::Read {
            AuthzMode::PublicRead
        } else {
            AuthzMode::Credentialed
        };
        self.load_authorized_space_with_mode(scope, mode, access)
            .await
    }

    /// Same as [`Self::load_authorized_space_with_token`] with an explicit
    /// [`AuthzMode`]. The info tools mirroring HTTP's `PublicReadLenient`
    /// endpoints must pass that mode: on public spaces those endpoints skip
    /// token verification entirely (`verify_space_token` counts usage), so a
    /// valid token sent to an MCP info tool must not drift the token's usage
    /// counter relative to the HTTP channel.
    async fn load_authorized_space_with_mode(
        &self,
        scope: TokenScope,
        mode: AuthzMode,
        access: &McpAccess,
    ) -> Result<(Arc<Space>, Caller), ErrorData> {
        let now_ms = unix_ms();
        let attempt = || {
            authz::authorize(
                &self.app,
                &access.space_id,
                &access.auth_token,
                access.sharding,
                scope,
                mode,
                now_ms,
            )
        };

        match attempt().await {
            Ok(rt) => Ok(rt),
            Err(AuthzError::SpaceNotFound { .. } | AuthzError::SpaceLoad { .. })
                if self.config.auto_create_space =>
            {
                if self.config.dynamic_space_from_path && !self.app.cwt_auth_enabled() {
                    return Err(ErrorData::invalid_request(
                        "remote MCP auto-create requires ED25519_PUBKEYS and a write CWT for the target space",
                        None,
                    ));
                }
                let creator = self
                    .app
                    .check_auth(
                        &access.auth_token,
                        &access.space_id,
                        TokenScope::Write,
                        now_ms,
                    )
                    .map_err(|_| unauthorized(TokenScope::Write))?;
                // The verified write-CWT principal owns the space it caused
                // to be created; `SELF_USER_ID` would misreport ownership.
                self.load_configured_space(&access.space_id, creator.user)
                    .await?;
                attempt().await.map_err(authz_error_data)
            }
            Err(err) => Err(authz_error_data(err)),
        }
    }

    pub async fn ensure_space_available(&self) -> Result<(), ErrorData> {
        if self.config.dynamic_space_from_path {
            return Ok(());
        }
        // Local fixed-space (stdio) mode has no authenticated caller; the
        // space belongs to the local dev identity.
        self.load_configured_space(&self.config.space_id, SELF_USER_ID)
            .await
            .map(|_| ())
    }

    /// Loads the space the way HTTP's `PublicReadLenient` endpoints do; see
    /// [`Self::load_authorized_space_with_mode`].
    async fn load_lenient_space(&self, access: &McpAccess) -> Result<Arc<Space>, ErrorData> {
        Ok(self
            .load_authorized_space_with_mode(TokenScope::Read, AuthzMode::PublicReadLenient, access)
            .await?
            .0)
    }

    async fn get_space_info_for(&self, access: &McpAccess) -> Result<CallToolResult, ErrorData> {
        let space = self.load_lenient_space(access).await?;
        structured_result(space.get_info())
    }

    async fn runtime_access(
        &self,
        access: &McpAccess,
        scope: TokenScope,
    ) -> Result<
        (
            Arc<crate::runtime_api::MemoryRuntime>,
            crate::runtime_api::RuntimeCaller,
        ),
        ErrorData,
    > {
        let (space, credential) = crate::authz::runtime_credentials(
            &self.app,
            &access.space_id,
            &access.auth_token,
            access.sharding,
            scope,
        )
        .await
        .map_err(runtime_error_data)?;
        let runtime = space.memory_runtime().ok_or_else(|| {
            runtime_error_data(crate::runtime_api::RuntimeError::Unavailable(
                "runtime bindings are not installed".into(),
            ))
        })?;
        let caller = runtime
            .map_credential(&credential)
            .map_err(runtime_error_data)?;
        Ok((runtime, caller))
    }
    async fn attention_for(
        &self,
        access: &McpAccess,
        input: crate::runtime_api::AttentionQuery,
    ) -> Result<CallToolResult, ErrorData> {
        let (runtime, caller) = self.runtime_access(access, TokenScope::Read).await?;
        structured_result(
            runtime
                .inbox(&caller, input)
                .await
                .map_err(runtime_error_data)?,
        )
    }
    async fn respond_attention_for(
        &self,
        access: &McpAccess,
        input: RespondAttentionToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (runtime, caller) = self.runtime_access(access, TokenScope::Write).await?;
        structured_result(
            runtime
                .respond(caller, input.id, input.response)
                .await
                .map_err(runtime_error_data)?,
        )
    }
    async fn runtime_status_for(&self, access: &McpAccess) -> Result<CallToolResult, ErrorData> {
        let (space, credential) = crate::authz::runtime_credentials(
            &self.app,
            &access.space_id,
            &access.auth_token,
            access.sharding,
            TokenScope::Read,
        )
        .await
        .map_err(runtime_error_data)?;
        let status = if let Some(runtime) = space.memory_runtime() {
            let caller = runtime
                .map_credential(&credential)
                .map_err(runtime_error_data)?;
            runtime
                .status(&caller, self.app.runtime_cwt_verifier_enabled())
                .await
                .map_err(runtime_error_data)?
        } else {
            crate::runtime_api::RuntimeStatus::unconfigured()
        };
        structured_result(status)
    }

    async fn get_formation_status_for(
        &self,
        access: &McpAccess,
    ) -> Result<CallToolResult, ErrorData> {
        let space = self.load_lenient_space(access).await?;
        structured_result(space.formation_status())
    }

    async fn remember_conversation_for(
        &self,
        access: &McpAccess,
        input: RememberConversationInput,
    ) -> Result<CallToolResult, ErrorData> {
        let input = input.into_formation_input()?;
        let space = self
            .load_authorized_space(TokenScope::Write, access)
            .await?;
        let output = space
            .ingest(SELF_USER_ID, StringOr::Value(input))
            .await
            .map_err(invalid_params)?;
        agent_output_result(output)
    }

    async fn recall_memory_for(
        &self,
        access: &McpAccess,
        input: RecallMemoryInput,
    ) -> Result<CallToolResult, ErrorData> {
        let input = RecallInput::from(input);
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        if let Some(reason) = caller.recall_forbidden() {
            // Same guard as HTTP /recall, surfaced through the shared authz
            // error mapping.
            return Err(authz_error_data(AuthzError::Forbidden(reason)));
        }
        // Same budget as HTTP /recall's 429 cap: without it, anonymous
        // callers on public spaces could run up to the global HTTP cap of
        // concurrent multi-turn LLM recalls through this tool.
        let _permit = self.acquire_llm_permit()?;
        let output = space
            .query(SELF_USER_ID, StringOr::Value(input))
            .await
            .map_err(invalid_params)?;
        agent_output_result(output)
    }

    async fn memory_for(
        &self,
        access: &McpAccess,
        input: MemoryInterfaceInput,
    ) -> Result<CallToolResult, ErrorData> {
        use anda_kip::memory::binding::{Operation, Request as MemoryRequest};
        let request: MemoryRequest =
            serde_json::from_value(input.request).map_err(invalid_params)?;
        let recall = request.operation == Operation::Recall;
        let scope = if recall {
            TokenScope::Read
        } else {
            TokenScope::Write
        };
        let (space, caller) = self.load_authorized_space_with_token(scope, access).await?;
        if recall && let Some(reason) = caller.recall_forbidden() {
            return Err(authz_error_data(AuthzError::Forbidden(reason)));
        }
        let _permit = if recall {
            Some(self.acquire_llm_permit()?)
        } else {
            None
        };
        let response = space
            .memory_request(&caller.namespace(), caller.is_owner(), request)
            .await;
        structured_result(response)
    }

    async fn stage_memory_source_for(
        &self,
        access: &McpAccess,
        input: crate::memory_interface::StageSourceInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Write, access)
            .await?;
        let staged = space
            .stage_memory_source(&caller.namespace(), input)
            .await
            .map_err(invalid_params)?;
        structured_result(staged)
    }

    async fn memory_receipt_for(
        &self,
        access: &McpAccess,
        input: MemoryReceiptInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_mode(TokenScope::Read, AuthzMode::Credentialed, access)
            .await?;
        let view = space
            .memory_receipt_view(&caller.namespace(), &input.receipt_ref)
            .await
            .map_err(invalid_params)?;
        structured_result(view)
    }

    async fn run_maintenance_for(
        &self,
        access: &McpAccess,
        input: RunMaintenanceInput,
    ) -> Result<CallToolResult, ErrorData> {
        let space = self
            .load_authorized_space(TokenScope::Write, access)
            .await?;
        if space.is_processing() {
            return Err(ErrorData::invalid_request(
                "formation or maintenance is already processing; retry after the current task finishes",
                None,
            ));
        }

        // Same budget as HTTP /maintenance's 429 cap (see recall_memory_for).
        let _permit = self.acquire_llm_permit()?;
        let output = space
            .maintenance(SELF_USER_ID, MaintenanceInput::from(input))
            .await
            .map_err(invalid_params)?;
        agent_output_result(output)
    }

    async fn execute_kip_readonly_for(
        &self,
        access: &McpAccess,
        input: ExecuteKipReadonlyInput,
    ) -> Result<CallToolResult, ErrorData> {
        let request = input.into_request()?;
        let space = self.load_lenient_space(access).await?;
        let response = space
            .execute_kip_readonly(request)
            .await
            .map_err(invalid_params)?;
        structured_result(response)
    }

    async fn get_or_init_user_for(
        &self,
        access: &McpAccess,
        input: GetOrInitUserToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let space = self
            .load_authorized_space(TokenScope::Write, access)
            .await?;
        let concept = space
            .formation
            .get_or_init_counterparty(input.user, input.name)
            .await
            .map_err(invalid_params)?;
        structured_result(concept)
    }

    async fn list_conversations_for(
        &self,
        access: &McpAccess,
        input: ListConversationsInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        // Same guard as the HTTP channel: label-restricted tokens are denied
        // outright, and anonymous public readers cannot touch recall
        // conversations (private-era runs may embed labeled wiki output).
        if let Some(reason) = caller.conversation_read_forbidden(input.collection.as_deref()) {
            return Err(authz_error_data(AuthzError::Forbidden(reason)));
        }
        let (conversations, next_cursor) = space
            .list_conversations(input.collection, input.cursor, input.limit)
            .await
            .map_err(invalid_params)?;
        structured_result(json!({
            "conversations": conversations,
            "next_cursor": next_cursor,
        }))
    }

    async fn get_conversation_for(
        &self,
        access: &McpAccess,
        input: GetConversationInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        // Same guard as list_conversations_for / the HTTP channel.
        if let Some(reason) = caller.conversation_read_forbidden(input.collection.as_deref()) {
            return Err(authz_error_data(AuthzError::Forbidden(reason)));
        }
        let conversation = space
            .get_conversation(input.collection, input.conversation_id)
            .await
            .map_err(invalid_params)?;

        if input.delta.unwrap_or(false) {
            structured_result(conversation.into_delta(
                input.messages_offset.unwrap_or_default(),
                input.artifacts_offset.unwrap_or_default(),
            ))
        } else {
            structured_result(conversation)
        }
    }
}

#[cfg(feature = "wiki")]
impl AndaBrainMcpServer {
    async fn wiki_commit_for(
        &self,
        access: &McpAccess,
        input: WikiCommitToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Write, access)
            .await?;
        // Real audit subject (PRD §8.1 hard requirement), same derivation as
        // the HTTP channel.
        let actor = caller.actor();
        let output = space
            .wiki
            .commit(actor, input.into(), unix_ms())
            .await
            .map_err(wiki_tool_error)?;
        structured_result(output)
    }

    async fn wiki_search_for(
        &self,
        access: &McpAccess,
        input: WikiSearchToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        let scope = caller.wiki_access();
        let input = WikiSearchInput::try_from(input)?;
        let output = space
            .wiki
            .search_scoped(&scope, input, unix_ms())
            .await
            .map_err(wiki_tool_error)?;
        structured_result(output)
    }

    async fn wiki_read_for(
        &self,
        access: &McpAccess,
        input: WikiReadToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        let scope = caller.wiki_access();
        let output = space
            .wiki
            .read_scoped(&scope, input.try_into()?, unix_ms())
            .await
            .map_err(wiki_tool_error)?;
        structured_result(output)
    }

    async fn wiki_verify_for(
        &self,
        access: &McpAccess,
        input: WikiVerifyToolInput,
    ) -> Result<CallToolResult, ErrorData> {
        let (space, caller) = self
            .load_authorized_space_with_token(TokenScope::Read, access)
            .await?;
        let scope = caller.wiki_access();
        let output = space
            .wiki
            .verify_scoped(
                &scope,
                WikiVerifyInput {
                    uri: Some(input.uri),
                    checksum: input.checksum,
                    ..Default::default()
                },
                unix_ms(),
            )
            .await
            .map_err(wiki_tool_error)?;
        structured_result(output)
    }
}

/// Mirrors `handler::wiki_error`'s classification on the MCP side: only
/// `Db` failures are internal; everything else is caller-fixable. A CAS
/// conflict carries `WikiError::retry_data` so tool callers get the same
/// re-read → merge → retry payload the HTTP channel returns.
#[cfg(feature = "wiki")]
fn wiki_tool_error(err: WikiError) -> ErrorData {
    match &err {
        WikiError::Db(_) => internal_error(err),
        WikiError::Conflict { .. } => {
            let data = err.retry_data();
            ErrorData::invalid_request(err.to_string(), data)
        }
        WikiError::NotFound(_) => ErrorData::invalid_request(err.to_string(), None),
        WikiError::TooLarge { .. } | WikiError::Invalid(_) => {
            ErrorData::invalid_params(err.to_string(), None)
        }
    }
}

/// Flat MCP input for `anda_brain_wiki_commit`.
#[cfg(feature = "wiki")]
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WikiCommitToolInput {
    /// Existing document id to update; omit to create a new document.
    pub doc_id: Option<u64>,
    /// Required for updates: the current_version this edit is based on. On a
    /// conflict error, re-read the document and retry with the reported
    /// current version.
    pub parent_version: Option<u64>,
    /// Logical partition such as "policy" or "engineering"; defaults to "default".
    pub namespace: Option<String>,
    /// Display slug; derived from the title when omitted.
    pub slug: Option<String>,
    pub title: String,
    /// Full Markdown content (the whole document, not a diff).
    pub content: String,
    pub tags: Option<Vec<String>>,
    /// External origin URI when importing.
    pub source_uri: Option<String>,
    /// Commit message: why this change.
    pub message: Option<String>,
    /// ACL label; omit to keep/inherit, empty string to clear.
    pub acl_label: Option<String>,
}

#[cfg(feature = "wiki")]
impl From<WikiCommitToolInput> for WikiCommitInput {
    fn from(input: WikiCommitToolInput) -> Self {
        Self {
            doc_id: input.doc_id,
            parent_version: input.parent_version,
            namespace: input.namespace,
            slug: input.slug,
            title: input.title,
            content: input.content,
            tags: input.tags,
            acl_label: input.acl_label,
            source_uri: input.source_uri,
            message: input.message,
            metadata: None,
        }
    }
}

/// Flat MCP input for `anda_brain_wiki_search`.
#[cfg(feature = "wiki")]
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WikiSearchToolInput {
    /// Keyword query; BM25 matching favors exact terms, product names and error codes.
    pub query: String,
    /// Restrict to these namespaces.
    pub namespaces: Option<Vec<String>>,
    /// Restrict to these document ids.
    pub doc_ids: Option<Vec<u64>>,
    /// Restrict to documents carrying any of these tags.
    pub tags: Option<Vec<String>>,
    /// Max hits (1-50, default 8).
    pub top_k: Option<usize>,
    /// "chunks" (default) for best passages, "docs" for one hit per document.
    pub mode: Option<String>,
    /// Neighbor expansion 0-2 (default 0): widen hits with adjacent passages.
    pub expand: Option<u8>,
}

#[cfg(feature = "wiki")]
impl TryFrom<WikiSearchToolInput> for WikiSearchInput {
    type Error = ErrorData;

    fn try_from(input: WikiSearchToolInput) -> Result<Self, Self::Error> {
        // The HTTP channel rejects unknown modes; the MCP channel must not
        // silently degrade them to "chunks" (an LLM typo like "documents"
        // would quietly change the result shape).
        let mode = match input.mode.as_deref() {
            Some("docs") => WikiSearchMode::Docs,
            Some("chunks") | None => WikiSearchMode::Chunks,
            Some(other) => {
                return Err(invalid_params(format!(
                    "invalid mode {other:?} (expected \"chunks\" or \"docs\")"
                )));
            }
        };
        Ok(Self {
            query: input.query,
            namespaces: input.namespaces.unwrap_or_default(),
            doc_ids: input.doc_ids.unwrap_or_default(),
            tags: input.tags.unwrap_or_default(),
            top_k: input.top_k,
            mode,
            expand: input.expand,
        })
    }
}

/// Flat MCP input for `anda_brain_wiki_read`.
#[cfg(feature = "wiki")]
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WikiReadToolInput {
    pub doc_id: u64,
    /// Version id for time-travel reads; omit for the current version.
    pub version: Option<u64>,
    /// "toc" | "section" | "range" | "full". Defaults to "section" when an
    /// anchor is given, "range" when start/end are given, else "toc".
    pub selector: Option<String>,
    /// Section anchor from a TOC entry or a search citation.
    pub anchor: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
}

#[cfg(feature = "wiki")]
impl TryFrom<WikiReadToolInput> for WikiReadInput {
    type Error = ErrorData;

    fn try_from(input: WikiReadToolInput) -> Result<Self, ErrorData> {
        let selector = match input.selector.as_deref() {
            Some("toc") => WikiSelector::Toc,
            Some("full") => WikiSelector::Full,
            Some("section") => WikiSelector::Section {
                anchor: input.anchor.clone().ok_or_else(|| {
                    ErrorData::invalid_request("selector 'section' requires anchor", None)
                })?,
            },
            Some("range") => match (input.start, input.end) {
                (Some(start), Some(end)) => WikiSelector::Range { start, end },
                _ => {
                    return Err(ErrorData::invalid_request(
                        "selector 'range' requires start and end",
                        None,
                    ));
                }
            },
            Some(other) => {
                return Err(ErrorData::invalid_request(
                    format!("unknown selector: {other}"),
                    None,
                ));
            }
            None => match (&input.anchor, input.start, input.end) {
                (Some(anchor), _, _) => WikiSelector::Section {
                    anchor: anchor.clone(),
                },
                (None, Some(start), Some(end)) => WikiSelector::Range { start, end },
                _ => WikiSelector::Toc,
            },
        };
        Ok(Self {
            doc_id: input.doc_id,
            version: input.version,
            selector,
        })
    }
}

/// Flat MCP input for `anda_brain_wiki_verify`.
#[cfg(feature = "wiki")]
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WikiVerifyToolInput {
    /// Citation URI: wiki://{space}/{doc_id}@{version_id}#{start}-{end}
    pub uri: String,
    /// Optional cited checksum to compare against stored content.
    pub checksum: Option<String>,
}

/// The wiki tool surface. It lives in its own router so the MCP channel
/// can be built without the `wiki` feature; [`AndaBrainMcpServer::new`]
/// merges it into the main `tool_router`.
#[cfg(feature = "wiki")]
#[tool_router(router = wiki_tool_router)]
impl AndaBrainMcpServer {
    /// Search the space wiki (versioned reference documents) with keyword BM25
    /// and get snippets with verifiable wiki:// citations. Cite the URIs when
    /// using the evidence; retry with reformulated keywords if results are poor.
    #[tool(
        name = "anda_brain_wiki_search",
        annotations(
            title = "Search Wiki",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn wiki_search(
        &self,
        Parameters(input): Parameters<WikiSearchToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.wiki_search_for(&access, input).await
    }

    /// Read a wiki document progressively: TOC first, then one section by
    /// anchor, a byte range, or the bounded full text. Supports reading
    /// historical versions.
    #[tool(
        name = "anda_brain_wiki_read",
        annotations(
            title = "Read Wiki Document",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn wiki_read(
        &self,
        Parameters(input): Parameters<WikiReadToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.wiki_read_for(&access, input).await
    }

    /// Commit a wiki document as an immutable new version (git-like).
    /// Create: omit doc_id. Update: pass doc_id and parent_version; on a
    /// conflict, re-read, merge, and retry. Identical content is a no-op.
    #[tool(
        name = "anda_brain_wiki_commit",
        annotations(
            title = "Commit Wiki Document",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn wiki_commit(
        &self,
        Parameters(input): Parameters<WikiCommitToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.wiki_commit_for(&access, input).await
    }

    /// Verify a wiki citation URI against the immutable stored content:
    /// valid, superseded by a newer version, or invalid.
    #[tool(
        name = "anda_brain_wiki_verify",
        annotations(
            title = "Verify Wiki Citation",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn wiki_verify(
        &self,
        Parameters(input): Parameters<WikiVerifyToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.wiki_verify_for(&access, input).await
    }
}

#[tool_router]
impl AndaBrainMcpServer {
    /// Read the current caller's durable memory attention inbox. Reading does not claim work.
    #[tool(
        name = "anda_brain_get_attention",
        annotations(
            title = "Read Memory Attention",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn get_attention(
        &self,
        Parameters(input): Parameters<crate::runtime_api::AttentionQuery>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.attention_for(&access, input).await
    }
    /// Answer a clarification or retain an agent statement. An answer is not execution authority; statements are not independent Outcomes.
    #[tool(
        name = "anda_brain_respond_attention",
        annotations(
            title = "Respond to Memory Attention",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn respond_attention(
        &self,
        Parameters(input): Parameters<RespondAttentionToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.respond_attention_for(&access, input).await
    }
    /// Read configured runtime capabilities and a bounded, caller-visible inventory.
    #[tool(
        name = "anda_brain_get_runtime_status",
        annotations(
            title = "Read Memory Runtime Status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn get_runtime_status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.runtime_status_for(&access).await
    }
    /// Return statistics and metadata for the configured Anda Brain memory space.
    #[tool(
        name = "anda_brain_get_space_info",
        annotations(
            title = "Get Space Info",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn get_space_info(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.get_space_info_for(&access).await
    }

    /// Return lightweight formation and maintenance progress for the configured memory space.
    #[tool(
        name = "anda_brain_get_formation_status",
        annotations(
            title = "Get Formation Status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn get_formation_status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.get_formation_status_for(&access).await
    }

    /// Encode conversation messages into long-term structured memory.
    /// Returns quickly after queuing asynchronous formation; use
    /// anda_brain_get_formation_status or anda_brain_get_conversation to track processing.
    #[tool(
        name = "anda_brain_remember_conversation",
        annotations(
            title = "Remember Conversation",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn remember_conversation(
        &self,
        Parameters(input): Parameters<RememberConversationInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.remember_conversation_for(&access, input).await
    }

    /// Ask a natural-language question against long-term memory.
    /// This is a synchronous recall run and can take over 60 seconds for complex searches;
    /// wait for the tool result instead of assuming it is background work.
    #[tool(
        name = "anda_brain_recall_memory",
        annotations(
            title = "Recall Memory",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn recall_memory(
        &self,
        Parameters(input): Parameters<RecallMemoryInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.recall_memory_for(&access, input).await
    }

    /// The KIP Memory Interface: one of observe, recall, revise, feedback or
    /// forget per request (`kip_memory: "2.0"`). Stage the observed messages
    /// first with anda_brain_stage_memory_source and cite the returned
    /// source_ref; retry a mutation with the same idempotency_key; pass
    /// outstanding receipts in recall `after`. An attention item or a briefing
    /// grants nothing.
    #[tool(
        name = "anda_brain_memory",
        annotations(
            title = "Memory Interface",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn memory(
        &self,
        Parameters(input): Parameters<MemoryInterfaceInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.memory_for(&access, input).await
    }

    /// Stage observed messages as an immutable source and get its
    /// source_ref for observe, revise or feedback. The same idempotency_key
    /// and bytes return the same handle.
    #[tool(
        name = "anda_brain_stage_memory_source",
        annotations(
            title = "Stage Memory Source",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn stage_memory_source(
        &self,
        Parameters(input): Parameters<crate::memory_interface::StageSourceInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.stage_memory_source_for(&access, input).await
    }

    /// Read a Memory Interface receipt's current progress.
    #[tool(
        name = "anda_brain_memory_receipt",
        annotations(
            title = "Memory Receipt",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn memory_receipt(
        &self,
        Parameters(input): Parameters<MemoryReceiptInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.memory_receipt_for(&access, input).await
    }

    /// Trigger a memory maintenance cycle for consolidation, pruning, and graph health.
    /// Returns quickly after starting asynchronous maintenance; use
    /// anda_brain_get_formation_status or anda_brain_get_conversation to track progress.
    #[tool(
        name = "anda_brain_run_maintenance",
        annotations(
            title = "Run Maintenance",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn run_maintenance(
        &self,
        Parameters(input): Parameters<RunMaintenanceInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.run_maintenance_for(&access, input).await
    }

    /// Run KIP 2.0 KQL and META commands against this Space's Cognitive Nexus for
    /// audit and graph inspection. A request containing any KML mutation is
    /// rejected, and each request is bounded by a 15-second timeout. Results are
    /// raw KIP responses: a Proposition that exists is not one that is believed, so
    /// use a `BELIEF` pattern for "is this true" questions and treat `insufficient`
    /// as unknown, not false. For natural-language questions use
    /// anda_brain_recall_memory instead; this tool does no interpretation.
    #[tool(
        name = "anda_brain_execute_kip_readonly",
        annotations(
            title = "Execute Read-Only KIP",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn execute_kip_readonly(
        &self,
        Parameters(input): Parameters<ExecuteKipReadonlyInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.execute_kip_readonly_for(&access, input).await
    }

    /// Get or create a counterparty concept for a user in memory formation context.
    #[tool(
        name = "anda_brain_get_or_init_user",
        annotations(
            title = "Get Or Init User",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn get_or_init_user(
        &self,
        Parameters(input): Parameters<GetOrInitUserToolInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.get_or_init_user_for(&access, input).await
    }

    /// List tracked formation, recall, or maintenance conversations with cursor pagination.
    #[tool(
        name = "anda_brain_list_conversations",
        annotations(
            title = "List Conversations",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn list_conversations(
        &self,
        Parameters(input): Parameters<ListConversationsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.list_conversations_for(&access, input).await
    }

    /// Get one tracked conversation, optionally as a delta from message/artifact offsets.
    #[tool(
        name = "anda_brain_get_conversation",
        annotations(
            title = "Get Conversation",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub async fn get_conversation(
        &self,
        Parameters(input): Parameters<GetConversationInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let access = self.access_from_context(&context)?;
        self.get_conversation_for(&access, input).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AndaBrainMcpServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("anda-brain-mcp", env!("CARGO_PKG_VERSION"))
                    .with_title("Anda Brain MCP Server"),
            )
            .with_instructions(format!(
                "Use these tools to access Anda Brain long-term memory for {}. Prefer anda_brain_recall_memory for questions and anda_brain_remember_conversation after meaningful user-agent exchanges.",
                if self.config.dynamic_space_from_path {
                    "the memory space selected by this MCP HTTP URL".to_string()
                } else {
                    format!("space '{}'", self.config.space_id)
                }
            ))
    }
}

pub async fn run_stdio_server(app: AppState, config: McpServerConfig) -> Result<(), BoxError> {
    let server = AndaBrainMcpServer::new(app.clone(), config);
    server.ensure_space_available().await.map_err(|err| {
        format!(
            "failed to initialize Anda Brain MCP server: {}",
            err.message
        )
    })?;

    let cancel_token = CancellationToken::new();
    let background_app = app.clone();
    let background_cancel = cancel_token.clone();
    let background_handle = tokio::spawn(async move {
        background_app
            .start_background_tasks(background_cancel)
            .await;
    });

    let service = rmcp::serve_server(server, stdio()).await?;
    let service_result = service.waiting().await;
    cancel_token.cancel();
    let _ = background_handle.await;
    service_result?;
    Ok(())
}

pub fn build_streamable_http_service(
    app: AppState,
    config: McpHttpServerConfig,
    cancellation_token: CancellationToken,
) -> StreamableHttpService<AndaBrainMcpServer, LocalSessionManager> {
    let path_prefix = normalize_mcp_path_prefix(&config.path_prefix);
    let service_config = McpServerConfig {
        space_id: String::new(),
        auth_token: None,
        auto_create_space: config.auto_create_space,
        auto_create_tier: config.auto_create_tier,
        dynamic_space_from_path: true,
        remote_path_prefix: path_prefix,
    };
    let service_app = app.clone();
    let mut transport_config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancellation_token)
        .with_legacy_session_mode(config.stateful_mode)
        .with_json_response(config.json_response)
        .with_sse_keep_alive(config.sse_keep_alive_secs.map(Duration::from_secs));

    if !config.allowed_hosts.is_empty() {
        transport_config = if config.allowed_hosts.iter().any(|host| host == "*") {
            transport_config.disable_allowed_hosts()
        } else {
            transport_config.with_allowed_hosts(config.allowed_hosts)
        };
    }
    if !config.allowed_origins.is_empty() {
        transport_config = if config.allowed_origins.iter().any(|origin| origin == "*") {
            transport_config.disable_allowed_origins()
        } else {
            transport_config.with_allowed_origins(config.allowed_origins)
        };
    }

    StreamableHttpService::new(
        move || {
            Ok(AndaBrainMcpServer::new(
                service_app.clone(),
                service_config.clone(),
            ))
        },
        Default::default(),
        transport_config,
    )
}

fn structured_result<T>(value: T) -> Result<CallToolResult, ErrorData>
where
    T: Serialize,
{
    let value = serde_json::to_value(value).map_err(internal_error)?;
    Ok(CallToolResult::structured(value))
}

fn agent_output_result(output: anda_core::AgentOutput) -> Result<CallToolResult, ErrorData> {
    let is_error = output.failed_reason.is_some();
    let text = if output.content.trim().is_empty() {
        serde_json::to_string(&output).map_err(internal_error)?
    } else {
        output.content.clone()
    };
    let value = serde_json::to_value(output).map_err(internal_error)?;
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(text)];
    result.is_error = Some(is_error);
    Ok(result)
}

fn unauthorized(scope: TokenScope) -> ErrorData {
    ErrorData::invalid_request(
        format!(
            "{scope:?} access denied for Anda Brain MCP space. Configure MCP_AUTH_TOKEN or --mcp-auth-token with a CWT or space token that has the required scope."
        ),
        None,
    )
}

fn runtime_error_data(error: crate::runtime_api::RuntimeError) -> ErrorData {
    let error = crate::handler::runtime_error(error);
    let data = Some(json!({"http_status":error.status.as_u16()}));
    if error.status == http::StatusCode::BAD_REQUEST {
        ErrorData::invalid_params(error.message, data)
    } else if error.status.is_server_error() {
        ErrorData::internal_error(error.message, data)
    } else {
        ErrorData::invalid_request(error.message, data)
    }
}

/// Maps a shared authorization failure onto the MCP error surface,
/// preserving this channel's historical messages (the HTTP mapping lives in
/// `From<AuthzError> for AppError`).
fn authz_error_data(err: AuthzError) -> ErrorData {
    match err {
        AuthzError::ShardMismatch { sharding, expected } => ErrorData::invalid_request(
            format!("space_id sharding {sharding} does not match server sharding {expected}"),
            None,
        ),
        AuthzError::Unauthorized(scope) => unauthorized(scope),
        AuthzError::SpaceNotFound {
            space_id, display, ..
        }
        | AuthzError::SpaceLoad {
            space_id, display, ..
        } => internal_error(format!(
            "failed to load memory space '{space_id}': {display}. Create it first or start MCP with --mcp-auto-create-space"
        )),
        AuthzError::Forbidden(message) => ErrorData::invalid_request(message, None),
    }
}

fn invalid_params(error: impl ToString) -> ErrorData {
    ErrorData::invalid_params(error.to_string(), None)
}

fn internal_error(error: impl ToString) -> ErrorData {
    ErrorData::internal_error(error.to_string(), None)
}

fn normalize_mcp_path_prefix(prefix: &str) -> String {
    let trimmed = prefix.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        "/mcp".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn space_id_from_mcp_path(path: &str, prefix: &str) -> Result<String, ErrorData> {
    let prefix = normalize_mcp_path_prefix(prefix);
    let rest = if path == prefix {
        ""
    } else if let Some(rest) = path.strip_prefix(&format!("{prefix}/")) {
        rest
    } else {
        path.trim_start_matches('/')
    };
    let space_id = rest.split('/').next().unwrap_or_default();
    if space_id.is_empty() {
        return Err(ErrorData::invalid_request(
            format!("MCP URL must include a space id, for example {prefix}/my_space_001"),
            None,
        ));
    }

    Ok(space_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{app_state_core, models_with_completer, signed_token, signing_key};
    use anda_core::{AgentOutput, BoxPinFut, CompletionRequest};
    use anda_engine::model::{CompletionFeaturesDyn, reqwest};
    use http::{HeaderMap, header};
    use ic_cose_types::cose::ed25519::VerifyingKey;

    #[tokio::test]
    async fn r4_mcp_shares_runtime_auth_and_response_service_without_observer_tools() {
        use crate::space::tests::runtime_api::{fixture, token};
        let name = "r4_mcp";
        let (app, space, _) = fixture(name, Some("Which date?".into())).await;
        space.attention().tick().await.unwrap();
        let server = AndaBrainMcpServer::new(
            app,
            McpServerConfig::stdio(name.into(), Some(token(name, 10))),
        );
        let access = McpAccess {
            space_id: name.into(),
            auth_token: token(name, 10),
            sharding: None,
        };
        let page = server
            .attention_for(&access, crate::runtime_api::AttentionQuery::default())
            .await
            .unwrap();
        let data = page.structured_content.unwrap();
        let question = data["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["clarification"].is_object())
            .unwrap();
        let response = server
            .respond_attention_for(
                &access,
                RespondAttentionToolInput {
                    id: question["id"].as_str().unwrap().into(),
                    response: crate::runtime_api::AttentionResponse::Clarification {
                        event_key: "mcp-answer".into(),
                        answer: "Tomorrow".into(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(
            response.structured_content.unwrap()["status"],
            "answer_received_not_authorization"
        );
        let anonymous = McpAccess {
            space_id: name.into(),
            auth_token: String::new(),
            sharding: None,
        };
        assert!(
            server
                .attention_for(&anonymous, crate::runtime_api::AttentionQuery::default())
                .await
                .is_err()
        );
        let tools = server.tool_router.list_all();
        let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"anda_brain_get_attention"));
        assert!(names.contains(&"anda_brain_respond_attention"));
        assert!(names.contains(&"anda_brain_get_runtime_status"));
        assert!(
            !names
                .iter()
                .any(|n| n.contains("outcome") || n.contains("register_executor"))
        );
        space.close().await.unwrap();
    }

    #[derive(Debug)]
    struct FinalCompleter;

    impl CompletionFeaturesDyn for FinalCompleter {
        fn model_name(&self) -> String {
            "mcp-test-model".to_string()
        }

        fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
            Box::pin(async move {
                Ok(AgentOutput {
                    content: format!("mcp processed: {}", req.prompt),
                    ..Default::default()
                })
            })
        }
    }

    fn test_app_state(name: &str, pubkeys: Vec<VerifyingKey>) -> AppState {
        app_state_core(
            name,
            models_with_completer(FinalCompleter),
            pubkeys,
            "test-version",
            0,
        )
    }

    async fn create_server(space_id: &str) -> AndaBrainMcpServer {
        let app = test_app_state("mcp_tests", vec![]);
        app.admin_create_space(
            SELF_USER_ID,
            SELF_USER_ID,
            space_id.to_string(),
            1,
            unix_ms(),
        )
        .await
        .unwrap();
        AndaBrainMcpServer::new(
            app,
            McpServerConfig {
                space_id: space_id.to_string(),
                auth_token: None,
                auto_create_space: false,
                auto_create_tier: 1,
                dynamic_space_from_path: false,
                remote_path_prefix: "/mcp".to_string(),
            },
        )
    }

    fn test_access(space_id: &str) -> McpAccess {
        McpAccess {
            space_id: space_id.to_string(),
            auth_token: String::new(),
            sharding: None,
        }
    }

    #[test]
    fn mcp_message_accepts_text_and_content_parts() {
        let text = McpMessage {
            role: "user".to_string(),
            content: McpMessageContent::Text("hello".to_string()),
            name: None,
            user: None,
            timestamp: Some(42),
        }
        .into_message()
        .unwrap();
        assert_eq!(text.text().as_deref(), Some("hello"));
        assert_eq!(text.timestamp, Some(42));

        let parts = McpMessage {
            role: "assistant".to_string(),
            content: McpMessageContent::Parts(vec![json!({
                "type": "Text",
                "text": "done"
            })]),
            name: None,
            user: None,
            timestamp: None,
        }
        .into_message()
        .unwrap();
        assert_eq!(parts.text().as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn tool_schemas_use_no_schema_combinators() {
        fn assert_free(path: &str, value: &Value) {
            match value {
                Value::Object(map) => {
                    for key in ["anyOf", "oneOf", "allOf"] {
                        assert!(!map.contains_key(key), "{path} contains {key}");
                    }
                    for (name, child) in map {
                        assert_free(&format!("{path}.{name}"), child);
                    }
                }
                Value::Array(items) => {
                    for (i, child) in items.iter().enumerate() {
                        assert_free(&format!("{path}[{i}]"), child);
                    }
                }
                _ => {}
            }
        }
        let server = create_server("mcp_tool_schemas").await;
        let tools = server.tool_router.list_all();
        for tool in &tools {
            assert_free(&tool.name, &Value::Object((*tool.input_schema).clone()));
        }
        let schema = |name: &str| {
            let tool = tools.iter().find(|tool| tool.name == name).unwrap();
            Value::Object((*tool.input_schema).clone())
        };

        // `Option<Struct>`: the field is left out instead of sent as null.
        let recall = schema("anda_brain_recall_memory");
        assert_eq!(
            recall["properties"]["context"],
            json!({"$ref": "#/$defs/InputContext"})
        );
        // An untagged enum of distinct types keeps both shapes.
        let kip = schema("anda_brain_execute_kip_readonly");
        let item = &kip["$defs"]["McpKipCommandItem"];
        assert_eq!(item["type"], json!(["string", "object"]));
        assert_eq!(item["required"], json!(["command"]));
        // A tagged enum: one object, the tag as an enum, and only the fields
        // every variant needs required.
        let attention = schema("anda_brain_respond_attention");
        let response = &attention["$defs"]["AttentionResponse"];
        assert_eq!(response["type"], "object");
        assert_eq!(
            response["properties"]["kind"]["enum"],
            json!(["clarification", "agent_statement"])
        );
        assert_eq!(response["required"], json!(["kind", "event_key"]));
        assert_eq!(response["additionalProperties"], false);
        let description = response["description"].as_str().unwrap();
        assert!(description.contains("With kind \"clarification\", also send answer."));
        assert!(description.contains("With kind \"agent_statement\", also send statement."));
        let parsed: crate::runtime_api::AttentionResponse = serde_json::from_value(
            json!({"kind": "agent_statement", "event_key": "e", "statement": "s"}),
        )
        .unwrap();
        assert!(matches!(
            parsed,
            crate::runtime_api::AttentionResponse::AgentStatement { .. }
        ));
    }

    #[tokio::test]
    async fn tool_router_exposes_core_memory_tools_with_annotations() {
        let server = create_server("mcp_tool_router").await;
        let tools = server.tool_router.list_all();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();

        assert!(names.contains(&"anda_brain_remember_conversation"));
        assert!(names.contains(&"anda_brain_recall_memory"));
        assert!(names.contains(&"anda_brain_run_maintenance"));
        assert!(names.contains(&"anda_brain_execute_kip_readonly"));

        // The wiki tools live in their own `#[tool_router]` block so the MCP
        // channel builds without the `wiki` feature; `new` merges them in.
        for wiki_tool in [
            "anda_brain_wiki_search",
            "anda_brain_wiki_read",
            "anda_brain_wiki_commit",
            "anda_brain_wiki_verify",
        ] {
            assert_eq!(names.contains(&wiki_tool), cfg!(feature = "wiki"));
        }

        let recall = tools
            .iter()
            .find(|tool| tool.name == "anda_brain_recall_memory")
            .unwrap();
        assert!(
            recall
                .description
                .as_deref()
                .is_some_and(|description| description.contains("Ask a natural-language question"))
        );
        assert!(
            recall
                .description
                .as_deref()
                .is_some_and(|description| description.contains("can take over 60 seconds"))
        );
        let recall_schema = Value::Object(recall.input_schema.as_ref().clone());
        let recall_properties = recall_schema["properties"].as_object().unwrap();
        assert_eq!(recall_properties["query"]["type"].as_str(), Some("string"));
        assert!(recall_properties.contains_key("context"));
        assert!(
            recall_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field.as_str() == Some("query"))
        );
        assert_eq!(
            recall.annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );

        let remember = tools
            .iter()
            .find(|tool| tool.name == "anda_brain_remember_conversation")
            .unwrap();
        assert!(
            remember
                .description
                .as_deref()
                .is_some_and(|description| description.contains("Encode conversation messages"))
        );
        assert!(
            remember
                .description
                .as_deref()
                .is_some_and(|description| description.contains("queuing asynchronous formation"))
        );
        let remember_schema = Value::Object(remember.input_schema.as_ref().clone());
        let remember_properties = remember_schema["properties"].as_object().unwrap();
        assert!(remember_properties.contains_key("messages"));
        assert!(
            remember_schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field.as_str() == Some("messages"))
        );
        assert_eq!(
            remember.annotations.as_ref().unwrap().read_only_hint,
            Some(false)
        );

        let maintenance = tools
            .iter()
            .find(|tool| tool.name == "anda_brain_run_maintenance")
            .unwrap();
        assert!(
            maintenance
                .description
                .as_deref()
                .is_some_and(|description| description.contains("asynchronous maintenance"))
        );

        let info = tools
            .iter()
            .find(|tool| tool.name == "anda_brain_get_space_info")
            .unwrap();
        assert!(
            info.description
                .as_deref()
                .is_some_and(|description| description.contains("statistics and metadata"))
        );
        let info_schema = Value::Object(info.input_schema.as_ref().clone());
        assert_eq!(info_schema["type"].as_str(), Some("object"));
    }

    #[tokio::test]
    async fn space_info_tool_returns_configured_space() {
        let server = create_server("mcp_space_info").await;

        let result = server
            .get_space_info_for(&test_access("mcp_space_info"))
            .await
            .unwrap();
        let value = result.structured_content.unwrap();

        assert_eq!(value["id"], "mcp_space_info");
        assert_eq!(result.is_error, Some(false));
    }

    #[tokio::test]
    async fn remember_tool_runs_through_formation_agent() {
        let server = create_server("mcp_remember").await;
        let result = server
            .remember_conversation_for(
                &test_access("mcp_remember"),
                RememberConversationInput {
                    messages: vec![McpMessage {
                        role: "user".to_string(),
                        content: McpMessageContent::Text("Alice likes dark mode".to_string()),
                        name: None,
                        user: None,
                        timestamp: None,
                    }],
                    context: Some(InputContext {
                        counterparty: Some("alice".to_string()),
                        agent: Some("test-agent".to_string()),
                        source: Some("mcp-test".to_string()),
                        topic: Some("preferences".to_string()),
                    }),
                    timestamp: Some("2026-06-25T00:00:00.000Z".to_string()),
                },
            )
            .await
            .unwrap();

        assert_eq!(result.is_error, Some(false));
        assert!(result.content[0].as_text().is_some());
        let value = result.structured_content.as_ref().unwrap();
        assert!(value["conversation"].is_number());
    }

    #[tokio::test]
    async fn input_errors_surface_as_invalid_params() {
        let server = create_server("mcp_invalid_params").await;

        // Same failure over HTTP is a 400: an empty conversation is rejected
        // by `Space::ingest`, so the MCP channel must not report it as an
        // internal error.
        let err = server
            .remember_conversation_for(
                &test_access("mcp_invalid_params"),
                RememberConversationInput {
                    messages: vec![],
                    context: None,
                    timestamp: None,
                },
            )
            .await
            .unwrap_err();

        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    #[cfg(feature = "wiki")]
    #[tokio::test]
    async fn wiki_commit_conflict_carries_retry_data() {
        let server = create_server("mcp_wiki_conflict").await;
        let access = test_access("mcp_wiki_conflict");

        let committed = server
            .wiki_commit_for(
                &access,
                WikiCommitToolInput {
                    title: "冲突文档".to_string(),
                    content: "# 冲突文档\n\n第一版。\n".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let value = committed.structured_content.unwrap();
        let doc_id = value["doc"]["id"].as_u64().unwrap();
        let current_version = value["doc"]["current_version"].as_u64().unwrap();

        // A stale CAS token must fail with the retry payload the tool
        // description promises (re-read, merge, retry with current_version).
        let err = server
            .wiki_commit_for(
                &access,
                WikiCommitToolInput {
                    doc_id: Some(doc_id),
                    parent_version: Some(current_version + 1),
                    title: "冲突文档".to_string(),
                    content: "# 冲突文档\n\n第二版。\n".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_REQUEST);
        let data = err.data.expect("conflict error must carry retry data");
        assert_eq!(data["current_version"].as_u64(), Some(current_version));
        assert!(data["current_checksum"].is_string());
        assert!(data["updated_by"].is_string());
        assert!(data["updated_at"].is_number());
    }

    #[cfg(feature = "wiki")]
    #[tokio::test]
    async fn wiki_tools_scope_anonymous_and_labeled_callers() {
        use crate::types::{AddSpaceTokenInput, UpdateSpaceInput};
        use crate::wiki::WikiCommitInput;

        // Auth enabled: an empty bearer is an anonymous caller, not the
        // synthetic dev CWT.
        let signing_key = signing_key(9);
        let app = test_app_state("mcp_wiki_acl", vec![signing_key.verifying_key()]);
        let space_id = "mcp_wiki_acl_space";
        app.admin_create_space(
            SELF_USER_ID,
            SELF_USER_ID,
            space_id.to_string(),
            1,
            unix_ms(),
        )
        .await
        .unwrap();
        let space = app.load_space(space_id, false).await.unwrap();
        space
            .update(
                UpdateSpaceInput {
                    public: Some(true),
                    ..Default::default()
                },
                unix_ms(),
            )
            .await
            .unwrap();
        space
            .wiki
            .commit(
                "owner".to_string(),
                WikiCommitInput {
                    title: "公开文档".to_string(),
                    content: "# 公开文档\n\n公开探针：风铃海岸。\n".to_string(),
                    ..Default::default()
                },
                unix_ms(),
            )
            .await
            .unwrap();
        space
            .wiki
            .commit(
                "owner".to_string(),
                WikiCommitInput {
                    title: "机密文档".to_string(),
                    content: "# 机密文档\n\n机密探针:曙光矩阵。\n".to_string(),
                    acl_label: Some("secret".to_string()),
                    ..Default::default()
                },
                unix_ms(),
            )
            .await
            .unwrap();
        space
            .add_space_token(
                "STsecret-reader".to_string(),
                AddSpaceTokenInput {
                    scope: TokenScope::Read,
                    name: "sec".to_string(),
                    expires_at: None,
                    labels: Some(vec!["secret".to_string()]),
                },
                unix_ms(),
            )
            .await
            .unwrap();
        space
            .add_space_token(
                "STwiki-writer".to_string(),
                AddSpaceTokenInput {
                    scope: TokenScope::Write,
                    name: "writer".to_string(),
                    expires_at: None,
                    labels: None,
                },
                unix_ms(),
            )
            .await
            .unwrap();

        let server = AndaBrainMcpServer::new(
            app,
            McpServerConfig {
                space_id: space_id.to_string(),
                auth_token: None,
                auto_create_space: false,
                auto_create_tier: 1,
                dynamic_space_from_path: false,
                remote_path_prefix: "/mcp".to_string(),
            },
        );
        let hits_of = |result: CallToolResult| {
            result.structured_content.unwrap()["hits"]
                .as_array()
                .unwrap()
                .len()
        };

        // Launch review P0-1: an anonymous reader of a public space gets the
        // unlabeled-only view, never the unrestricted one.
        let anon = test_access(space_id);
        let secret_probe = server
            .wiki_search_for(
                &anon,
                WikiSearchToolInput {
                    query: "曙光矩阵".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hits_of(secret_probe), 0);
        let open_probe = server
            .wiki_search_for(
                &anon,
                WikiSearchToolInput {
                    query: "风铃海岸".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hits_of(open_probe), 1);

        // A labeled token keeps its granted labels on the public space
        // instead of degrading to anonymous.
        let labeled = McpAccess {
            space_id: space_id.to_string(),
            auth_token: "STsecret-reader".to_string(),
            sharding: None,
        };
        let granted = server
            .wiki_search_for(
                &labeled,
                WikiSearchToolInput {
                    query: "曙光矩阵".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hits_of(granted), 1);

        // Launch review P1-3: label-restricted tokens cannot use agentic
        // recall (its wiki tools span all labels).
        let err = server
            .recall_memory_for(
                &labeled,
                RecallMemoryInput {
                    query: "机密内容是什么?".to_string(),
                    context: None,
                    budget: None,
                },
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("unrestricted"), "{}", err.message);

        // Launch review P1-2: MCP commits record the real token subject.
        let writer = McpAccess {
            space_id: space_id.to_string(),
            auth_token: "STwiki-writer".to_string(),
            sharding: None,
        };
        let committed = server
            .wiki_commit_for(
                &writer,
                WikiCommitToolInput {
                    title: "白皮书".to_string(),
                    content: "# 白皮书\n\n提交自 MCP。\n".to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let value = committed.structured_content.unwrap();
        assert_eq!(value["doc"]["created_by"].as_str(), Some("st:writer"));
        assert_eq!(value["version"]["author"].as_str(), Some("st:writer"));
    }

    #[tokio::test]
    async fn auth_enabled_requires_configured_token() {
        let mut bytes = [0x66; 32];
        bytes[0] = 0x58;
        let key = VerifyingKey::from_bytes(&bytes).unwrap();
        let app = test_app_state("mcp_auth", vec![key]);
        app.admin_create_space(
            SELF_USER_ID,
            SELF_USER_ID,
            "mcp_auth_space".to_string(),
            1,
            unix_ms(),
        )
        .await
        .unwrap();
        let server = AndaBrainMcpServer::new(
            app,
            McpServerConfig {
                space_id: "mcp_auth_space".to_string(),
                auth_token: None,
                auto_create_space: false,
                auto_create_tier: 1,
                dynamic_space_from_path: false,
                remote_path_prefix: "/mcp".to_string(),
            },
        );

        let err = server
            .get_space_info_for(&test_access("mcp_auth_space"))
            .await
            .unwrap_err();

        assert!(err.message.contains("access denied"));
    }

    #[tokio::test]
    async fn remote_auto_create_requires_cwt_auth_to_be_enabled() {
        let app = test_app_state("mcp_auto_create_no_auth", vec![]);
        let server = AndaBrainMcpServer::new(
            app.clone(),
            McpServerConfig {
                space_id: "unused".to_string(),
                auth_token: None,
                auto_create_space: true,
                auto_create_tier: 1,
                dynamic_space_from_path: true,
                remote_path_prefix: "/mcp".to_string(),
            },
        );
        let space_id = "mcp_auto_create_no_auth_space";

        let err = match server
            .load_authorized_space(TokenScope::Write, &test_access(space_id))
            .await
        {
            Ok(_) => panic!("expected auth-disabled remote auto-create to fail"),
            Err(err) => err,
        };

        assert!(err.message.contains("requires ED25519_PUBKEYS"));
        assert!(app.load_space(space_id, false).await.is_err());
    }

    #[tokio::test]
    async fn remote_auto_create_requires_write_cwt_before_creating_space() {
        let signing_key = signing_key(9);
        let app = test_app_state("mcp_auto_create_auth", vec![signing_key.verifying_key()]);
        let server = AndaBrainMcpServer::new(
            app.clone(),
            McpServerConfig {
                space_id: "unused".to_string(),
                auth_token: None,
                auto_create_space: true,
                auto_create_tier: 1,
                dynamic_space_from_path: true,
                remote_path_prefix: "/mcp".to_string(),
            },
        );
        let space_id = "mcp_auto_create_guard";

        let err = match server
            .load_authorized_space(TokenScope::Read, &test_access(space_id))
            .await
        {
            Ok(_) => panic!("expected unauthenticated auto-create to fail"),
            Err(err) => err,
        };
        assert!(err.message.contains("access denied"));
        assert!(app.load_space(space_id, false).await.is_err());

        let read_cwt = signed_token(&signing_key, SELF_USER_ID, space_id, "read");
        let err = match server
            .load_authorized_space(
                TokenScope::Read,
                &McpAccess {
                    space_id: space_id.to_string(),
                    auth_token: read_cwt,
                    sharding: None,
                },
            )
            .await
        {
            Ok(_) => panic!("expected read CWT auto-create to fail"),
            Err(err) => err,
        };
        assert!(err.message.contains("access denied"));
        assert!(app.load_space(space_id, false).await.is_err());

        let write_cwt = signed_token(&signing_key, SELF_USER_ID, space_id, "write");
        let created = server
            .load_authorized_space(
                TokenScope::Write,
                &McpAccess {
                    space_id: space_id.to_string(),
                    auth_token: write_cwt,
                    sharding: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(created.get_info().id, space_id);
    }

    #[tokio::test]
    async fn streamable_http_endpoint_uses_space_from_path() {
        let app = test_app_state("mcp_http", vec![]);
        app.admin_create_space(
            SELF_USER_ID,
            SELF_USER_ID,
            "mcp_http_space".to_string(),
            1,
            unix_ms(),
        )
        .await
        .unwrap();

        let cancel_token = CancellationToken::new();
        let service = build_streamable_http_service(
            app,
            McpHttpServerConfig {
                path_prefix: "/mcp".to_string(),
                auto_create_space: false,
                auto_create_tier: 1,
                allowed_hosts: vec!["127.0.0.1".to_string()],
                allowed_origins: vec![],
                stateful_mode: true,
                json_response: false,
                sse_keep_alive_secs: None,
            },
            cancel_token.child_token(),
        );
        let router = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_cancel = cancel_token.clone();
        let server_handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { server_cancel.cancelled_owned().await })
                .await
                .unwrap();
        });

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let endpoint = format!("http://{addr}/mcp/mcp_http_space");
        let initialize = client
            .post(&endpoint)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "anda-brain-test",
                        "version": "0.0.0"
                    }
                }
            }))
            .send()
            .await
            .unwrap();

        let initialize_status = initialize.status();
        let initialize_headers = initialize.headers().clone();
        let initialize_body = initialize.text().await.unwrap();
        assert!(
            initialize_status.is_success(),
            "initialize failed: {initialize_status} {initialize_body}"
        );
        let session_id = initialize_headers
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let initialized = client
            .post(&endpoint)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", &session_id)
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }))
            .send()
            .await
            .unwrap();
        assert!(initialized.status().is_success());

        let call = client
            .post(&endpoint)
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .header("mcp-session-id", &session_id)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "anda_brain_get_space_info",
                    "arguments": {}
                }
            }))
            .send()
            .await
            .unwrap();

        assert!(call.status().is_success());
        let body = call.text().await.unwrap();
        assert!(body.contains("mcp_http_space"));

        cancel_token.cancel();
        server_handle.await.unwrap();
    }

    #[test]
    fn http_helpers_extract_space_token_and_sharding() {
        assert_eq!(
            space_id_from_mcp_path("/mcp/alice_space", "/mcp").unwrap(),
            "alice_space"
        );
        assert_eq!(
            space_id_from_mcp_path("/mcp/alice_space/events", "/mcp").unwrap(),
            "alice_space"
        );
        assert_eq!(
            space_id_from_mcp_path("/alice_space", "/mcp").unwrap(),
            "alice_space"
        );
        assert!(space_id_from_mcp_path("/mcp", "/mcp").is_err());

        // The MCP channel shares the HTTP header extractors from `payload`.
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer ST-token".parse().unwrap());
        headers.insert("X-Shard", "7".parse().unwrap());

        assert_eq!(extract_bearer_token(&headers), "ST-token");
        assert_eq!(extract_shard_id(&headers), 7);
    }
}
