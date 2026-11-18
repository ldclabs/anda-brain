//! Memory Interface mechanism tests: a scripted model drives the real
//! Formation and Recall passes; everything else is the production path.
//! These establish mechanism, not model behavior (MIB measures that).
use super::*;
use crate::{
    agents::SELF_USER_ID,
    testkit::{app_state_core, create_loaded_space},
};
use anda_core::{AgentOutput, BoxPinFut, CompletionRequest, Message, ToolCall};
use anda_engine::model::CompletionFeaturesDyn;
use anda_kip::memory::binding::{Briefing, ChannelState, Phase};
use object_store::memory::InMemory;
use std::collections::VecDeque;

#[test]
fn wire_bundle_names_survive_admission_in_json_and_cbor() {
    let mut raw = json!({"kip_memory":"2.0","request_id":"request-1","operation":"recall",
        "input":{"mode":"attention"},"requires":["memory_basic","future_bundle"]});
    let cbor = cbor2::to_vec(&raw).unwrap();
    for incoming in [
        serde_json::from_value::<IncomingRequest>(raw.clone()).unwrap(),
        cbor2::from_slice::<IncomingRequest>(&cbor).unwrap(),
    ] {
        let failed = incoming.resolve().unwrap_err();
        assert_eq!(failed.request_id.as_deref(), Some("request-1"));
        assert_eq!(failed.error.unwrap().code, "UnsupportedCapability");
    }
    raw["requires"] = json!(["memory_experience"]);
    let request = serde_json::from_value::<IncomingRequest>(raw.clone())
        .unwrap()
        .resolve()
        .unwrap();
    assert_eq!(request.requires, vec![Bundle::MemoryExperience]);
    raw["typo"] = json!(true);
    assert!(serde_json::from_value::<IncomingRequest>(raw).is_err());
}

#[derive(Debug)]
enum Step {
    Kip(Json),
    Read(Json),
    Final(String),
    Hold(Arc<tokio::sync::Notify>),
    Barrier {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    },
}

/// Replays tool calls and answers in order, across every agent.
#[derive(Debug, Clone, Default)]
struct Script(Arc<std::sync::Mutex<VecDeque<Step>>>);

impl Script {
    fn push(&self, step: Step) {
        self.0.lock().unwrap().push_back(step);
    }
    fn write(&self, command: &str) {
        self.push(Step::Kip(json!({ "command": command })));
    }
    fn read(&self, command: &str) {
        self.push(Step::Read(json!({ "command": command })));
    }
    fn done(&self) {
        self.push(Step::Final("done".into()));
    }
}

impl CompletionFeaturesDyn for Script {
    fn model_name(&self) -> String {
        "memory-interface-script".into()
    }

    /// Returns each call's new history as a provider does: the prompt, the
    /// tool outputs it was given, and its own turn.
    fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
        let script = self.clone();
        Box::pin(async move {
            let step = loop {
                let step = script.0.lock().unwrap().pop_front();
                match step {
                    Some(Step::Hold(gate)) => gate.notified().await,
                    Some(Step::Barrier { entered, release }) => {
                        entered.notify_one();
                        release.notified().await;
                    }
                    other => break other,
                }
            };
            let mut chat_history = Vec::new();
            if !req.prompt.is_empty() {
                chat_history.push(Message {
                    role: "user".into(),
                    content: vec![req.prompt.clone().into()],
                    ..Default::default()
                });
            }
            if !req.content.is_empty() {
                chat_history.push(Message {
                    role: req.role.clone().unwrap_or_else(|| "tool".into()),
                    content: req.content.clone(),
                    ..Default::default()
                });
            }
            let (name, args) = match step {
                Some(Step::Kip(args)) => ("execute_kip", args),
                Some(Step::Read(args)) => ("execute_kip_readonly", args),
                Some(Step::Final(text)) => {
                    chat_history.push(Message {
                        role: "assistant".into(),
                        content: vec![text.clone().into()],
                        ..Default::default()
                    });
                    return Ok(AgentOutput {
                        content: text,
                        chat_history,
                        ..Default::default()
                    });
                }
                _ => {
                    chat_history.push(Message {
                        role: "assistant".into(),
                        content: vec!["done".to_string().into()],
                        ..Default::default()
                    });
                    return Ok(AgentOutput {
                        content: "done".into(),
                        chat_history,
                        ..Default::default()
                    });
                }
            };
            let call = ToolCall {
                name: name.into(),
                args,
                result: None,
                call_id: Some(format!("call-{}", rand::random::<u32>())),
                remote_id: None,
            };
            chat_history.push(Message {
                role: "assistant".into(),
                content: vec![anda_core::ContentPart::ToolCall {
                    name: call.name.clone(),
                    args: call.args.clone(),
                    call_id: call.call_id.clone(),
                }],
                ..Default::default()
            });
            Ok(AgentOutput {
                tool_calls: vec![call],
                chat_history,
                ..Default::default()
            })
        })
    }
}

struct Fixture {
    app: crate::space::AppState,
    store: Arc<InMemory>,
    space: Arc<Space>,
    script: Script,
    name: String,
}

async fn fixture(name: &str) -> Fixture {
    let script = Script::default();
    let store = Arc::new(InMemory::new());
    let app = app_state_core(
        name,
        crate::testkit::models_with_completer(script.clone()),
        vec![],
        "test",
        0,
    )
    .fork_with_store(store.clone());
    let space = create_loaded_space(&app, name).await;
    crate::testkit::declare_types(&space, &["ColorScheme", "Editor"]).await;
    Fixture {
        app,
        store,
        space,
        script,
        name: name.into(),
    }
}

const NS: &str = "tester";

fn request(operation: &str, key: Option<&str>, scope: Option<Json>, input: Json) -> Request {
    serde_json::from_value(json!({
        "kip_memory": "2.0",
        "operation": operation,
        "idempotency_key": key,
        "scope": scope,
        "input": input,
    }))
    .map(|mut request: Request| {
        if key.is_none() {
            request.idempotency_key = None;
        }
        request
    })
    .unwrap()
}

async fn stage(space: &Arc<Space>, key: &str, text: &str, at: &str) -> String {
    space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message {
                    role: "user".into(),
                    content: vec![text.to_string().into()],
                    ..Default::default()
                }],
                observed_at: Some(at.into()),
                kind: SourceKind::Message,
                order: None,
                idempotency_key: key.into(),
            },
        )
        .await
        .unwrap()
        .source_ref
}

