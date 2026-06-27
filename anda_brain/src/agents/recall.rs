use anda_cognitive_nexus::ConceptPK;
use anda_core::{
    Agent, AgentContext, AgentOutput, BoxError, CompletionRequest, Document, Documents,
    FunctionDefinition, Json, Message, Resource, StateFeatures, Tool, ToolOutput, estimate_tokens,
};
use anda_engine::{
    context::{AgentCtx, BaseCtx},
    extension::note::{NoteTool, load_notes, load_notes_from_legacy},
    local_date_hour,
    memory::{
        Conversation, ConversationRef, ConversationStatus, Conversations, MemoryManagement,
        MemoryReadonly,
    },
    unix_ms,
};
use parking_lot::RwLock;
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::time::timeout;

use anda_kip::{KipError, KipErrorCode, Request, Response};

use super::{
    BrainHook, SELF_USER_ID, append_runner_history, compact_runner_if_needed,
    push_completed_history,
};
use crate::types::RecallInput;

const SELF_INSTRUCTIONS: &str = include_str!("../../assets/BrainRecall.md");
const RECALL_CONTEXT_TIMEOUT: Duration = Duration::from_secs(2);
pub const READONLY_KIP_TIMEOUT: Duration = Duration::from_secs(15);

pub static FUNCTION_DEFINITION: LazyLock<FunctionDefinition> = LazyLock::new(|| {
    serde_json::from_value(json!({
        "name": "recall_memory",
        "description": "Recall information from the assistant's long-term memory (the Cognitive Nexus owned by $self). Use only for information that is not already present in the active conversation. Do not call for facts just mentioned, just submitted to formation, or otherwise available in current context; formation is asynchronous and fresh memories may take a minute or more to become searchable.",
        "parameters": {
            "type": "object",
            "properties": {
            "query": {
                "type": "string",
                "description": "A natural language question about older or out-of-context memory. Be specific and include the subject, timeframe, and topic when known. Examples: 'What do we know about the current user's communication preferences?', 'What happened in our last discussion about Project Aurora?', 'Who are the members of the engineering team?'"
            },
            "context": {
                "type": [
                    "object",
                    "null"
                ],
                "description": "Optional current conversational context used only to disambiguate the query within $self's memory. Pass an object, not a JSON string. It does not change the memory owner.",
                "properties": {
                "counterparty": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Preferred. Durable identifier of the current external person or organization interacting with the business agent. Useful for resolving implicit references such as 'the current user', 'they', or omitted subjects."
                },
                "agent": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "The identifier of the calling business agent, if applicable. Useful for provenance or caller-specific queries, but it does not change whose memory is searched."
                },
                "source": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "Identifier of the current source, thread, channel, or app context. Useful when the query refers to a previous discussion in the same place."
                },
                "topic": {
                    "type": [
                        "string",
                        "null"
                    ],
                    "description": "The topic of the current conversation, to help disambiguate the query."
                }
                },
                "required": [
                    "counterparty",
                    "agent",
                    "source",
                    "topic"
                ],
                "additionalProperties": false
            }
            },
            "required": [
                "query",
                "context"
            ],
            "additionalProperties": false
        },
        "strict": true
        })).unwrap()
});

#[derive(Clone)]
pub struct TimedMemoryReadonly {
    memory: Arc<MemoryManagement>,
    timeout: Duration,
}

impl TimedMemoryReadonly {
    pub fn new(memory: Arc<MemoryManagement>) -> Self {
        Self {
            memory,
            timeout: READONLY_KIP_TIMEOUT,
        }
    }
}

impl Tool<BaseCtx> for TimedMemoryReadonly {
    type Args = Request;
    type Output = Response;

    fn name(&self) -> String {
        MemoryReadonly::NAME.to_string()
    }

    fn description(&self) -> String {
        "Executes one or more KIP (Knowledge Interaction Protocol) commands against the Cognitive Nexus to read from your persistent memory. This tool does not allow any modifications to the memory and is safe to use for retrieval operations.".to_string()
    }

