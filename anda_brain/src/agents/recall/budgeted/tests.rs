use super::*;
use crate::PROFILE;
use crate::{
    payload::{ContentType, StringOr},
    recall_budget::MemoryPacket,
    space::{AppState, Space},
    testkit::{app_state_core, create_loaded_space, models_with_completer},
    types::UpdateSpaceInput,
};
use anda_core::{BoxPinFut, ToolCall};
use anda_engine::model::CompletionFeaturesDyn;
use axum::response::IntoResponse;
use parking_lot::Mutex;

const LEAK: &str = "provider-private-data-must-not-escape";

#[tokio::test]
async fn product_changes_exclude_history_from_budgeted_planning_and_delivery() {
    let (_, space, seen) = setup("budget_product_history", Behavior::Select).await;
    let history = Conversation {
        status: ConversationStatus::Completed,
        messages: vec![json!(Message {
            role: "assistant".into(),
            content: vec!["removed-product-memory".to_string().into()],
            ..Default::default()
        })],
        ..Default::default()
    };
    let history = Document::from(history);
    space.recall.history.write().push_back(history.clone());
    space
        .query(SELF_USER_ID, input(Some(limits(8192, 131_072))))
        .await
        .unwrap();
    assert!(seen.lock()[0].prompt.contains("removed-product-memory"));

    // Keep the old history present, as it also is after loading stored conversations.
    *space.recall.history.write() = VecDeque::from([history]);
    let mut state = space.product_control.snapshot();
    state.epoch += 1;
    space.product_control.save(state).await.unwrap();
    seen.lock().clear();
    let output = space
        .query(SELF_USER_ID, input(Some(limits(8192, 131_072))))
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    assert!(!output.content.contains("removed-product-memory"));
    assert!(
        seen.lock()
            .iter()
            .all(|request| !request.prompt.contains("removed-product-memory"))
    );
    space.close().await.unwrap();
}

#[tokio::test]
async fn references_are_budgeted_planning_context_never_memory_or_coverage() {
    let reference_call = ToolCall {
        name: "kip_reference".into(),
        args: json!({"document":"syntax","section":"kql","offset":0}),
        ..Default::default()
    };
    let (_, space, seen) = setup(
        "reference_planning_context",
        Behavior::ReadThenSelect(vec![reference_call.clone()]),
    )
    .await;
    let output = space
        .query(SELF_USER_ID, input(Some(limits(8192, 131_072))))
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    let requests = seen.lock().clone();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request.instructions.matches(anda_kip::KIP_SYNTAX).count(),
            1
        );
        assert!(
            request.instructions.find("# Host budget mode").unwrap()
                > request.instructions.find(anda_kip::KIP_SYNTAX).unwrap()
        );
    }
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "kip_reference")
    );
    assert!(
        !requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "memory_runtime")
    );
    let prompt: Json = serde_json::from_str(&requests[1].prompt).unwrap();
    let reference = prompt["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item.get("reference").is_some())
        .unwrap();
    assert_eq!(reference["reference"]["document"], "syntax");
    assert!(
        reference["reference"]["content"]
            .as_str()
            .unwrap()
            .contains("KQL")
    );
    let packet: MemoryPacket = serde_json::from_str(&output.content).unwrap();
    assert!(
        packet
            .items
            .iter()
            .all(|item| item.content.get("crate_version").is_none())
    );
    let initial: Json = serde_json::from_str(&requests[0].prompt).unwrap();
    assert_eq!(initial["coverage"], prompt["coverage"]);
    // Leave enough cumulative input budget for the first call, but not even
    // the fixed second-pass instructions + reference (with all memories evicted).
    let first = budget::count(&normalized_request(&requests[0]).unwrap()).unwrap();
    space.close().await.unwrap();

    let (_, space, seen) = setup(
        "reference_context_exhaustion",
        Behavior::ReadThenSelect(vec![reference_call]),
    )
    .await;
    let output = space
        .query(SELF_USER_ID, input(Some(limits(8192, first as u32 + 256))))
        .await
        .unwrap();
    assert_eq!(seen.lock().len(), 1);
    assert!(output.failed_reason.is_none());
    let packet = check_output(&output, 8192).unwrap();
    assert!(
        packet
            .items
            .iter()
            .any(|item| item.content["reason"] == "recall_context_budget_exhausted")
    );
    space.close().await.unwrap();
}