async fn wait_available(space: &Arc<Space>, receipt: &str) -> wire::Progress {
    for _ in 0..200 {
        let progress = space.memory_progress(NS, receipt).await.unwrap();
        if progress.phase != Phase::Recorded {
            return progress;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("receipt {receipt} stayed recorded");
}

fn error_code(response: &Response) -> String {
    response
        .error
        .as_ref()
        .map(|error| error.code.clone())
        .unwrap_or_else(|| format!("no error: {:?}", response.status))
}

fn briefing(response: &Response) -> Briefing {
    serde_json::from_value(response.result.clone().expect("a briefing")).unwrap()
}

/// The claim a scripted Formation pass writes for a preference.
fn prefers(option: &str, kind: &str, at: &str, scoped: bool) -> String {
    format!(
        r#"MUTATE {{
            UPSERT CONCEPT ?user {{ MATCH {{type: "Person", key: "user"}} SET FIELDS {{name: "User"}} }}
            UPSERT CONCEPT ?option {{ MATCH {{type: "{kind}", key: "{option}"}} SET FIELDS {{name: "{option}"}} }}
            ASSERT ?claim (?user, "prefers", ?option) {{ by: ?user, mode: "stated", evidence: :msg1, at: "{at}"{} }}
        }}"#,
        if scoped { ", context: :contexts" } else { "" }
    )
}

async fn evidence_count(space: &Arc<Space>) -> u64 {
    let response = space
        .execute_kip_readonly(crate::kip::request(
            "FIND(COUNT(?e)) WHERE { ?e EVIDENCE {} }",
        ))
        .await
        .unwrap();
    crate::kip::ok_result(&response)
        .map(|v| crate::agents::first_row(v.clone()))
        .and_then(|v| v.as_u64().or_else(|| v.as_array()?.first()?.as_u64()))
        .unwrap()
}