    fn definition(&self) -> FunctionDefinition {
        FunctionDefinition {
            name: self.name(),
            description: self.description(),
            parameters: self.memory.kip_function_definitions.parameters.clone(),
            strict: Some(true),
        }
    }

    async fn call(
        &self,
        _ctx: BaseCtx,
        mut request: Self::Args,
        _resources: Vec<Resource>,
    ) -> Result<ToolOutput<Self::Output>, BoxError> {
        let res = match timeout(
            self.timeout,
            request.readonly().execute(self.memory.nexus.as_ref()),
        )
        .await
        {
            Ok((_, res)) => res,
            Err(_) => Response::err(KipError::new(
                KipErrorCode::ExecutionTimeout,
                format!(
                    "read-only KIP execution timed out after {} seconds; memory is busy, retry later",
                    self.timeout.as_secs()
                ),
            )),
        };

        let is_error = if matches!(res, Response::Err { .. }) {
            Some(true)
        } else {
            None
        };

        let mut output = ToolOutput::new(res);
        output.is_error = is_error;
        Ok(output)
    }
}

#[derive(Clone)]
pub struct RecallAgent {
    pub conversations: Conversations,
    memory: Arc<MemoryManagement>,
    hook: Arc<dyn BrainHook>,
    history: Arc<RwLock<VecDeque<Document>>>,
    max_input_tokens: usize,
}

impl RecallAgent {
    pub const NAME: &'static str = "recall_memory";
    pub fn new(
        memory: Arc<MemoryManagement>,
        conversations: Conversations,
        hook: Arc<dyn BrainHook>,
        max_input_tokens: usize,
    ) -> Self {
        Self {
            conversations,
            memory,
            hook,
            history: Arc::new(RwLock::new(VecDeque::new())),
            max_input_tokens,
        }
    }

    pub async fn init(&self) -> Result<(), BoxError> {
        let (conversations, _) = self
            .conversations
            .list_conversations_by_user(&SELF_USER_ID, None, Some(3))
            .await?;
        // Only completed conversations belong in the model context, matching
        // the runtime push_completed_history behavior. The list is newest
        // first while the runtime queue runs oldest -> newest, so reverse it;
        // otherwise the next push_back would evict the newest entry first.
        *self.history.write() = conversations
            .into_iter()
            .filter(|c| c.status == ConversationStatus::Completed)
            .rev()
            .map(Document::from)
            .collect();
        Ok(())
    }

    pub async fn get_counterparty(&self, counterparty: &str) -> Result<Json, BoxError> {
        let user = self
            .memory
            .nexus
            .get_concept(&ConceptPK::Object {
                r#type: "Person".to_string(),
                name: counterparty.to_string(),
            })
            .await?;

        Ok(user.to_concept_node())
    }
}

/// Implementation of the [`Agent`] trait for RecallAgent.
impl Agent<AgentCtx> for RecallAgent {
    /// Returns the agent's name identifier
    fn name(&self) -> String {
        Self::NAME.to_string()
    }

    /// Returns a description of the agent's purpose and capabilities.
    fn description(&self) -> String {
        FUNCTION_DEFINITION.description.clone()
    }

    fn definition(&self) -> FunctionDefinition {
        FUNCTION_DEFINITION.clone()
    }

    /// Returns a list of tool names that this agent depends on
    fn tool_dependencies(&self) -> Vec<String> {
        vec![MemoryReadonly::NAME.to_string(), NoteTool::NAME.to_string()]
    }