#[tokio::test]
async fn readonly_model_batches_use_the_schema_without_an_execution_field() {
    let args = json!({"operations":[
        "FIND(?c.id) WHERE {?c CONCEPT {type:\"Person\"}} LIMIT 1",
        "FIND(?c.id) WHERE {?c CONCEPT {type:\"Insight\"}} LIMIT 1"
    ]});
    let (_, space, _) = setup(
        "readonly_batch_contract",
        Behavior::ReadThenSelect(vec![ToolCall {
            name: "execute_kip_readonly".into(),
            args: args.clone(),
            ..Default::default()
        }]),
    )
    .await;
    let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
    let ordinary = anda_core::Tool::call(
        &TimedMemoryReadonly::new(space.memory.clone()),
        ctx.base,
        serde_json::from_value(args).unwrap(),
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(ordinary.output.status, anda_kip::TopLevelStatus::Succeeded);
    assert_eq!(ordinary.output.results.len(), 2);
    let output = space
        .query(
            SELF_USER_ID,
            StringOr::Value(RecallInput {
                query: "Read both record types".into(),
                // This contract needs a read pass and a selection pass. Keep
                // the independent cumulative-context budget large enough for both.
                budget: Some(RecallBudget {
                    context_tokens: 131_072,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    let packet: MemoryPacket = serde_json::from_str(&output.content).unwrap();
    let response = packet
        .items
        .iter()
        .find(|item| item.content.get("kip").is_some())
        .unwrap();
    assert_eq!(response.content["status"], "succeeded");
    assert_eq!(response.content["results"].as_array().unwrap().len(), 2);
    space.close().await.unwrap();
}

#[test]
fn query_limits_never_raise_literal_or_parameter_bounds_and_expired_flags_are_removed() {
    let small = Scalar::Literal(KipValue::Number(1.into()));
    assert_eq!(capped_limit(Some(&small), None, None).unwrap(), small);
    let parameter = Scalar::Param("n".into());
    let global = serde_json::from_value(json!({"n":2})).unwrap();
    let local = serde_json::from_value(json!({"n":1})).unwrap();
    assert_eq!(
        capped_limit(Some(&parameter), Some(&local), Some(&global)).unwrap(),
        small
    );
    assert!(capped_limit(Some(&parameter), None, None).is_err());
    let mut items = vec![MemoryItem {
        id: "p".into(),
        channel: Channel::Procedures,
        priority: Priority::Required,
        content: json!({"recommendation_allowed":true,"context_expires_at_ms":0,"review_due_at":"2099-01-01T00:00:00.000Z","reasons":[]}),
    }];
    assert!(expire_procedure_checks(&mut items, unix_ms(), false));
    assert_eq!(items[0].content["recommendation_allowed"], false);
    assert_eq!(items[0].priority, Priority::Warning);
}

#[derive(Clone, Debug)]
enum Behavior {
    Select,
    RejectOutputLimit,
    ProviderFailure,
    ReadThenSelect(Vec<ToolCall>),
    LegacyOrSelect,
}

#[derive(Debug)]
struct Planner {
    behavior: Behavior,
    requests: Arc<Mutex<Vec<CompletionRequest>>>,
}

impl CompletionFeaturesDyn for Planner {
    fn model_name(&self) -> String {
        "bounded-recall-contract-fixture".into()
    }

    fn completion(&self, request: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
        let behavior = self.behavior.clone();
        let requests = self.requests.clone();
        Box::pin(async move {
            let call = {
                let mut seen = requests.lock();
                let call = seen.len();
                seen.push(request.clone());
                call
            };
            match &behavior {
                Behavior::RejectOutputLimit if request.max_output_tokens.is_some() => {
                    return Err("Unsupported parameter: max_output_tokens".into());
                }
                Behavior::ProviderFailure => return Err(LEAK.into()),
                _ => {}
            }
            let prompt: Json = serde_json::from_str(&request.prompt).unwrap_or(Json::Null);
            let ids = prompt["memory_items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["id"].as_str().map(str::to_string))
                .chain(std::iter::once("model-invented-verified-id".to_string()))
                .collect::<Vec<_>>();
            let mut output = AgentOutput {
                content: json!({"selected_ids":ids}).to_string(),
                thoughts: Some(LEAK.repeat(1000)),
                chat_history: vec![Message {
                    role: "assistant".into(),
                    content: vec![LEAK.repeat(1000).into()],
                    ..Default::default()
                }],
                raw_history: vec![json!({"provider_state":LEAK.repeat(1000)})],
                artifacts: vec![Resource {
                    name: LEAK.into(),
                    blob: Some(LEAK.as_bytes().to_vec().into()),
                    ..Default::default()
                }],
                session: Some(LEAK.into()),
                model: Some(LEAK.into()),
                usage: Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                    requests: 1,
                    ..Default::default()
                },
                ..Default::default()
            };
            match behavior {
                Behavior::ReadThenSelect(tools) if call == 0 => output.tool_calls = tools,
                Behavior::LegacyOrSelect if prompt.get("memory_items").is_none() => {
                    output.content = "legacy answer".into();
                }
                _ => {}
            }
            Ok(output)
        })
    }
}

async fn setup(
    name: &str,
    behavior: Behavior,
) -> (AppState, Arc<Space>, Arc<Mutex<Vec<CompletionRequest>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = app_state_core(
        name,
        models_with_completer(Planner {
            behavior,
            requests: requests.clone(),
        }),
        vec![],
        "p5-contract-test",
        0,
    );
    let space = create_loaded_space(&app, name).await;
    (app, space, requests)
}

fn limits(max_tokens: u32, context_tokens: u32) -> RecallBudget {
    RecallBudget {
        max_tokens,
        context_tokens,
        ..Default::default()
    }
}

fn input(budget: Option<RecallBudget>) -> StringOr<RecallInput> {
    StringOr::Value(RecallInput {
        query: "Select the relevant authorized memories and retain constraints.".into(),
        context: None,
        budget,
    })
}

fn tool(name: &str, command: &str) -> ToolCall {
    ToolCall {
        name: name.into(),
        args: json!({"command":command}),
        call_id: Some("fixture-call".into()),
        ..Default::default()
    }
}

fn check_output(output: &AgentOutput, max_tokens: u32) -> Option<MemoryPacket> {
    assert!(budget::count(&output.content).unwrap() <= max_tokens as usize);
    assert!(output.thoughts.is_none());
    assert!(output.chat_history.is_empty());
    assert!(output.raw_history.is_empty());
    assert!(output.tool_calls.is_empty());
    assert!(output.artifacts.is_empty());
    assert!(output.session.is_none());
    assert!(!serde_json::to_string(output).unwrap().contains(LEAK));
    assert!(
        output.failed_reason.is_none()
            || output
                .failed_reason
                .as_deref()
                .is_some_and(|r| r.starts_with("recall_") && r.len() <= 64)
    );
    let packet: Option<MemoryPacket> = serde_json::from_str(&output.content).unwrap();
    if let Some(packet) = &packet {
        assert!(!packet.semantic_complete);
        assert!(!packet.action_ready);
        assert_eq!(packet.token_limit, max_tokens);
    } else {
        assert_eq!(output.content, "null");
        assert!(output.failed_reason.is_some());
    }
    packet
}

async fn seed(space: &Space, command: &str, parameters: serde_json::Map<String, Json>) -> Json {
    let result = anda_kip::execute_request(
        space.memory.nexus().as_ref(),
        &kip::request_with(command, parameters),
    )
    .await;
    assert!(
        kip::succeeded(&result),
        "{}",
        serde_json::to_string(&result).unwrap()
    );
    result.first_result().unwrap().clone()
}

#[tokio::test]
async fn tiny_output_or_context_budget_never_calls_provider_or_returns_side_channels() {
    let (_app, space, requests) = setup("p5_tiny_budget", Behavior::Select).await;
    space
        .recall
        .history
        .write()
        .push_back(Document::from_text("old", LEAK));
    for budget in [limits(1, 131_072), limits(65_536, 1)] {
        let output = space
            .query(SELF_USER_ID, input(Some(budget.clone())))
            .await
            .unwrap();
        let packet = check_output(&output, budget.max_tokens);
        if budget.max_tokens == 1 {
            assert!(output.failed_reason.is_some());
            if let Some(packet) = packet {
                assert_eq!(packet.status, "budget_insufficient");
                assert!(packet.items.is_empty());
            }
        } else {
            assert!(output.failed_reason.is_none());
            assert!(
                packet
                    .unwrap()
                    .items
                    .iter()
                    .any(|item| item.content["reason"] == "recall_context_budget_exhausted")
            );
        }
    }
    assert!(requests.lock().is_empty());
    space.close().await.unwrap();
}

#[tokio::test]
async fn actual_planner_requests_and_final_packets_obey_the_same_pinned_encoding() {
    let (_app, space, requests) = setup("p5_select_packet", Behavior::Select).await;
    let budget = RecallBudget::default();
    let output = space
        .query(SELF_USER_ID, input(Some(budget.clone())))
        .await
        .unwrap();
    let packet = check_output(&output, budget.max_tokens).unwrap();
    assert!(output.failed_reason.is_none(), "{}", output.content);
    let seen = requests.lock().clone();
    assert_eq!(seen.len(), 1);
    for request in &seen {
        assert!(
            budget::count(&normalized_request(request).unwrap()).unwrap()
                <= budget.context_tokens as usize
        );
        assert!(request.chat_history.is_empty());
        assert!(request.raw_history.is_empty());
        assert!(request.documents.is_empty());
        assert!(request.content.is_empty());
        assert!(
            request
                .tools
                .iter()
                .any(|definition| definition.name == SELECT)
        );
    }
    let snapshot: Json = serde_json::from_str(&seen[0].prompt).unwrap();
    let admitted: Vec<MemoryItem> =
        serde_json::from_value(snapshot["memory_items"].clone()).unwrap();
    assert!(!packet.items.is_empty());
    for item in &packet.items {
        assert!(
            admitted.contains(item),
            "model cannot synthesize or promote an item"
        );
    }
    assert!(!output.content.contains("model-invented-verified-id"));

    // A budget that could hold this request without syntax must still fail
    // before the provider if it cannot hold even the full fixed instructions.
    let mut without_syntax = seen[0].clone();
    assert_eq!(
        without_syntax
            .instructions
            .matches(anda_kip::KIP_SYNTAX)
            .count(),
        1
    );
    without_syntax.instructions = without_syntax
        .instructions
        .replace(anda_kip::KIP_SYNTAX, "");
    let smaller_budget = budget::count(&normalized_request(&without_syntax).unwrap()).unwrap();
    let mut fixed_only = seen[0].clone();
    fixed_only.prompt.clear();
    assert!(smaller_budget < budget::count(&normalized_request(&fixed_only).unwrap()).unwrap());
    let rejected = space
        .query(
            SELF_USER_ID,
            input(Some(limits(budget.max_tokens, smaller_budget as u32))),
        )
        .await
        .unwrap();
    assert!(rejected.failed_reason.is_none());
    let packet = check_output(&rejected, budget.max_tokens).unwrap();
    assert!(
        packet
            .items
            .iter()
            .any(|item| item.content["reason"] == "recall_context_budget_exhausted")
    );
    assert_eq!(requests.lock().len(), seen.len());
    space.close().await.unwrap();
}

#[tokio::test]
async fn long_real_evidence_notes_and_history_are_removed_atomically_before_the_next_model_call() {
    let (_app, space, requests) = setup(
        "p5_large_channels",
        Behavior::ReadThenSelect(vec![tool(
            "execute_kip_readonly",
            "FIND(?e) WHERE {?e EVIDENCE {}} LIMIT 32",
        )]),
    )
    .await;
    // Two fresh planner requests share this cumulative context allowance.
    // Either large source alone exceeds it, while both native stored payloads
    // remain below the per-item byte cap.
    let context_limit = 131_072;
    let long_evidence = "证据原文🧑🏽‍🚀不得截断否定。 ".repeat(12000);
    assert!(budget::count(&long_evidence).unwrap() > context_limit as usize);
    seed(
        &space,
        r#"CREATE EVIDENCE ?e { SET FIELDS {evidence_class:"agent_statement",payload: :payload} }"#,
        kip::param("payload", long_evidence.clone()),
    )
    .await;
    let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
    let notes = "长备注：核实前不可行动。 ".repeat(500);
    let written = ctx
        .tool_call(ToolInput {
            name: "note".into(),
            args: json!({"op":"set","items":[{"id":"long-note","content":notes}]}),
            resources: vec![],
            meta: None,
        })
        .await
        .unwrap();
    assert_ne!(written.is_error, Some(true));
    space.recall.history.write().push_back(Document::from_text(
        "large-history",
        "历史不表示当前有效。 ".repeat(20_000),
    ));

    let output = space
        .query(SELF_USER_ID, input(Some(limits(65_536, context_limit))))
        .await
        .unwrap();
    let packet = check_output(&output, 65_536).unwrap();
    assert!(output.failed_reason.is_none(), "{}", output.content);
    let seen = requests.lock().clone();
    assert_eq!(
        seen.len(),
        2,
        "there is no provider-controlled automatic expansion loop"
    );
    for request in &seen {
        assert!(
            budget::count(&normalized_request(request).unwrap()).unwrap() <= context_limit as usize
        );
    }
    let cumulative_tokens: usize = seen
        .iter()
        .map(|request| budget::count(&normalized_request(request).unwrap()).unwrap())
        .sum();
    assert!(cumulative_tokens <= context_limit as usize);
    assert!(packet.coverage.partial.contains(&Channel::Kip));
    assert!(packet.coverage.omitted.contains(&Channel::Kip));
    assert!(
        packet.coverage.omitted.contains(&Channel::Notes)
            || packet.coverage.omitted.contains(&Channel::History)
    );
    let final_snapshot: Json = serde_json::from_str(&seen[1].prompt).unwrap();
    let admitted: Vec<MemoryItem> =
        serde_json::from_value(final_snapshot["memory_items"].clone()).unwrap();
    for item in &packet.items {
        assert!(admitted.contains(item));
    }
    let prefix: String = long_evidence.chars().take(100).collect();
    assert!(!output.content.contains(&prefix));
    // The source itself remains intact: budgeting is a delivery operation.
    let stored = space
        .execute_kip_readonly(kip::request("FIND(?e) WHERE {?e EVIDENCE {}} LIMIT 32"))
        .await
        .unwrap();
    assert!(
        serde_json::to_string(&stored)
            .unwrap()
            .contains(&long_evidence)
    );
    space.close().await.unwrap();
}

#[tokio::test]
async fn model_tool_injection_and_too_many_calls_cannot_execute_a_native_write() {
    let write = r#"CREATE CONCEPT ?bad {TYPE "Insight" NAME "injected-budget-write" SET ATTRIBUTES {summary:"injected"}}"#;
    let scenarios = [
        ("p5_forbidden_tool", vec![tool("execute_kip", write)], true),
        (
            "p5_kml_in_readonly",
            vec![tool("execute_kip_readonly", write)],
            false,
        ),
        (
            "p5_too_many_tools",
            vec![tool("execute_kip_readonly", write); MAX_CALLS_PER_TURN + 1],
            true,
        ),
    ];
    for (name, calls, must_fail) in scenarios {
        let (_app, space, requests) = setup(name, Behavior::ReadThenSelect(calls)).await;
        let output = space
            .query(SELF_USER_ID, input(Some(limits(65_536, 131_072))))
            .await
            .unwrap();
        let packet = check_output(&output, 65_536).unwrap();
        let response = space.execute_kip_readonly(kip::request(
            r#"FIND(?c) WHERE {?c CONCEPT {type:"Insight",name:"injected-budget-write"}} LIMIT 5"#,
        )).await.unwrap();
        assert!(kip::succeeded(&response));
        assert!(
            response
                .first_result()
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        if must_fail {
            assert_eq!(packet.status, "budget_insufficient");
            assert!(packet.items.is_empty());
            assert_eq!(requests.lock().len(), 1);
        } else {
            // A denied read becomes an explicit host warning, never an empty
            // successful retrieval. The planner may then select the warning.
            assert!(packet.coverage.partial.contains(&Channel::Kip));
            assert!(
                packet
                    .items
                    .iter()
                    .any(|item| item.channel == Channel::Kip && item.priority == Priority::Warning)
            );
            assert_eq!(requests.lock().len(), 2);
        }
        space.close().await.unwrap();
    }
}

#[tokio::test]
async fn legacy_calls_remain_legacy_until_space_policy_enforces_non_expandable_caps() {
    let (_app, space, requests) = setup("p5_legacy_policy", Behavior::LegacyOrSelect).await;
    let legacy = space.query(SELF_USER_ID, input(None)).await.unwrap();
    assert_eq!(legacy.content, "legacy answer");
    assert_eq!(requests.lock().len(), 1);
    space
        .update(
            UpdateSpaceInput {
                memory_policy: Some(MemoryPolicy {
                    recall_budget: Some(limits(1, 1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
            unix_ms(),
        )
        .await
        .unwrap();
    let expanded = space
        .query(SELF_USER_ID, input(Some(limits(65_536, 131_072))))
        .await
        .unwrap();
    check_output(&expanded, 1);
    assert_eq!(expanded.content, "null");
    let omitted = space.query(SELF_USER_ID, input(None)).await.unwrap();
    check_output(&omitted, 1);
    assert_eq!(omitted.content, "null");
    assert_eq!(
        requests.lock().len(),
        1,
        "a request cannot lift or omit the operator cap"
    );

    let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
    let error = Agent::<AgentCtx>::run(
        space.recall.as_ref(),
        ctx,
        r#"{"query":"malformed budget must not become prose","budget":{"max_tokens":0}}"#.into(),
        vec![],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("max_tokens"));
    assert_eq!(requests.lock().len(), 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn unreadable_notes_are_reported_unavailable_not_empty() {
    use anda_core::{Path, PutMode, StoreFeatures};

    let (_app, space, requests) = setup("recall_unreadable_notes", Behavior::LegacyOrSelect).await;
    let budget = limits(8192, 131_072);
    let output = space
        .query(SELF_USER_ID, input(Some(budget.clone())))
        .await
        .unwrap();
    let packet = check_output(&output, 8192).unwrap();
    assert!(packet.coverage.queried.contains(&Channel::Notes));
    assert!(!packet.coverage.omitted.contains(&Channel::Notes));

    // The note store exists but no longer decodes.
    let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
    let notes = ctx.child_base("note").unwrap();
    notes
        .store_put(
            &Path::from(notes.agent.as_str()),
            PutMode::Overwrite,
            vec![0xff].into(),
        )
        .await
        .unwrap();
    let output = space
        .query(SELF_USER_ID, input(Some(budget)))
        .await
        .unwrap();
    let packet = check_output(&output, 8192).unwrap();
    assert!(
        packet.coverage.omitted.contains(&Channel::Notes),
        "{:?}",
        packet.coverage
    );

    requests.lock().clear();
    let legacy = space.query(SELF_USER_ID, input(None)).await.unwrap();
    assert_eq!(legacy.content, "legacy answer");
    let instructions = requests.lock()[0].instructions.clone();
    assert!(instructions.contains(r#""notes":"unavailable""#));
    assert!(!instructions.contains("# Your Notes:\n[]"));
    space.close().await.unwrap();
}

#[tokio::test]
async fn structured_and_markdown_surfaces_share_one_budgeted_semantic_packet() {
    let (_app, space, requests) = setup("p5_structured_packet", Behavior::Select).await;
    let budget = limits(65_536, 131_072);
    let output = space
        .query_structured(SELF_USER_ID, input(Some(budget.clone())))
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{}", output.answer);
    let packet: MemoryPacket = serde_json::from_str(&output.answer).unwrap();
    assert!(!packet.semantic_complete && !packet.action_ready);
    assert!(
        output.memories.is_empty(),
        "legacy trace citations must not add another memory channel"
    );
    assert!(output.uncertainty.is_none());
    let receipt = output.memory_budget.as_ref().unwrap();
    assert_eq!(receipt.tokenizer, budget.tokenizer);
    assert_eq!(receipt.token_limit, budget.max_tokens);
    assert_eq!(receipt.context_token_limit, budget.context_tokens);
    assert_eq!(receipt.tokens, budget::count(&output.answer).unwrap());
    assert!(receipt.tokens <= budget.max_tokens as usize);
    assert!(!serde_json::to_string(&output).unwrap().contains(LEAK));
    let response = ContentType::Markdown(true)
        .response(output.answer.clone())
        .into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4 * MAX_BYTES)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), output.answer);
    assert_eq!(requests.lock().len(), 1);

    let failed = space
        .query_structured(SELF_USER_ID, input(Some(limits(1, 1))))
        .await
        .unwrap();
    assert_eq!(failed.answer, "null");
    assert!(!failed.found);
    assert!(failed.memories.is_empty());
    assert!(failed.failed_reason.is_some());
    assert_eq!(failed.memory_budget.unwrap().tokens, 1);
    assert_eq!(requests.lock().len(), 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn r6_recall_delivery_receipts_bind_actual_versions_without_graph_writes() {
    let (_app, space, _) = setup(
        "r6_recall_receipt",
        Behavior::ReadThenSelect(vec![tool(
            "execute_kip_readonly",
            r#"FIND(?c) WHERE {?c CONCEPT {type:"Person",name:"receipt-person"}} LIMIT 1"#,
        )]),
    )
    .await;
    seed(
        &space,
        r#"CREATE CONCEPT ?c {TYPE "Person" NAME "receipt-person"}"#,
        serde_json::Map::new(),
    )
    .await;
    let nexus = space.memory.nexus();
    let seq = nexus
        .store
        .get_space(anda_cognitive_nexus::nexus::DEFAULT_SPACE)
        .await
        .unwrap()
        .seq;
    let output = space
        .query_structured(SELF_USER_ID, input(Some(limits(65_536, 131_072))))
        .await
        .unwrap();
    let reference = output.recall_receipt.clone().unwrap();
    let receipt = space.recall_receipts().read(&reference).await.unwrap();
    assert_eq!(
        receipt.packet_digest,
        anda_cognitive_nexus::content_digest(&json!(output.answer)).unwrap()
    );
    assert_eq!(receipt.delivery, "bounded_packet");
    assert!(
        receipt
            .pins
            .iter()
            .any(|p| p.id.starts_with("C-") && p.version > 0)
    );
    assert!(!receipt.semantic_complete && !receipt.action_ready);
    assert_eq!(
        nexus
            .store
            .get_space(anda_cognitive_nexus::nexus::DEFAULT_SPACE)
            .await
            .unwrap()
            .seq,
        seq
    );
    assert_eq!(
        space
            .recall_receipts()
            .for_conversation(output.conversation.unwrap())
            .await
            .unwrap(),
        Some(reference.clone())
    );
    let mut forged = reference;
    forged.digest =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".into();
    assert!(space.recall_receipts().read(&forged).await.is_err());
    space.close().await.unwrap();
}

#[tokio::test]
async fn effective_budget_is_persisted_and_success_updates_the_history_ring() {
    let (_app, space, _requests) = setup("p5_effective_budget", Behavior::Select).await;
    let enforced = limits(4096, 131_072);
    space
        .update(
            UpdateSpaceInput {
                memory_policy: Some(MemoryPolicy {
                    recall_budget: Some(enforced.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            unix_ms(),
        )
        .await
        .unwrap();

    let ctx = space.ctx_for_test(SELF_USER_ID, RecallAgent::NAME).unwrap();
    let output = Agent::<AgentCtx>::run(
        space.recall.as_ref(),
        ctx,
        input(Some(limits(65_536, 131_072))).to_string(),
        vec![],
    )
    .await
    .unwrap();
    let id = output.conversation.unwrap();
    assert_eq!(
        space.recall.conversation_budget(id).await.unwrap(),
        Some(enforced)
    );
    assert_eq!(space.recall.history.read().len(), 1);

    space.recall.history.write().clear();
    space.recall.init().await.unwrap();
    assert_eq!(space.recall.history.read().len(), 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn budgeted_tool_results_reach_the_usage_ledger() {
    let (_app, space, _requests) = setup(
        "p5_usage_ledger",
        Behavior::ReadThenSelect(vec![tool(
            "execute_kip_readonly",
            r#"FIND(?c) WHERE {?c CONCEPT {type:"Person",name:"budget-ledger-person"}} LIMIT 1"#,
        )]),
    )
    .await;
    seed(
        &space,
        r#"CREATE CONCEPT ?c {TYPE "Person" NAME "budget-ledger-person"}"#,
        serde_json::Map::new(),
    )
    .await;

    let output = space
        .query(SELF_USER_ID, input(Some(limits(65_536, 131_072))))
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    let metrics: crate::types::MemoryMetrics = space.db.get_extension_as("memory_metrics").unwrap();
    assert_eq!(metrics.recalls_completed, 1);
    assert_eq!(metrics.entities_recalled, 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn real_commitment_that_cannot_fit_prevents_an_ordinary_answer_or_model_call() {
    let (_app, space, requests) = setup("p5_required_commitment", Behavior::Select).await;
    let summary = "脑 ".repeat(100_000);
    assert!(budget::count(&summary).unwrap() > 131_072);
    seed(&space, r#"CREATE CONCEPT ?c {TYPE "Commitment" SET ATTRIBUTES {summary: :summary,status:"pending"}}"#,
        kip::param("summary", summary)).await;
    let output = space
        .query(SELF_USER_ID, input(Some(limits(65_536, 131_072))))
        .await
        .unwrap();
    let packet = check_output(&output, 65_536).unwrap();
    assert_eq!(packet.status, "budget_insufficient");
    assert!(packet.items.is_empty());
    assert!(output.failed_reason.is_some());
    assert!(requests.lock().is_empty());
    // The constraint window was actually queried, not replaced by a model's
    // assertion that no applicable restrictions were found.
    assert!(packet.coverage.queried.contains(&Channel::Kip));
    space.close().await.unwrap();
}

#[tokio::test]
async fn budgeted_recall_accepts_backends_without_an_output_limit_parameter() {
    let (_app, space, requests) =
        setup("provider_output_defaults", Behavior::RejectOutputLimit).await;
    let output = space
        .query(SELF_USER_ID, input(Some(limits(8192, 131_072))))
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    let packet = check_output(&output, 8192).unwrap();
    assert!(packet.failed_reason.is_none());
    assert_eq!(requests.lock().len(), 1);
    assert!(requests.lock()[0].max_output_tokens.is_none());
    space.close().await.unwrap();
}

#[tokio::test]
async fn provider_failure_reason_reaches_the_packet_without_private_diagnostics() {
    let (_app, space, requests) = setup("provider_failure_packet", Behavior::ProviderFailure).await;
    let output = space
        .query(SELF_USER_ID, input(Some(limits(8192, 131_072))))
        .await
        .unwrap();
    let packet = check_output(&output, 8192).unwrap();
    assert_eq!(
        output.failed_reason.as_deref(),
        Some("recall_model_unavailable")
    );
    assert_eq!(packet.failed_reason, output.failed_reason);
    assert!(packet.items.is_empty());
    assert!(!output.content.contains(LEAK));
    assert_eq!(requests.lock().len(), 1);
    space.close().await.unwrap();
}

#[test]
fn compact_views_keep_constraints_and_provenance_without_duplicate_legacy_rows() {
    let metadata =
        json!({"source":"recorded conversation","observed_at":"2026-06-01T00:00:00.000Z"});
    let original = json!({"id":"C-10","kind":"concept","schema_ref":format!("{PROFILE}Commitment"),
        "_system":{"version":1},"attributes":{"summary":"Never publish without approval","description":"Never publish without approval",
        "status":"pending","approval_required":true,"legacy":{"id":9,"metadata":metadata}},
        "facets":{"kip://legacy/nexus@1.1.0/LegacyRecord":{"record":{"_id":9,"type":"Commitment",
        "attributes":{"status":"pending","history":"old text ".repeat(3000)},"metadata":metadata}}}});
    let mut compact = original.clone();
    assert!(super::compact::memory(&mut compact, true));
    assert_eq!(compact["attributes"]["approval_required"], true);
    assert_eq!(
        compact["attributes"]["summary"],
        "Never publish without approval"
    );
    assert_eq!(
        compact["recall_detail"]["legacy_source"]["metadata"],
        metadata
    );
    assert_eq!(compact["recall_detail"]["element"], "C-10");
    assert!(
        compact["facets"]
            .get("kip://legacy/nexus@1.1.0/LegacyRecord")
            .is_none()
    );
    assert!(compact.to_string().len() < original.to_string().len() / 10);
}

#[tokio::test]
async fn different_questions_discover_different_records_before_model_selection() {
    let (_, space, requests) = setup("question_grounding", Behavior::Select).await;
    let mut ids = Vec::new();
    for term in ["Orionlaunch", "Oceanplaybook"] {
        ids.push(
            seed(
                &space,
                r#"CREATE CONCEPT ?c {TYPE "Event" NAME :term SET ATTRIBUTES {summary: :term}}"#,
                kip::param("term", term),
            )
            .await["handles"]["c"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    for (index, term) in ["Orionlaunch", "Oceanplaybook"].into_iter().enumerate() {
        let output = space
            .query(
                SELF_USER_ID,
                StringOr::Value(RecallInput {
                    query: term.into(),
                    budget: Some(limits(4096, 131_072)),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert!(output.failed_reason.is_none(), "{output:?}");
        let packet = check_output(&output, 4096).unwrap();
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.channel == Channel::Kip && item.content["id"] == ids[index])
        );
        assert!(
            !packet
                .items
                .iter()
                .any(|item| item.channel == Channel::Kip && item.content["id"] == ids[1 - index])
        );
    }
    assert_eq!(requests.lock().len(), 2); // no model-controlled lookup was needed
    space.close().await.unwrap();
}

#[tokio::test]
async fn a_tight_planning_budget_evicts_only_optional_items_until_the_request_fits() {
    let (_app, space, requests) = setup("p5_partial_eviction", Behavior::Select).await;
    let budget = RecallBudget::default();
    // The same optional history item before each pass, so the two planner
    // requests differ only by what the tighter budget evicts.
    let history = || {
        let mut history = space.recall.history.write();
        history.clear();
        history.push_back(Document::from_text("old", "An earlier conversation."));
    };
    history();
    space
        .query(SELF_USER_ID, input(Some(budget.clone())))
        .await
        .unwrap();
    let full = requests.lock()[0].clone();
    let full_tokens = budget::count(&normalized_request(&full).unwrap()).unwrap();
    let items_of = |request: &CompletionRequest| {
        let snapshot: Json = serde_json::from_str(&request.prompt).unwrap();
        let items: Vec<MemoryItem> =
            serde_json::from_value(snapshot["memory_items"].clone()).unwrap();
        (items, snapshot["coverage"]["omitted"].clone())
    };
    let (all, _) = items_of(&full);
    let pinned =
        |item: &MemoryItem| matches!(item.priority, Priority::Required | Priority::Warning);
    assert!(all.iter().any(|item| !pinned(item)));

    history();
    let output = space
        .query(
            SELF_USER_ID,
            input(Some(limits(budget.max_tokens, full_tokens as u32 - 1))),
        )
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{}", output.content);
    let tight = requests.lock()[1].clone();
    assert!(budget::count(&normalized_request(&tight).unwrap()).unwrap() < full_tokens);
    let (kept, omitted) = items_of(&tight);
    assert!(kept.len() < all.len());
    for item in all.iter().filter(|item| pinned(item)) {
        assert!(
            kept.contains(item),
            "a required item was evicted: {}",
            item.id
        );
    }
    assert!(!omitted.as_array().unwrap().is_empty());
    space.close().await.unwrap();
}

#[tokio::test]
async fn small_output_and_context_budgets_return_useful_partial_records() {
    let (_, space, requests) = setup("small_partial_recall", Behavior::Select).await;
    for _ in 0..6 {
        seed(&space, r#"CREATE CONCEPT ?c {TYPE "Commitment" SET ATTRIBUTES {summary: :summary,status:"fulfilled"}}"#,
            kip::param("summary","Historical finished work ".repeat(1000))).await;
    }
    let pending=seed(&space,r#"CREATE CONCEPT ?c {TYPE "Commitment" SET ATTRIBUTES {summary:"Never publish without approval",status:"pending"}}"#,
        Default::default()).await["handles"]["c"].clone();
    let event=seed(&space,r#"CREATE CONCEPT ?c {TYPE "Event" NAME "Orionlaunch" SET ATTRIBUTES {summary:"Orionlaunch release reached its milestone"}}"#,
        Default::default()).await["handles"]["c"].clone();
    for context_tokens in [131_072, 1] {
        let output = space
            .query(
                SELF_USER_ID,
                StringOr::Value(RecallInput {
                    query: "Orionlaunch".into(),
                    budget: Some(limits(2048, context_tokens)),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert!(output.failed_reason.is_none(), "{output:?}");
        let packet = check_output(&output, 2048).unwrap();
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.priority == Priority::Required && item.content["id"] == pending)
        );
        assert!(packet.items.iter().any(|item| item.content["id"] == event));
        assert!(
            !packet
                .items
                .iter()
                .any(|item| item.content["attributes"]["status"] == "fulfilled")
        );
        if context_tokens == 1 {
            assert!(
                packet
                    .items
                    .iter()
                    .any(|item| item.priority == Priority::Warning
                        && item.content["reason"] == "recall_context_budget_exhausted")
            );
        }
    }
    assert_eq!(requests.lock().len(), 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn fulfilled_commitments_remain_searchable_as_optional_history() {
    let (_, space, _) = setup("fulfilled_history", Behavior::Select).await;
    let id=seed(&space,r#"CREATE CONCEPT ?c {TYPE "Commitment" NAME "Mercuryreport" SET ATTRIBUTES {summary:"Mercuryreport delivered",status:"fulfilled"}}"#,
        Default::default()).await["handles"]["c"].clone();
    let output = space
        .query(
            SELF_USER_ID,
            StringOr::Value(RecallInput {
                query: "Mercuryreport".into(),
                budget: Some(limits(2048, 131_072)),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    assert!(output.failed_reason.is_none(), "{output:?}");
    let packet = check_output(&output, 2048).unwrap();
    assert!(
        packet
            .items
            .iter()
            .any(|item| item.content["id"] == id && item.priority == Priority::Relevant)
    );
    space.close().await.unwrap();
}

#[test]
fn packet_ties_preserve_search_order_beyond_nine_candidates() {
    let mut material = Material::default();
    let ids: Vec<_> = (0..14)
        .map(|rank| {
            material
                .add(Channel::Kip, Priority::Relevant, json!({"rank":rank}))
                .unwrap()
                .unwrap()
        })
        .collect();
    let full = limits(8192, 131_072);
    let first = budget::pack(&full, &material.items, &ids[..1], material.coverage()).unwrap();
    let restricted = limits(first.tokens as u32, 131_072);
    let packet = budget::pack(&restricted, &material.items, &ids, material.coverage()).unwrap();
    let packet: MemoryPacket = serde_json::from_str(&packet.content).unwrap();
    assert_eq!(packet.items.len(), 1);
    assert_eq!(packet.items[0].id, ids[0]);
}

#[tokio::test]
async fn discovered_concepts_deliver_their_claims_and_final_beliefs_without_model_planning() {
    let (_, space, requests) = setup("discovered_claims", Behavior::Select).await;
    for definition in [
        r#"DEFINE CONCEPT TYPE "Project" {description: "A project or named undertaking."}"#,
        r#"DEFINE PREDICATE "release_time" {description: "When the subject project is released.", subject: {concept_types: ["Project"]}, functional: true}"#,
        r#"DEFINE PREDICATE "owned_by" {description: "The subject project is owned by the object person.", subject: {concept_types: ["Project"]}, object: {concept_types: ["Person"]}}"#,
    ] {
        seed(&space, definition, Default::default()).await;
    }
    let handle = |result: Json, name: &str| json!({"id": result["handles"][name]});
    let user = handle(
        seed(&space, r#"UPSERT CONCEPT ?u { MATCH {type: "Person", key: "synthetic-user"} SET FIELDS {name: "Synthetic User"} }"#, Default::default()).await,
        "u",
    );
    let birch = handle(
        seed(&space, r#"UPSERT CONCEPT ?p { MATCH {type: "Project", key: "project-birch"} SET FIELDS {name: "Project Birch"} }"#, Default::default()).await,
        "p",
    );
    // The Brain's own queued review of a draft symbol repeats the question's words.
    seed(&space, r#"CREATE CONCEPT ?t {TYPE "SleepTask" NAME "Review draft PredicateType release_time" SET ATTRIBUTES {task_class: "review_schema", summary: "Review the draft PredicateType release_time: when project Birch releases", status: "pending"}}"#, Default::default()).await;
    let mut parameters = serde_json::Map::new();
    parameters.insert("u".into(), user.clone());
    parameters.insert("b".into(), birch.clone());
    for command in [
        r#"ASSERT (:b, "release_time", "Friday") {by: :u, mode: "stated", at: "2026-01-01T00:00:00.000Z"}"#,
        r#"ASSERT (:b, "release_time", "Tuesday") {by: :u, mode: "stated", at: "2026-01-02T00:00:00.000Z"}"#,
        r#"ASSERT (:b, "owned_by", :u) {by: :u, mode: "stated", at: "2026-01-03T00:00:00.000Z"}"#,
    ] {
        seed(&space, command, parameters.clone()).await;
    }
    // The Bot's product search allows one 32K planning budget. An explicitly
    // smaller budget must still return the same grounded claims without a call.
    for context_tokens in [131_072, 32_768, 1] {
        let output = space
            .query(
                SELF_USER_ID,
                StringOr::Value(RecallInput {
                    query: "On what day does project Birch currently release?".into(),
                    budget: Some(limits(4096, context_tokens)),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        assert!(output.failed_reason.is_none(), "{output:?}");
        let packet = check_output(&output, 4096).unwrap();
        let claims: Vec<&Json> = packet
            .items
            .iter()
            .filter(|item| item.channel == Channel::Kip && item.content["kind"] == "claim")
            .map(|item| &item.content)
            .collect();
        let claim = |object: Json| {
            claims
                .iter()
                .find(|claim| claim["object"] == object)
                .unwrap_or_else(|| panic!("no claim with object {object}: {claims:?}"))
        };
        let tuesday = claim(json!("Tuesday"));
        assert_eq!(tuesday["belief"], "accepted");
        assert_eq!(
            tuesday["subject"],
            json!({"id": birch["id"], "name": "Project Birch"})
        );
        assert!(
            tuesday["predicate"]
                .as_str()
                .unwrap()
                .ends_with("/release_time")
        );
        assert_eq!(tuesday["stance"], "support");
        assert_eq!(tuesday["mode"], "stated");
        assert_eq!(tuesday["asserted_at"], "2026-01-02T00:00:00.000Z");
        assert_eq!(
            tuesday["asserted_by"],
            json!({"id": user["id"], "name": "Synthetic User"})
        );
        assert!(tuesday.get("excluded").is_none());
        // The value it succeeded is delivered with the engine's own status and reason.
        let friday = claim(json!("Friday"));
        assert_eq!(friday["belief"], "insufficient");
        assert_eq!(friday["excluded"], json!(["outside_valid_time"]));
        // A Concept object carries its name; each Assertion is delivered once.
        let owner = claim(json!({"id": user["id"], "name": "Synthetic User"}));
        assert_eq!(owner["belief"], "accepted");
        assert_eq!(claims.len(), 3, "{claims:?}");
        assert!(
            packet
                .items
                .iter()
                .any(|item| item.content["id"] == birch["id"])
        );
        assert!(!packet.items.iter().any(|item| {
            item.content["schema_ref"]
                .as_str()
                .is_some_and(|s| s.ends_with("/SleepTask"))
        }));
    }
    {
        let seen = requests.lock();
        assert_eq!(seen.len(), 2);
        let selection = &seen[1];
        assert_eq!(selection.tools.len(), 1);
        assert_eq!(selection.tools[0].name, SELECT);
        let counted = budget::count(&normalized_request(selection).unwrap()).unwrap();
        assert!(counted <= 32_768, "{counted}-token selection request");
        for reference in [
            anda_kip::KIP_SYNTAX,
            anda_kip::COGNITIVE_MEMORY_PROFILE,
            anda_kip::KIP_RECALL_CARD,
            anda_kip::KIP_VOCABULARY_CARD,
        ] {
            assert_eq!(selection.instructions.matches(reference).count(), 1);
        }
    }
    space.close().await.unwrap();
}

#[tokio::test]
async fn host_discovery_follows_one_supported_relation_and_resolves_the_counterparty_by_key() {
    let (_, space, requests) = setup("bounded_relation_path", Behavior::Select).await;
    for definition in [
        r#"DEFINE CONCEPT TYPE "Project" {description: "A named project."}"#,
        r#"DEFINE CONCEPT TYPE "Team" {description: "A named team."}"#,
        r#"DEFINE PREDICATE "owned_by" {description: "The team owning a project.", subject: {concept_types: ["Project"]}, object: {concept_types: ["Team"]}}"#,
        r#"DEFINE PREDICATE "leads" {description: "A person leads a team.", subject: {concept_types: ["Person"]}, object: {concept_types: ["Team"]}}"#,
    ] {
        seed(&space, definition, Default::default()).await;
    }
    let graph = seed(&space, r#"MUTATE {
        CREATE CONCEPT ?project {TYPE "Project" NAME "Larch"}
        CREATE CONCEPT ?team {TYPE "Team" NAME "Payments"}
        CREATE CONCEPT ?other {TYPE "Team" NAME "Shipping"}
        UPSERT CONCEPT ?lead {MATCH {type: "Person", key: "owner-key"} SET FIELDS {name: "Morgan Reyes"}}
        CREATE CONCEPT ?outsider {TYPE "Person" NAME "Other Morgan"}
        ASSERT ?first (?project, "owned_by", ?team) {by: ?lead, mode: "stated"}
        ASSERT ?second (?lead, "leads", ?team) {by: ?lead, mode: "stated"}
        ASSERT ?negative (?project, "owned_by", ?other) {by: ?lead, mode: "stated", stance: "reject"}
        ASSERT ?unrelated (?outsider, "leads", ?other) {by: ?outsider, mode: "stated"}
    }"#, Default::default()).await;
    let handles = &graph["handles"];
    // The initial search can only find the named project. Its team's leader needs a second edge.
    let hits = space
        .execute_kip_readonly(kip::request_with(
            "SEARCH CONCEPT :query LIMIT 16",
            kip::param("query", "Larch"),
        ))
        .await
        .unwrap();
    let hits = kip::ok_result(&hits).unwrap()["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["element"]["id"], handles["project"]);
    for (query, counterparty, context_tokens) in [
        ("Larch", None, 1),
        ("Larch", None, 32_768),
        ("我带的是哪个团队？", Some("owner-key"), 1),
    ] {
        let result = space
            .query_structured(
                SELF_USER_ID,
                StringOr::Value(RecallInput {
                    query: query.into(),
                    context: counterparty.map(|key| crate::types::InputContext {
                        counterparty: Some(key.into()),
                        source: Some("memory-product-search".into()),
                        ..Default::default()
                    }),
                    budget: Some(limits(4096, context_tokens)),
                }),
            )
            .await
            .unwrap();
        assert!(result.failed_reason.is_none());
        let packet: MemoryPacket = serde_json::from_str(&result.answer).unwrap();
        assert_eq!(
            result.memory_budget.unwrap().tokens,
            budget::count(&result.answer).unwrap()
        );
        let claims: Vec<_> = packet
            .items
            .iter()
            .filter(|i| i.content["kind"] == "claim")
            .map(|i| &i.content)
            .collect();
        for key in ["first", "second"] {
            assert_eq!(
                claims
                    .iter()
                    .filter(|c| c["assertion"] == handles[key] && c["belief"] == "accepted")
                    .count(),
                1,
                "{claims:?}"
            );
        }
        assert!(
            !claims
                .iter()
                .any(|c| c["assertion"] == handles["unrelated"]),
            "{claims:?}"
        );
        assert!(!packet.action_ready && !packet.semantic_complete);
    }
    assert_eq!(requests.lock().len(), 1);
    space.close().await.unwrap();
}

#[tokio::test]
async fn selection_only_rejects_an_unadvertised_read() {
    let (_, space, seen) = setup(
        "selector_fences_reads",
        Behavior::ReadThenSelect(vec![tool("execute_kip_readonly", "DESCRIBE PRIMER")]),
    )
    .await;
    let query = || {
        StringOr::Value(RecallInput {
            query: "Selectorcandidate".into(),
            budget: Some(limits(4096, 32_768)),
            ..Default::default()
        })
    };
    let empty = space.query(SELF_USER_ID, query()).await.unwrap();
    assert!(empty.failed_reason.is_none());
    assert!(
        seen.lock().is_empty(),
        "no native candidate needs selection"
    );
    seed(&space, r#"CREATE CONCEPT ?c { TYPE "Event" NAME "Selectorcandidate" SET ATTRIBUTES {summary: "Memory to rank"} }"#, Default::default()).await;
    let output = space.query(SELF_USER_ID, query()).await.unwrap();
    assert_eq!(
        output.failed_reason.as_deref(),
        Some("recall_planner_incomplete")
    );
    assert!(output.tools_usage.is_empty());
    assert_eq!(seen.lock().len(), 1);
    space.close().await.unwrap();
}