async fn claims(space: &Arc<Space>, option: &str) -> Vec<Json> {
    let response = space
        .execute_kip_readonly(crate::kip::request_with(
            r#"FIND(?a) WHERE { ?o {key: :key} ?p (?u, "prefers", ?o) ?a ASSERTION {proposition: ?p} }"#,
            crate::kip::param("key", option),
        ))
        .await
        .unwrap();
    crate::kip::ok_result(&response)
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_binding_advertises_only_what_it_serves() {
    let f = fixture("mi_descriptor").await;
    let descriptor = f.space.memory_descriptor();
    descriptor.validate().unwrap();
    assert_eq!(descriptor.bundles, vec![Bundle::MemoryBasic]);
    assert_eq!(descriptor.tokenizer, crate::recall_budget::TOKENIZER);

    // An unadvertised level fails before anything runs; so does an unknown
    // tokenizer and a Space other than this one.
    let mut learning = request("recall", None, None, json!({"query": "anything"}));
    learning.requires = vec![
        Bundle::MemoryBasic,
        Bundle::MemoryExperience,
        Bundle::MemoryLearning,
    ];
    let response = f.space.memory_request(NS, true, learning).await;
    assert_eq!(error_code(&response), "UnsupportedCapability");
    let mut tokenizer = request("recall", None, None, json!({"mode": "attention"}));
    tokenizer.budget = Some(wire::Budget {
        max_output_tokens: Some(1000),
        deadline_ms: Some(1000),
        tokenizer: Some("cl100k_base".into()),
    });
    let response = f.space.memory_request(NS, true, tokenizer).await;
    assert_eq!(error_code(&response), "UnsupportedCapability");
    // The identity advertised through 0.13.1 names the same encoding.
    let mut legacy = request("recall", None, None, json!({"mode": "attention"}));
    legacy.budget = Some(wire::Budget {
        max_output_tokens: Some(1000),
        deadline_ms: Some(1000),
        tokenizer: Some(crate::recall_budget::LEGACY_TOKENIZER.into()),
    });
    let response = f.space.memory_request(NS, true, legacy).await;
    assert!(response.error.is_none(), "{}", error_code(&response));
    let mut other = request("recall", None, None, json!({"mode": "attention"}));
    other.space = Some(SpaceSelector {
        id: Some("another_space".into()),
        uri: None,
    });
    let response = f.space.memory_request(NS, true, other).await;
    assert_eq!(error_code(&response), "NotFoundOrNotVisible");
    // A mutation needs a key; a recall takes none.
    let response = f
        .space
        .memory_request(
            NS,
            true,
            request("observe", None, None, json!({"source_ref": "src-x"})),
        )
        .await;
    assert_eq!(error_code(&response), "InvalidRequestEnvelope");

    // The Nexus reports the same binding a raw KIP client discovers.
    let described = f
        .space
        .execute_kip_readonly(crate::kip::request("DESCRIBE CAPABILITIES"))
        .await
        .unwrap();
    let registry = &crate::kip::ok_result(&described).unwrap()["supported"]["registry"];
    assert_eq!(
        registry["memory_interface"]["bundles"],
        json!(["memory_basic"])
    );
    assert_eq!(registry["durable_brain_runtime"], json!(false));
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn staged_sources_are_idempotent_owned_and_admitted_first() {
    let f = fixture("mi_staging").await;
    let a = stage(
        &f.space,
        "k1",
        "I prefer dark mode",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let again = stage(
        &f.space,
        "k1",
        "I prefer dark mode",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    assert_eq!(a, again);
    let conflict = f
        .space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message {
                    role: "user".into(),
                    content: vec!["something else".to_string().into()],
                    ..Default::default()
                }],
                observed_at: None,
                kind: SourceKind::Message,
                order: None,
                idempotency_key: "k1".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code, KipErrorCode::IdempotencyConflict);
    // Another caller cannot read or cite the handle.
    assert_eq!(
        f.space
            .staged_memory_source("intruder", &a)
            .await
            .unwrap_err()
            .code,
        KipErrorCode::NotFoundOrNotVisible
    );
    let foreign = f
        .space
        .memory_request(
            "intruder",
            true,
            request("observe", Some("o1"), None, json!({"source_ref": a})),
        )
        .await;
    assert_eq!(error_code(&foreign), "NotFoundOrNotVisible");
    // Unparseable observation time is refused, not replaced.
    let bad = f
        .space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message::default()],
                observed_at: Some("yesterday".into()),
                kind: SourceKind::Message,
                order: None,
                idempotency_key: "k2".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(bad.code, KipErrorCode::InvalidRequestEnvelope);
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn a_trusted_host_stages_under_its_own_identity_and_context() {
    let f = fixture("mi_host_source").await;
    f.script.write(&prefers(
        "dark",
        "ColorScheme",
        "2026-01-01T00:00:00.000Z",
        false,
    ));
    f.script.done();
    let host = HostSource {
        identity: crate::product::SourceIdentity {
            key: "bot/conversation/7".into(),
            parents: vec!["bot/session/a".into()],
        },
        context: Some(crate::types::InputContext {
            counterparty: Some("alice-counterparty".into()),
            ..Default::default()
        }),
    };
    let input = |text: &str| StageSourceInput {
        messages: vec![Message {
            role: "user".into(),
            content: vec![text.to_string().into()],
            ..Default::default()
        }],
        observed_at: Some("2026-01-01T00:00:00.000Z".into()),
        kind: SourceKind::Message,
        order: None,
        idempotency_key: "window:7:0".into(),
    };
    let staged = f
        .space
        .stage_host_memory_source(NS, input("I prefer dark mode"), host.clone())
        .await
        .unwrap();
    // The host's attachment is part of the key's meaning.
    let conflict = f
        .space
        .stage_host_memory_source(
            NS,
            input("I prefer dark mode"),
            HostSource {
                context: None,
                ..host.clone()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(
        conflict.downcast_ref::<KipError>().map(|e| e.code),
        Some(KipErrorCode::IdempotencyConflict)
    );

    let response = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("window:7:0"),
                None,
                json!({"source_ref": staged.source_ref}),
            ),
        )
        .await;
    let receipt = response.receipt.unwrap().receipt_ref;
    assert_eq!(
        wait_available(&f.space, &receipt).await.phase,
        Phase::Available
    );
    let (progress, conversation) = f.space.memory_receipt_state(NS, &receipt).await.unwrap();
    assert_eq!(progress.phase, Phase::Available);
    // Formation records the host's identity, joined by the handle's keys,
    // and receives the host's context.
    let conversation = f
        .space
        .memory
        .get_conversation(conversation.unwrap())
        .await
        .unwrap();
    let source: crate::product::SourceIdentity = serde_json::from_value(
        conversation.extra.as_ref().unwrap()[crate::product::control::SOURCE_KEY].clone(),
    )
    .unwrap();
    assert_eq!(source.key, "bot/conversation/7");
    assert_eq!(
        source.parents,
        vec![
            "bot/session/a".to_string(),
            format!("memory-source:{}", staged.source_ref),
            format!("memory-source-digest:{}", staged.source_digest),
        ]
    );
    assert!(
        serde_json::to_string(&conversation.messages)
            .unwrap()
            .contains("alice-counterparty")
    );

    // Once the host's product deletion excludes its identity, a new source
    // under it is refused before anything is stored.
    f.space
        .suppress_sources(&std::collections::BTreeSet::from([
            "bot/conversation/7".to_string()
        ]))
        .await
        .unwrap();
    let mut later = input("I now prefer light mode");
    later.idempotency_key = "window:7:1".into();
    let refused = f
        .space
        .stage_host_memory_source(NS, later, host)
        .await
        .unwrap_err();
    assert!(matches!(
        refused.downcast_ref::<crate::product::SourceAdmissionError>(),
        Some(crate::product::SourceAdmissionError::Suppressed)
    ));
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn observe_is_recorded_before_it_is_formed_and_replays_across_restart() {
    let f = fixture("mi_observe").await;
    let gate = Arc::new(tokio::sync::Notify::new());
    f.script.push(Step::Hold(gate.clone()));
    f.script.write(&prefers(
        "dark",
        "ColorScheme",
        "2026-01-01T00:00:00.000Z",
        false,
    ));
    f.script.done();
    let source = stage(
        &f.space,
        "s1",
        "I prefer dark mode",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let observe = request(
        "observe",
        Some("observe:s1"),
        None,
        json!({"source_ref": source}),
    );
    let response = f.space.memory_request(NS, false, observe.clone()).await;
    assert_eq!(response.status, Status::Pending, "{response:?}");
    let receipt = response.receipt.clone().unwrap();
    assert_eq!(response.progress.as_ref().unwrap().phase, Phase::Recorded);

    // Durable intake is not processed memory: the barrier stays pending.
    let mut recall = request(
        "recall",
        None,
        None,
        json!({"mode": "attention", "after": [receipt.receipt_ref]}),
    );
    recall.budget = Some(wire::Budget {
        max_output_tokens: Some(4000),
        deadline_ms: Some(100),
        tokenizer: None,
    });
    let pending = f.space.memory_request(NS, false, recall.clone()).await;
    assert_eq!(pending.status, Status::Pending);
    let brief = briefing(&pending);
    assert_eq!(
        brief.coverage.pending_receipts,
        vec![receipt.receipt_ref.clone()]
    );
    assert!(!brief.coverage.action_eligible);

    gate.notify_one();
    let progress = wait_available(&f.space, &receipt.receipt_ref).await;
    assert_eq!(progress.phase, Phase::Available);
    assert_eq!(progress.disposition, Some(wire::Disposition::Formed));
    assert!(progress.available_seq.unwrap() >= receipt.accepted_seq);
    let satisfied = f.space.memory_request(NS, false, recall).await;
    assert!(briefing(&satisfied).coverage.pending_receipts.is_empty());

    // The same key and meaning replays the acknowledgement; transport ids
    // and budgets are not part of the meaning. Nothing is re-extracted.
    let conversations = f.space.conversations.len();
    let mut retry = observe.clone();
    retry.request_id = Some("another-transport-id".into());
    retry.budget = Some(wire::Budget {
        max_output_tokens: Some(10),
        deadline_ms: Some(5),
        tokenizer: None,
    });
    let replay = f.space.memory_request(NS, false, retry).await;
    assert_eq!(replay.status, Status::Succeeded, "{replay:?}");
    assert_eq!(replay.receipt, Some(receipt.clone()));
    assert_eq!(f.space.conversations.len(), conversations);
    let formed: wire::FormationResult = serde_json::from_value(replay.result.unwrap()).unwrap();
    assert!(!formed.memory_refs.is_empty());

    // The same key with another meaning conflicts.
    let changed_scope = request(
        "observe",
        Some("observe:s1"),
        Some(json!({"task_ref": "task-9"})),
        json!({"source_ref": source}),
    );
    assert_eq!(
        error_code(&f.space.memory_request(NS, false, changed_scope).await),
        "IdempotencyConflict"
    );
    let other = stage(&f.space, "s2", "I use vim", "2026-01-02T00:00:00.000Z").await;
    let changed_source = request(
        "observe",
        Some("observe:s1"),
        None,
        json!({"source_ref": other}),
    );
    assert_eq!(
        error_code(&f.space.memory_request(NS, false, changed_source).await),
        "IdempotencyConflict"
    );

    // A restart keeps the key, the receipt and its progress.
    f.space.close().await.unwrap();
    let reopened = f
        .app
        .fork_with_store(f.store.clone())
        .load_space_with(&f.name, false, false)
        .await
        .unwrap();
    let replay = reopened.memory_request(NS, false, observe).await;
    assert_eq!(replay.receipt, Some(receipt.clone()));
    assert_eq!(replay.progress.unwrap().phase, Phase::Available);
    assert_eq!(
        reopened
            .memory_progress("intruder", &receipt.receipt_ref)
            .await
            .unwrap_err()
            .code,
        KipErrorCode::NotFoundOrNotVisible
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn a_scoped_observation_stays_in_its_task() {
    let f = fixture("mi_scope").await;
    // The pass first forgets the context; the gate refuses and it retries.
    f.script.write(&prefers(
        "light",
        "ColorScheme",
        "2026-02-01T00:00:00.000Z",
        false,
    ));
    f.script.write(&prefers(
        "light",
        "ColorScheme",
        "2026-02-01T00:00:00.000Z",
        true,
    ));
    f.script.done();
    let source = stage(
        &f.space,
        "t1",
        "For task A, use light mode",
        "2026-02-01T00:00:00.000Z",
    )
    .await;
    let response = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:t1"),
                Some(json!({"task_ref": "task-a"})),
                json!({"source_ref": source}),
            ),
        )
        .await;
    let receipt = response.receipt.unwrap().receipt_ref;
    let progress = wait_available(&f.space, &receipt).await;
    assert_eq!(progress.disposition, Some(wire::Disposition::Formed));
    let rows = claims(&f.space, "light").await;
    assert_eq!(rows.len(), 1, "the unscoped attempt was refused");
    let contexts = rows[0]["context_refs"].as_array().unwrap().clone();
    assert_eq!(contexts.len(), 1);

    // task B does not see it; task A does, as accepted final belief.
    let claim = rows[0]["id"].as_str().unwrap().to_string();
    let find = format!(r#"FIND(?a) WHERE {{ ?a ASSERTION {{id: "{claim}"}} }}"#);
    f.script.read(&find);
    f.script.push(Step::Final("You prefer light mode.".into()));
    let other_task = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "recall",
                None,
                Some(json!({"task_ref": "task-b"})),
                json!({"query": "Which color scheme?"}),
            ),
        )
        .await;
    let brief = briefing(&other_task);
    assert!(brief.items.iter().all(|item| !item.text.contains("light")));
    assert!(!other_task.warnings.is_empty(), "{other_task:?}");

    let evidence_before = evidence_count(&f.space).await;
    let seq_before = f.space.memory_seq().await.unwrap();
    f.script.read(&find);
    f.script.push(Step::Final("You prefer light mode.".into()));
    let task_a = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "recall",
                None,
                Some(json!({"task_ref": "task-a"})),
                json!({"query": "Which color scheme?", "context": "just a transient note"}),
            ),
        )
        .await;
    let brief = briefing(&task_a);
    let item = brief
        .items
        .iter()
        .find(|item| item.text.contains("light"))
        .expect("the task-A claim");
    assert_eq!(item.epistemic_status, wire::EpistemicStatus::Accepted);
    assert!(!item.evidence_refs.is_empty());
    // Recall wrote nothing: transient context is not memory, and reading
    // reinforces nothing.
    assert_eq!(evidence_count(&f.space).await, evidence_before);
    assert_eq!(f.space.memory_seq().await.unwrap(), seq_before);
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn revise_writes_each_history_and_repairs_a_misrecording() {
    let f = fixture("mi_revise").await;
    // An extraction the source never said.
    f.script.write(&prefers(
        "vegetarian",
        "Editor",
        "2026-01-01T00:00:00.000Z",
        false,
    ));
    f.script.done();
    let source = stage(
        &f.space,
        "r0",
        "Alice talked about dinner",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let observed = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:r0"),
                None,
                json!({"source_ref": source}),
            ),
        )
        .await;
    wait_available(&f.space, &observed.receipt.unwrap().receipt_ref).await;
    let wrong = claims(&f.space, "vegetarian").await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A world change never supersedes: the gate refuses it.
    f.script.write(&format!(
        r#"ASSERT (:x, "prefers", :y) {{ by: :x, mode: "stated", evidence: :msg1, at: "2026-09-01T00:00:00.000Z" }} SUPERSEDING "{wrong}""#
    ));
    f.script.done();
    let moved = stage(
        &f.space,
        "r1",
        "I changed my mind",
        "2026-09-01T00:00:00.000Z",
    )
    .await;
    let world = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "revise",
                Some("revise:r1"),
                None,
                json!({"source_ref": moved, "change_kind": "world_change"}),
            ),
        )
        .await;
    let progress = wait_available(&f.space, &world.receipt.unwrap().receipt_ref).await;
    assert_eq!(progress.disposition, Some(wire::Disposition::Skipped));
    let still = claims(&f.space, "vegetarian").await;
    assert_eq!(still[0]["lifecycle"]["status"], "active");

    // A misrecording without a target is preserved and reported, not guessed.
    let report = stage(
        &f.space,
        "r2",
        "You misheard me",
        "2026-09-24T00:00:00.000Z",
    )
    .await;
    let untargeted = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "revise",
                Some("revise:r2"),
                None,
                json!({"source_ref": report, "change_kind": "misrecorded"}),
            ),
        )
        .await;
    assert_eq!(untargeted.status, Status::Partial, "{untargeted:?}");
    assert_eq!(
        untargeted.progress.unwrap().disposition,
        Some(wire::Disposition::EvidenceOnly)
    );

    // With the target, the host repairs it; the pass writes nothing.
    f.script.done();
    let repair = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "revise",
                Some("revise:r3"),
                None,
                json!({"source_ref": report, "change_kind": "misrecorded", "target_ref": wrong}),
            ),
        )
        .await;
    let progress = wait_available(&f.space, &repair.receipt.unwrap().receipt_ref).await;
    assert_eq!(progress.phase, Phase::Available, "{progress:?}");
    let rows = claims(&f.space, "vegetarian").await;
    assert_eq!(
        rows[0]["_system"]["recording_validity"]["status"],
        "invalidated"
    );
    // No actor withdrawal was forged: the lifecycle is untouched.
    assert_eq!(rows[0]["lifecycle"]["status"], "active");
    f.script.read(&format!(
        "FIND(?a) WHERE {{ ?a ASSERTION {{id: \"{wrong}\"}} }} LIMIT 1"
    ));
    f.script.push(Step::Final("You are vegetarian.".into()));
    let recalled = f
        .space
        .memory_request(
            NS,
            false,
            request("recall", None, None, json!({"query":"What diet?"})),
        )
        .await;
    assert_ne!(briefing(&recalled).summary, "You are vegetarian.");
    assert!(!recalled.warnings.is_empty());
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn feedback_keeps_its_origin_and_grades_nothing() {
    let f = fixture("mi_feedback").await;
    let source = f
        .space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message {
                    role: "assistant".into(),
                    content: vec!["I completed the deployment successfully".to_string().into()],
                    ..Default::default()
                }],
                observed_at: Some("2026-09-01T00:00:00.000Z".into()),
                kind: SourceKind::Message,
                order: None,
                idempotency_key: "f1".into(),
            },
        )
        .await
        .unwrap()
        .source_ref;
    let missing = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "feedback",
                Some("feedback:bad"),
                None,
                json!({"source_ref": source, "decision_ref": "X-999"}),
            ),
        )
        .await;
    assert_eq!(error_code(&missing), "NotFoundOrNotVisible");
    let response = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "feedback",
                Some("feedback:f1"),
                None,
                json!({"source_ref": source}),
            ),
        )
        .await;
    assert_eq!(response.status, Status::Succeeded, "{response:?}");
    assert_eq!(
        response.progress.unwrap().disposition,
        Some(wire::Disposition::EvidenceOnly)
    );
    let result: wire::FormationResult = serde_json::from_value(response.result.unwrap()).unwrap();
    let evidence = f
        .space
        .execute_kip_readonly(crate::kip::request_with(
            "FIND(?e) WHERE { ?e EVIDENCE {id: :id} }",
            crate::kip::param("id", result.memory_refs[0].as_str()),
        ))
        .await
        .unwrap();
    let row = &crate::kip::ok_result(&evidence).unwrap()[0];
    assert_eq!(row["evidence_class"], "agent_statement");
    let outcomes = f
        .space
        .execute_kip_readonly(crate::kip::request(
            r#"FIND(?e) WHERE { ?e EVIDENCE {evidence_class: "outcome"} } LIMIT 1"#,
        ))
        .await
        .unwrap();
    assert_eq!(crate::kip::ok_result(&outcomes).unwrap(), &json!([]));
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn forget_completes_only_after_every_surface_and_blocks_re_ingestion() {
    let f = fixture("mi_forget").await;
    f.script.write(&prefers(
        "dark",
        "ColorScheme",
        "2026-01-01T00:00:00.000Z",
        false,
    ));
    f.script.done();
    let source = stage(
        &f.space,
        "g1",
        "I prefer dark mode",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let observed = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:g1"),
                None,
                json!({"source_ref": source}),
            ),
        )
        .await;
    let receipt = observed.receipt.unwrap().receipt_ref;
    wait_available(&f.space, &receipt).await;
    let claim = claims(&f.space, "dark").await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let forget = request(
        "forget",
        Some("forget:g1"),
        None,
        json!({"target_ref": claim, "mode": "semantic"}),
    );
    // A delegated token does not decide a semantic forget.
    assert_eq!(
        error_code(&f.space.memory_request(NS, false, forget.clone()).await),
        "NotAuthorized"
    );
    let response = f.space.memory_request(NS, true, forget.clone()).await;
    assert_eq!(response.status, Status::Succeeded, "{response:?}");
    let progress = response.progress.unwrap();
    assert_eq!(progress.disposition, Some(wire::Disposition::Erased));
    let result: wire::ForgetResult = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result.status, wire::ForgetStatus::Completed);
    let plan = f.space.memory_plan(NS, &result.plan_ref).await.unwrap();
    assert_eq!(plan["plan"]["status"], "completed");
    assert!(
        !plan["plan"]["source_event_refs"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(claims(&f.space, "dark").await.is_empty());
    // The staged bytes and the Formation transcript are gone.
    let staged = f.space.staged_memory_source(NS, &source).await.unwrap();
    assert!(staged.erased && staged.messages.is_empty());
    // The source cannot come back: re-staging the same bytes is refused.
    let restage = f
        .space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message {
                    role: "user".into(),
                    content: vec!["I prefer dark mode".to_string().into()],
                    ..Default::default()
                }],
                observed_at: Some("2026-01-01T00:00:00.000Z".into()),
                kind: SourceKind::Message,
                order: None,
                idempotency_key: "g1-again".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(restage.code, KipErrorCode::NotFoundOrNotVisible);
    // A replay reports the same completed erasure; the old receipt keeps its
    // processing horizon but recreates nothing.
    let replay = f.space.memory_request(NS, true, forget).await;
    assert_eq!(replay.status, Status::Succeeded);
    assert!(claims(&f.space, "dark").await.is_empty());
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn recall_surfaces_scoped_constraints_within_budget_and_expands_its_basis() {
    let f = fixture("mi_recall").await;
    f.script.write(
        r#"MUTATE {
            CREATE CONCEPT ?rule {
                TYPE "Insight" NAME "No Friday deploys"
                SET ATTRIBUTES {summary: "Never deploy on Fridays", insight_class: "constraint"}
                SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts}
            }
            CREATE EVIDENCE ?e { CLIENT KEY "fixture-evidence" SET FIELDS {evidence_class: "user_statement", payload: "never deploy on Fridays", observed_at: "2026-09-20T00:00:00.000Z"} }
        }"#,
    );
    f.script.done();
    let source = stage(
        &f.space,
        "c1",
        "Never deploy on Fridays",
        "2026-09-20T00:00:00.000Z",
    )
    .await;
    let scope = json!({"task_ref": "release"});
    let observed = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:c1"),
                Some(scope.clone()),
                json!({"source_ref": source}),
            ),
        )
        .await;
    let receipt = observed.receipt.unwrap().receipt_ref;
    assert_eq!(
        wait_available(&f.space, &receipt).await.disposition,
        Some(wire::Disposition::Formed)
    );

    f.script
        .push(Step::Final("Plan the deployment for 2026-09-25.".into()));
    let mut action = request(
        "recall",
        None,
        Some(scope.clone()),
        json!({"query": "Help me schedule the deployment on 2026-09-25", "mode": "action", "after": [receipt]}),
    );
    action.budget = Some(wire::Budget {
        max_output_tokens: Some(4000),
        deadline_ms: Some(10_000),
        tokenizer: None,
    });
    let response = f.space.memory_request(NS, false, action.clone()).await;
    let brief = briefing(&response);
    let rule = brief
        .items
        .iter()
        .find(|item| item.role == wire::ItemRole::Constraint)
        .expect("the unasked constraint surfaces");
    assert!(rule.text.contains("Fridays"));
    assert_eq!(brief.coverage.channels.constraints, ChannelState::Complete);
    // What recall returned reaches the next Maintenance cycle as an exposure
    // batch — once; the cycle after reads only newer entries.
    let assessment = f.space.maintenance_assessment(None).await;
    assert!(
        assessment.exposures.iter().any(|tally| tally.retrieved > 0),
        "{:?}",
        assessment.exposures
    );
    let again = f.space.maintenance_assessment(None).await;
    assert!(again.exposures.is_empty());

    // Other tasks do not inherit it.
    f.script.push(Step::Final("ok".into()));
    let mut other = action.clone();
    other.scope = Some(serde_json::from_value(json!({"task_ref": "unrelated"})).unwrap());
    other.input = json!({"query": "Help me schedule the deployment", "mode": "action"});
    let brief_other = briefing(&f.space.memory_request(NS, false, other).await);
    assert!(
        brief_other
            .items
            .iter()
            .all(|item| item.role != wire::ItemRole::Constraint)
    );

    // An impossibly small budget is an explicit limit error, not omission.
    f.script.push(Step::Final("x".into()));
    let mut tiny = action.clone();
    tiny.budget = Some(wire::Budget {
        max_output_tokens: Some(20),
        deadline_ms: Some(10_000),
        tokenizer: None,
    });
    assert_eq!(
        error_code(&f.space.memory_request(NS, false, tiny).await),
        "ResultLimitExceeded"
    );

    // The basis expands to the retained version and coverage.
    let expand = request(
        "recall",
        None,
        None,
        json!({"target_ref": brief.basis_ref, "detail": "evidence"}),
    );
    let expanded = briefing(&f.space.memory_request(NS, false, expand.clone()).await);
    let details = expanded.details.expect("details");
    assert!(details.coverage["plans"]["constraints"]["method"] == "exact");
    assert!(!details.elements.is_empty());
    assert!(!expanded.coverage.action_eligible);
    // Another caller cannot expand it.
    assert_eq!(
        error_code(&f.space.memory_request("intruder", false, expand).await),
        "NotFoundOrNotVisible"
    );

    // Never observed is insufficient, not no.
    f.script
        .push(Step::Final("I have no memory of that.".into()));
    let unknown = briefing(
        &f.space
            .memory_request(
                NS,
                false,
                request(
                    "recall",
                    None,
                    None,
                    json!({"query": "Is Alice vegetarian?"}),
                ),
            )
            .await,
    );
    assert!(
        unknown
            .uncertainties
            .iter()
            .any(|u| u.contains("insufficient"))
    );
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn a_failed_predecessor_blocks_its_successor() {
    let f = fixture("mi_order").await;
    // A holds the queue; B and C wait behind it, C naming B as predecessor.
    let gate = Arc::new(tokio::sync::Notify::new());
    f.script.push(Step::Hold(gate.clone()));
    f.script.done();
    let first = stage(&f.space, "o1", "first", "2026-01-01T00:00:00.000Z").await;
    let a = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:o1"),
                None,
                json!({"source_ref": first}),
            ),
        )
        .await;
    let a_ref = a.receipt.unwrap().receipt_ref;
    let second = stage(&f.space, "o2", "second", "2026-01-02T00:00:00.000Z").await;
    let b = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:o2"),
                None,
                json!({"source_ref": second}),
            ),
        )
        .await;
    let b_ref = b.receipt.unwrap().receipt_ref;
    let third = f
        .space
        .stage_memory_source(
            NS,
            StageSourceInput {
                messages: vec![Message {
                    role: "user".into(),
                    content: vec!["third".to_string().into()],
                    ..Default::default()
                }],
                observed_at: Some("2026-01-03T00:00:00.000Z".into()),
                kind: SourceKind::Message,
                order: Some(SourceOrder {
                    stream_ref: "chat".into(),
                    event_ref: "m3".into(),
                    ordinal: 3,
                    predecessor_receipts: vec![b_ref.clone()],
                }),
                idempotency_key: "o3".into(),
            },
        )
        .await
        .unwrap()
        .source_ref;
    let c = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("observe:o3"),
                None,
                json!({"source_ref": third}),
            ),
        )
        .await;
    let c_ref = c.receipt.unwrap().receipt_ref;
    // B's source is excluded before B runs, so B fails terminally.
    f.space
        .suppress_sources(&[format!("memory-source:{second}")].into())
        .await
        .unwrap();
    gate.notify_one();
    assert_eq!(
        wait_available(&f.space, &a_ref).await.phase,
        Phase::Available
    );
    let b_progress = wait_available(&f.space, &b_ref).await;
    assert_eq!(b_progress.phase, Phase::Failed, "{b_progress:?}");
    let progress = wait_available(&f.space, &c_ref).await;
    assert_eq!(progress.phase, Phase::Failed, "{progress:?}");
    assert!(progress.reason.unwrap().starts_with("predecessor_failed"));
    assert_eq!(progress.error.unwrap().code, "PreconditionFailed");
    f.space.close().await.unwrap();
    let _ = SELF_USER_ID;
}