    async fn run(
        &self,
        ctx: AgentCtx,
        prompt: String, // RecallInput serialized as JSON string
        _resources: Vec<Resource>,
    ) -> Result<AgentOutput, BoxError> {
        let caller = ctx.caller();
        let now_ms = unix_ms();
        let token_count = estimate_tokens(&prompt);
        if token_count > self.max_input_tokens {
            return Err(format!(
                "Input too large: {} tokens (estimated), max allowed is {} tokens",
                token_count, self.max_input_tokens
            )
            .into());
        }

        let counterparty = if let Ok(input) = serde_json::from_str::<RecallInput>(&prompt)
            && let Some(ctx) = input.context
            && let Some(counterparty) = ctx.counterparty
        {
            Some(counterparty)
        } else {
            None
        };

        let counterparty_info = if let Some(counterparty) = counterparty {
            match timeout(RECALL_CONTEXT_TIMEOUT, self.get_counterparty(&counterparty)).await {
                Ok(Ok(info)) => Some(info),
                Ok(Err(err)) => {
                    log::debug!(
                        target: "brain",
                        counterparty;
                        "recall counterparty profile not available: {err:?}"
                    );
                    None
                }
                Err(_) => {
                    log::warn!(
                        target: "brain",
                        counterparty;
                        "recall counterparty profile lookup timed out"
                    );
                    None
                }
            }
        } else {
            None
        };

        // add history conversations to provide more context for recall
        let chat_history: Vec<Document> = { self.history.read().iter().cloned().collect() };

        let chat_history = if chat_history.is_empty() {
            vec![]
        } else {
            vec![Message {
                role: "user".into(),
                content: vec![
                    Documents::new("history_recall".to_string(), chat_history)
                        .to_string()
                        .into(),
                ],
                name: Some("$system".into()),
                timestamp: Some(now_ms),
                ..Default::default()
            }]
        };

        let primer = match timeout(RECALL_CONTEXT_TIMEOUT, self.memory.describe_primer()).await {
            Ok(Ok(primer)) => primer,
            Ok(Err(err)) => {
                log::debug!(target: "brain", "recall primer not available: {err:?}");
                Json::default()
            }
            Err(_) => {
                log::warn!(target: "brain", "recall primer lookup timed out");
                Json::default()
            }
        };
        let mut conversation = Conversation {
            user: *caller,
            messages: vec![serde_json::json!(Message {
                role: "user".into(),
                content: vec![prompt.clone().into()],
                timestamp: Some(now_ms),
                ..Default::default()
            })],
            status: ConversationStatus::Working,
            period: now_ms / 3600 / 1000,
            created_at: now_ms,
            updated_at: now_ms,
            label: Some("recall".to_string()),
            ..Default::default()
        };

        let id = self
            .conversations
            .add_conversation(ConversationRef::from(&conversation))
            .await?;
        conversation._id = id;
        let notes = match load_notes(&ctx).await {
            Some(n) => n,
            None => load_notes_from_legacy(&ctx).await.unwrap_or_default(),
        };
        let mut runner = ctx.clone().completion_iter(
            CompletionRequest {
                instructions: format!(
                    "{}\n\n---\n\n# `DESCRIBE PRIMER` Result:\n{}\n\n---\n\n# Your Notes:\n{}\n\n# Counterparty profile:\n{}\n\n# Current Datetime: {}",
                    SELF_INSTRUCTIONS,
                    primer,
                    serde_json::to_string(&notes.items).unwrap_or_default(),
                    serde_json::to_string(&counterparty_info).unwrap_or_default(),
                    local_date_hour(now_ms).unwrap_or_default()
                ),
                prompt,
                chat_history,
                tools: ctx.tool_definitions(Some(&self.tool_dependencies())),
                tool_choice_required: true,
                ..Default::default()
            },
            vec![],
        );

        let mut replace_initial_input = true;
        let mut persisted_runner_history_len = 0;
        let mut last_output: Option<AgentOutput> = None;
        loop {
            match compact_runner_if_needed(&mut runner, 0, true).await {
                Ok(true) => {
                    persisted_runner_history_len = 0;
                    replace_initial_input = false;
                }
                Ok(false) => {}
                Err(err) => {
                    conversation.status = ConversationStatus::Failed;
                    conversation.failed_reason = Some(err.to_string());
                    conversation.updated_at = unix_ms();
                    if let Ok(changes) = conversation.to_changes() {
                        let _ = self
                            .conversations
                            .update_conversation(conversation._id, changes)
                            .await;
                    }
                    self.hook
                        .on_conversation_end(Self::NAME, &conversation)
                        .await;
                    return Err(err);
                }
            }

            match runner.next().await {
                Ok(None) => break,
                Ok(Some(mut output)) => {
                    let is_done = runner.is_done();
                    append_runner_history(
                        &mut conversation,
                        &output.chat_history,
                        &mut persisted_runner_history_len,
                        &mut replace_initial_input,
                    );
                    conversation.status = if output.failed_reason.is_some() {
                        ConversationStatus::Failed
                    } else if is_done {
                        ConversationStatus::Completed
                    } else {
                        ConversationStatus::Working
                    };
                    conversation.usage = output.usage.clone();
                    conversation.updated_at = unix_ms();

                    if let Some(ref failed_reason) = output.failed_reason {
                        conversation.failed_reason = Some(failed_reason.clone());
                    } else {
                        conversation.failed_reason = None;
                        push_completed_history(&self.history, &conversation, 3);
                    }

                    if let Ok(changes) = conversation.to_changes() {
                        let _ = self
                            .conversations
                            .update_conversation(conversation._id, changes)
                            .await;
                    }
                    output.conversation = Some(conversation._id);
                    last_output = Some(output);

                    if conversation.status == ConversationStatus::Failed
                        || conversation.status == ConversationStatus::Completed
                    {
                        break;
                    }
                }
                Err(err) => {
                    conversation.status = ConversationStatus::Failed;
                    conversation.failed_reason = Some(err.to_string());
                    conversation.updated_at = unix_ms();
                    if let Ok(changes) = conversation.to_changes() {
                        let _ = self
                            .conversations
                            .update_conversation(conversation._id, changes)
                            .await;
                    }
                    self.hook
                        .on_conversation_end(Self::NAME, &conversation)
                        .await;
                    return Err(err);
                }
            }
        }

        self.hook
            .on_conversation_end(Self::NAME, &conversation)
            .await;
        last_output.ok_or_else(|| "completion runner returned no output".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{FUNCTION_DEFINITION, READONLY_KIP_TIMEOUT, RecallAgent};
    use crate::{
        agents::SELF_USER_ID,
        space::AppState,
        types::{InputContext, RecallInput},
    };
    use anda_core::{
        Agent, AgentOutput, BoxError, BoxPinFut, CompletionRequest, Message, Principal,
    };
    use anda_db::{database::DBConfig, storage::StorageConfig};
    use anda_engine::{
        context::AgentCtx,
        management::{BaseManagement, Visibility},
        memory::ConversationStatus,
        model::{CompletionFeaturesDyn, Model, Models, reqwest},
    };
    use object_store::memory::InMemory;
    use std::{collections::BTreeSet, sync::Arc};

    #[derive(Debug)]
    struct FinalCompleter;

    impl CompletionFeaturesDyn for FinalCompleter {
        fn model_name(&self) -> String {
            "recall-final-test-model".to_string()
        }

        fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
            Box::pin(async move {
                Ok(AgentOutput {
                    content: "answer".to_string(),
                    chat_history: vec![Message {
                        role: "assistant".to_string(),
                        content: vec![format!("answered: {}", req.prompt).into()],
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            })
        }
    }

    #[derive(Debug)]
    struct FailedReasonCompleter;

    impl CompletionFeaturesDyn for FailedReasonCompleter {
        fn model_name(&self) -> String {
            "recall-failed-reason-test-model".to_string()
        }

        fn completion(&self, _req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
            Box::pin(async move {
                Ok(AgentOutput {
                    failed_reason: Some("recall failed".to_string()),
                    chat_history: vec![Message {
                        role: "assistant".to_string(),
                        content: vec!["recall failure".to_string().into()],
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            })
        }
    }

    #[derive(Debug)]
    struct ErrorCompleter;

    impl CompletionFeaturesDyn for ErrorCompleter {
        fn model_name(&self) -> String {
            "recall-error-test-model".to_string()
        }

        fn completion(&self, _req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
            Box::pin(async move { Err("model error".into()) })
        }
    }

    #[derive(Debug)]
    struct EmptyHistoryCompleter;

    impl CompletionFeaturesDyn for EmptyHistoryCompleter {
        fn model_name(&self) -> String {
            "recall-empty-history-test-model".to_string()
        }

        fn completion(&self, _req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
            Box::pin(async move {
                Ok(AgentOutput {
                    content: "no output".to_string(),
                    ..Default::default()
                })
            })
        }
    }

    fn test_db_config(name: &str) -> DBConfig {
        DBConfig {
            name: name.to_string(),
            description: "test database".to_string(),
            storage: StorageConfig::default(),
            lock: None,
        }
    }

    fn test_app_state_with_completer<C>(name: &str, completer: C) -> AppState
    where
        C: CompletionFeaturesDyn,
    {
        let models = Models::default();
        models.set_model(Model::with_completer(Arc::new(completer)));
        let management = Arc::new(BaseManagement {
            controller: SELF_USER_ID,
            managers: BTreeSet::new(),
            visibility: Visibility::Public,
        });
        let http_client = reqwest::Client::builder().build().unwrap();

        AppState::new(
            Arc::new(InMemory::new()),
            Arc::new(test_db_config(name)),
            management,
            http_client,
            Arc::new(models),
            Arc::new(vec![]),
            "anda_brain".to_string(),
            "test".to_string(),
            0,
        )
    }

    async fn create_loaded_space(app: &AppState, id: &str) -> Arc<crate::space::Space> {
        app.admin_create_space(
            Principal::from_slice(&[1]),
            Principal::from_slice(&[2]),
            id.to_string(),
            1,
            123,
        )
        .await
        .unwrap();

        app.load_space(id, false).await.unwrap()
    }

    fn recall_prompt(query: &str, counterparty: Option<&str>) -> String {
        serde_json::to_string(&RecallInput {
            query: query.to_string(),
            context: counterparty.map(|counterparty| InputContext {
                counterparty: Some(counterparty.to_string()),
                ..Default::default()
            }),
        })
        .unwrap()
    }

    #[test]
    fn recall_function_definition_matches_agent_contract() {
        assert_eq!(RecallAgent::NAME, "recall_memory");
        assert_eq!(FUNCTION_DEFINITION.name, RecallAgent::NAME);
        assert_eq!(FUNCTION_DEFINITION.strict, Some(true));
        assert_eq!(
            FUNCTION_DEFINITION
                .parameters
                .pointer("/properties/query/type")
                .and_then(|v| v.as_str()),
            Some("string")
        );
        assert_eq!(
            FUNCTION_DEFINITION
                .parameters
                .pointer("/required")
                .and_then(|v| v.as_array())
                .map(|values| values.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>()),
            Some(vec!["query", "context"])
        );
    }

    #[test]
    fn readonly_kip_timeout_stays_bounded() {
        assert_eq!(READONLY_KIP_TIMEOUT.as_secs(), 15);
    }

    #[tokio::test]
    async fn recall_agent_trait_metadata_matches_function_definition() {
        let app = test_app_state_with_completer("recall_trait_metadata", FinalCompleter);
        let space = create_loaded_space(&app, "recall_trait_metadata").await;

        assert_eq!(
            Agent::<AgentCtx>::name(space.recall.as_ref()),
            RecallAgent::NAME
        );
        assert_eq!(
            Agent::<AgentCtx>::description(space.recall.as_ref()),
            FUNCTION_DEFINITION.description
        );
        assert_eq!(
            Agent::<AgentCtx>::definition(space.recall.as_ref()).name,
            RecallAgent::NAME
        );
        let tools = Agent::<AgentCtx>::tool_dependencies(space.recall.as_ref());
        assert!(tools.iter().any(|name| name == "execute_kip_readonly"));
        assert!(tools.iter().any(|name| name == "note"));
    }

    #[tokio::test]
    async fn recall_run_uses_history_and_tolerates_missing_counterparty_profile() {
        let app = test_app_state_with_completer("recall_history", FinalCompleter);
        let space = create_loaded_space(&app, "recall_history").await;
        let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();

        let first = Agent::<AgentCtx>::run(
            space.recall.as_ref(),
            ctx.clone(),
            recall_prompt("what is remembered?", None),
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(first.conversation, Some(1));

        let second = Agent::<AgentCtx>::run(
            space.recall.as_ref(),
            ctx,
            recall_prompt("what about this missing person?", Some("missing-person")),
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(second.conversation, Some(2));

        let stored = space
            .get_conversation(Some("recall".to_string()), 2)
            .await
            .unwrap();
        assert_eq!(stored.status, ConversationStatus::Completed);
    }

    #[tokio::test]
    async fn recall_run_persists_model_failed_reason_and_model_errors() {
        let app = test_app_state_with_completer("recall_failed_reason", FailedReasonCompleter);
        let space = create_loaded_space(&app, "recall_failed_reason").await;
        let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
        let output = Agent::<AgentCtx>::run(
            space.recall.as_ref(),
            ctx,
            recall_prompt("fail this recall", None),
            vec![],
        )
        .await
        .unwrap();
        let conversation_id = output.conversation.unwrap();
        let stored = space
            .get_conversation(Some("recall".to_string()), conversation_id)
            .await
            .unwrap();
        assert_eq!(stored.status, ConversationStatus::Failed);
        assert_eq!(stored.failed_reason.as_deref(), Some("recall failed"));

        let app = test_app_state_with_completer("recall_model_error", ErrorCompleter);
        let space = create_loaded_space(&app, "recall_model_error").await;
        let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
        let err = Agent::<AgentCtx>::run(
            space.recall.as_ref(),
            ctx,
            recall_prompt("error this recall", None),
            vec![],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("model error"));

        let stored = space
            .get_conversation(Some("recall".to_string()), 1)
            .await
            .unwrap();
        assert_eq!(stored.status, ConversationStatus::Failed);
        assert!(
            stored
                .failed_reason
                .as_deref()
                .unwrap()
                .contains("model error")
        );
    }

    #[tokio::test]
    async fn recall_run_preserves_input_when_chat_history_is_empty() {
        let app = test_app_state_with_completer("recall_empty_history", EmptyHistoryCompleter);
        let space = create_loaded_space(&app, "recall_empty_history").await;
        let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();

        let output = Agent::<AgentCtx>::run(
            space.recall.as_ref(),
            ctx,
            recall_prompt("anything stored?", None),
            vec![],
        )
        .await
        .unwrap();

        let stored = space
            .get_conversation(Some("recall".to_string()), output.conversation.unwrap())
            .await
            .unwrap();
        assert_eq!(stored.status, ConversationStatus::Completed);
        // The anomalous empty model output must not erase the original input.
        assert_eq!(stored.messages.len(), 1);
    }

    #[tokio::test]
    async fn recall_run_rejects_oversized_input_before_persisting() {
        let app = test_app_state_with_completer("recall_input_too_large", FinalCompleter);
        let space = create_loaded_space(&app, "recall_input_too_large").await;
        let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
        let prompt = "x ".repeat(1_000_000);

        let err = Agent::<AgentCtx>::run(space.recall.as_ref(), ctx, prompt, vec![])
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Input too large"));
        assert_eq!(space.recall.conversations.conversations.len(), 0);
    }
}