#[tokio::test]
async fn staging_without_observed_at_replays_after_restart() {
    let f = fixture("mi_staging_time").await;
    let input = StageSourceInput {
        messages: vec![Message {
            role: "user".into(),
            content: vec!["same source".to_string().into()],
            ..Default::default()
        }],
        observed_at: None,
        kind: SourceKind::Message,
        order: None,
        idempotency_key: "implicit-time".into(),
    };
    let first = f
        .space
        .stage_memory_source(NS, input.clone())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_eq!(
        first,
        f.space
            .stage_memory_source(NS, input.clone())
            .await
            .unwrap()
    );
    f.space.close().await.unwrap();
    let reopened = f
        .app
        .fork_with_store(f.store.clone())
        .load_space_with(&f.name, false, false)
        .await
        .unwrap();
    assert_eq!(
        first,
        reopened.stage_memory_source(NS, input).await.unwrap()
    );
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn scoped_feedback_and_raw_evidence_do_not_escape_the_task() {
    let f = fixture("mi_feedback_scope").await;
    let source = stage(
        &f.space,
        "feedback",
        "task A private feedback",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let response = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "feedback",
                Some("f"),
                Some(json!({"task_ref":"task-a"})),
                json!({"source_ref": source}),
            ),
        )
        .await;
    let evidence = response.result.unwrap()["memory_refs"][0]
        .as_str()
        .unwrap()
        .to_string();
    f.script.read(&format!(
        "FIND(?e) WHERE {{ ?e EVIDENCE {{id: \"{evidence}\"}} }} LIMIT 1"
    ));
    f.script.push(Step::Final("task A private feedback".into()));
    let other = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "recall",
                None,
                Some(json!({"task_ref":"task-b"})),
                json!({"query":"What feedback?"}),
            ),
        )
        .await;
    let brief = briefing(&other);
    assert!(brief.items.is_empty(), "{brief:?}");
    assert!(!brief.summary.contains("task A private feedback"));
    assert!(!other.warnings.is_empty());
    let original = anda_kip::execute_request(f.space.memory.nexus().as_ref(), &crate::kip::request(
        r#"CREATE EVIDENCE ?e { SET FIELDS {evidence_class: "tool_result", payload: "tool feedback", observed_at: "2026-01-01T00:00:00.000Z"} }"#
    )).await;
    let original_id = crate::kip::ok_result(&original).unwrap()["handles"]["e"]
        .as_str()
        .unwrap();
    let copied = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "feedback",
                Some("tool-feedback"),
                Some(json!({"task_ref":"task-a"})),
                json!({"source_ref":original_id}),
            ),
        )
        .await;
    let copied_id = copied.result.unwrap()["memory_refs"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let Element::Evidence(copied) = f
        .space
        .memory
        .nexus()
        .store
        .get_element(copied_id)
        .await
        .unwrap()
    else {
        panic!("expected Evidence");
    };
    assert_eq!(copied.evidence_class, "tool_result");
    f.space.close().await.unwrap();
}

/// A scoped constraint, as a scripted Formation pass writes it.
const SCOPED_RULE: &str = r#"MUTATE {
    CREATE CONCEPT ?rule {
        TYPE "Insight" NAME "No Friday deploys"
        SET ATTRIBUTES {summary: "Never deploy on Fridays", insight_class: "constraint"}
        SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts}
    }
}"#;

/// Observes one scoped rule in `scope` and waits until it is formed.
async fn observe_scoped_rule(f: &Fixture, key: &str, scope: &Json) {
    f.script.write(SCOPED_RULE);
    f.script.done();
    let source = stage(
        &f.space,
        key,
        "Never deploy on Fridays",
        "2026-09-20T00:00:00.000Z",
    )
    .await;
    let observed = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some(&format!("observe:{key}")),
                Some(scope.clone()),
                json!({"source_ref": source}),
            ),
        )
        .await;
    let receipt = observed.receipt.expect("a receipt").receipt_ref;
    assert_eq!(
        wait_available(&f.space, &receipt).await.disposition,
        Some(wire::Disposition::Formed)
    );
}

/// The scoped action recall a later task makes.
async fn recall_in_scope(f: &Fixture, scope: &Json) -> Briefing {
    f.script.push(Step::Final("Plan the deployment.".into()));
    briefing(
        &f.space
            .memory_request(
                NS,
                false,
                request(
                    "recall",
                    None,
                    Some(scope.clone()),
                    json!({"query": "When can I deploy?", "mode": "action"}),
                ),
            )
            .await,
    )
}

fn constraints(brief: &Briefing) -> usize {
    brief
        .items
        .iter()
        .filter(|item| item.role == wire::ItemRole::Constraint)
        .count()
}

async fn handle_state(f: &Fixture, handle: &str) -> Json {
    let response = f
        .space
        .execute_kip_readonly(crate::kip::request_with(
            "FIND(?e._system.state) WHERE { ?e {type: \"Event\", key: :key, state: ?state} } LIMIT 1",
            crate::kip::param("key", format!("memory_scope:{handle}")),
        ))
        .await
        .unwrap();
    crate::kip::ok_result(&response)
        .and_then(|rows| rows.as_array()?.first().cloned())
        .map(crate::agents::first_row)
        .unwrap_or(Json::Null)
}

#[tokio::test]
async fn a_scope_handle_retired_by_maintenance_keeps_its_scope() {
    // To Maintenance the handle is an Event like any other: the reference
    // policy archives stale Events (and its retention sweep archives lapsed
    // ones), tombstones what is withdrawn and merges duplicates. Each goes
    // through the maintenance gate, which cannot tell a handle from the text.
    for (name, command) in [
        (
            "archived",
            r#"TRANSITION ?e TO "archived" WHERE { ?e {type: "Event"} } LIMIT 20"#,
        ),
        (
            "tombstoned",
            r#"TRANSITION ?e TO "tombstoned" WHERE { ?e {type: "Event", key: "memory_scope:release"} } LIMIT 1"#,
        ),
        (
            "merged",
            r#"MERGE CONCEPT ?a INTO ?b WHERE { ?a {type: "Event", key: "memory_scope:release"} ?b {type: "Event", name: "release week"} }"#,
        ),
    ] {
        let f = fixture(&format!("mi_scope_handle_{name}")).await;
        let scope = json!({"task_ref": "release"});
        observe_scoped_rule(&f, "r1", &scope).await;
        let nexus = f.space.memory.nexus();
        let other = anda_kip::execute_request(
            nexus.as_ref(),
            &crate::kip::request(
                r#"CREATE CONCEPT ?e {TYPE "Event" NAME "release week" SET ATTRIBUTES {event_class: "conversation", summary: "Release week"}}"#,
            ),
        )
        .await;
        assert!(crate::kip::succeeded(&other), "{other:?}");
        let retired =
            crate::kip::execute_maintenance_request(nexus.as_ref(), &crate::kip::request(command))
                .await;
        assert!(crate::kip::succeeded(&retired), "{name}: {retired:?}");
        assert_eq!(handle_state(&f, "release").await, json!(name));

        // The scope still resolves to the id its memory was written under,
        // so the rule is recalled rather than lost with the handle.
        assert_eq!(constraints(&recall_in_scope(&f, &scope).await), 1, "{name}");
        // A new observation joins the same scope, and recall sees both.
        observe_scoped_rule(&f, "r2", &scope).await;
        assert_eq!(constraints(&recall_in_scope(&f, &scope).await), 2, "{name}");
        f.space.close().await.unwrap();
    }
}

#[tokio::test]
async fn source_forget_erases_owned_constraints_but_keeps_shared_concepts() {
    let f = fixture("mi_source_concept").await;
    f.script.write(r#"MUTATE {
        UPSERT CONCEPT ?person { MATCH {type: "Person", key: "shared-person"} SET FIELDS {name: "Alice"} }
        CREATE CONCEPT ?rule { TYPE "Insight" NAME "No Friday deploys"
            SET ATTRIBUTES {summary: "Never deploy on Fridays", insight_class: "constraint"}
            SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts} }
    }"#);
    f.script.done();
    let source = stage(
        &f.space,
        "s",
        "Never deploy on Fridays",
        "2026-01-01T00:00:00.000Z",
    )
    .await;
    let observed = f
        .space
        .memory_request(
            NS,
            false,
            request(
                "observe",
                Some("o"),
                Some(json!({"task_ref":"release"})),
                json!({"source_ref":source}),
            ),
        )
        .await;
    wait_available(&f.space, &observed.receipt.unwrap().receipt_ref).await;
    let forgotten = f
        .space
        .memory_request(
            NS,
            true,
            request(
                "forget",
                Some("f"),
                None,
                json!({"target_ref":source,"mode":"semantic"}),
            ),
        )
        .await;
    assert_eq!(
        forgotten.result.as_ref().unwrap()["status"],
        "completed",
        "{forgotten:?}"
    );
    let rules = f
        .space
        .execute_kip_readonly(crate::kip::request(
            "FIND(?c) WHERE { ?c {type: \"Insight\"} } LIMIT 5",
        ))
        .await
        .unwrap();
    assert_eq!(crate::kip::ok_result(&rules).unwrap(), &json!([]));
    let people = f
        .space
        .execute_kip_readonly(crate::kip::request(
            "FIND(?c) WHERE { ?c {key: \"shared-person\"} } LIMIT 5",
        ))
        .await
        .unwrap();
    assert_eq!(
        crate::kip::ok_result(&people)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        f.space
            .staged_memory_source(NS, &source)
            .await
            .unwrap()
            .messages
            .is_empty()
    );
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn forget_a_proposition_with_multiple_assertions_cleans_every_source() {
    let f = fixture("mi_forget_overlap").await;
    let mut sources = Vec::new();
    for (key, at) in [
        ("first", "2026-01-01T00:00:00.000Z"),
        ("second", "2026-01-02T00:00:00.000Z"),
    ] {
        f.script.write(&prefers("dark", "ColorScheme", at, false));
        f.script.done();
        let source = stage(&f.space, key, "I prefer dark mode", at).await;
        let response = f
            .space
            .memory_request(
                NS,
                false,
                request("observe", Some(key), None, json!({"source_ref":source})),
            )
            .await;
        wait_available(&f.space, &response.receipt.unwrap().receipt_ref).await;
        sources.push(source);
    }
    let rows = claims(&f.space, "dark").await;
    assert_eq!(rows.len(), 2);
    let record = f
        .space
        .product_record(rows[0]["id"].as_str().unwrap())
        .await
        .unwrap();
    let forgotten = f
        .space
        .memory_request(
            NS,
            true,
            request(
                "forget",
                Some("f"),
                None,
                json!({"target_ref":record.proposition_id,"mode":"semantic"}),
            ),
        )
        .await;
    assert_eq!(forgotten.status, Status::Succeeded, "{forgotten:?}");
    for source in sources {
        assert!(
            f.space
                .staged_memory_source(NS, &source)
                .await
                .unwrap()
                .erased
        );
    }
    f.space.close().await.unwrap();
}

#[tokio::test]
async fn close_waits_for_memory_interface_work() {
    let f = fixture("mi_close_owner").await;
    let (release, wait) = tokio::sync::oneshot::channel();
    let work = f
        .space
        .memory_interface
        .tasks
        .start(async move {
            wait.await?;
            Ok(())
        })
        .unwrap();
    let space = f.space.clone();
    let close = tokio::spawn(async move { space.close().await });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        !close.is_finished(),
        "close must wait for admitted Memory Interface work"
    );
    release.send(()).unwrap();
    work.await.unwrap().unwrap();
    close.await.unwrap().unwrap();
}

#[tokio::test]
async fn recall_and_expansion_keep_the_snapshot_while_memory_changes() {
    let f = fixture("mi_snapshot").await;
    let nexus = f.space.memory.nexus();
    let created = anda_kip::execute_request(nexus.as_ref(), &crate::kip::request(
        r#"CREATE CONCEPT ?rule { TYPE "Insight" NAME "Release rule" SET ATTRIBUTES {summary: "old rule", insight_class: "constraint"} }"#
    )).await;
    let id = crate::kip::ok_result(&created).unwrap()["handles"]["rule"]
        .as_str()
        .unwrap()
        .to_string();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    f.script.read(&format!(
        "FIND(?c) WHERE {{ ?c CONCEPT {{id: \"{id}\"}} }} LIMIT 1"
    ));
    f.script.push(Step::Barrier {
        entered: entered.clone(),
        release: release.clone(),
    });
    f.script.push(Step::Final("old rule".into()));
    let space = f.space.clone();
    let recall = tokio::spawn(async move {
        space
            .memory_request(
                NS,
                false,
                request("recall", None, None, json!({"query":"Which release rule?"})),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let updated = anda_kip::execute_request(
        nexus.as_ref(),
        &crate::kip::request_with(
            r#"UPDATE :id SET ATTRIBUTES {summary: "new rule"}"#,
            crate::kip::param("id", id.as_str()),
        ),
    )
    .await;
    assert!(crate::kip::succeeded(&updated), "{updated:?}");
    release.notify_one();
    let response = recall.await.unwrap();
    let brief = briefing(&response);
    assert!(
        brief
            .items
            .iter()
            .any(|item| item.text.contains("old rule")),
        "{brief:?}"
    );
    assert!(
        brief
            .items
            .iter()
            .all(|item| !item.text.contains("new rule"))
    );
    let expanded = briefing(
        &f.space
            .memory_request(
                NS,
                false,
                request(
                    "recall",
                    None,
                    None,
                    json!({"target_ref":brief.basis_ref,"detail":"evidence"}),
                ),
            )
            .await,
    );
    let details = expanded.details.as_ref().unwrap();
    assert!(
        details
            .elements
            .iter()
            .any(|row| row["id"] == id && row["attributes"]["summary"] == "old rule")
    );
    assert!(expanded.uncertainties.is_empty(), "{expanded:?}");
    f.space.close().await.unwrap();
}
