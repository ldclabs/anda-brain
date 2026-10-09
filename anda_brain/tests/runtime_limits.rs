use anda_brain::{
    agents::SELF_USER_ID,
    payload::StringOr,
    space::{AppState, Space},
    types::{MaintenanceInput, MemoryPolicy, UpdateSpaceInput},
};
use anda_core::{AgentOutput, BoxError, BoxPinFut, CompletionRequest, ToolCall, Usage};
use anda_db::{database::DBConfig, storage::StorageConfig};
use anda_engine::{
    management::{BaseManagement, Visibility},
    model::{CompletionFeaturesDyn, Model, Models, reqwest},
    unix_ms,
};
use object_store::memory::InMemory;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct Mock {
    calls: Arc<AtomicUsize>,
    pending: bool,
}
impl CompletionFeaturesDyn for Mock {
    fn model_name(&self) -> String {
        "review-local-mock".into()
    }
    fn completion(&self, req: CompletionRequest) -> BoxPinFut<Result<AgentOutput, BoxError>> {
        let calls = self.calls.clone();
        let pending = self.pending;
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if pending {
                return std::future::pending().await;
            }
            let usage = Usage {
                requests: 1,
                input_tokens: 100_000,
                output_tokens: 1,
                ..Default::default()
            };
            if req.tools.is_empty() {
                return Ok(AgentOutput {
                    content: "handoff".into(),
                    usage,
                    ..Default::default()
                });
            }
            Ok(AgentOutput {
                tool_calls: vec![ToolCall {
                    name: "execute_kip_readonly".into(),
                    args: serde_json::json!({"command":"DESCRIBE PRIMER"}),
                    call_id: Some("review-call".into()),
                    remote_id: None,
                    result: None,
                }],
                usage,
                ..Default::default()
            })
        })
    }
}
fn app(calls: Arc<AtomicUsize>, pending: bool) -> AppState {
    let models = Models::default();
    let mut model = Model::with_completer(Arc::new(Mock { calls, pending }));
    if !pending {
        model.context_window = 1;
    }
    models.set_model(model);
    AppState::new(
        Arc::new(InMemory::new()),
        Arc::new(DBConfig {
            name: "review".into(),
            description: "review".into(),
            storage: StorageConfig::default(),
            lock: None,
        }),
        Arc::new(BaseManagement {
            controller: SELF_USER_ID,
            managers: BTreeSet::new(),
            visibility: Visibility::Public,
        }),
        reqwest::Client::new(),
        Arc::new(models),
        Arc::new(vec![]),
        "review".into(),
        "0.13.4".into(),
        0,
    )
    .with_llm_concurrency(1)
}
async fn space(app: &AppState, id: &str) -> Arc<Space> {
    app.admin_create_space(SELF_USER_ID, SELF_USER_ID, id.into(), 1, unix_ms())
        .await
        .unwrap();
    app.load_space(id, false).await.unwrap()
}
async fn await_calls(calls: &AtomicUsize, n: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while calls.load(Ordering::SeqCst) < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn manual_maintenance_cannot_overlap_formation() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone(), true);
    let space = space(&app, "review_overlap").await;
    space
        .ingest(
            SELF_USER_ID,
            StringOr::String("Remember that Ada likes tea.".into()),
        )
        .await
        .unwrap();
    await_calls(&calls, 1).await;
    let result = space
        .maintenance(SELF_USER_ID, MaintenanceInput::default())
        .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let status = space.formation_status();
    assert!(status.formation_processing && !status.maintenance_processing);
    space.close().await.unwrap();
}
#[tokio::test]
async fn background_model_calls_hold_shared_limit_until_completion() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone(), true);
    let a = space(&app, "review_budget_a").await;
    let b = space(&app, "review_budget_b").await;
    for space in [&a, &b] {
        let permit = app.llm_request_semaphore().try_acquire().unwrap();
        space
            .ingest(
                SELF_USER_ID,
                StringOr::String("Remember that Ada likes tea.".into()),
            )
            .await
            .unwrap();
        drop(permit); // Same lifetime as the HTTP route's concurrency permit.
    }
    await_calls(&calls, 1).await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.llm_semaphore().available_permits(), 0);
    // Closing the first owner cancels its call and releases exactly one slot.
    // Which Space acquired first is deliberately not assumed.
    if a.formation_status().formation_processing && b.formation_status().formation_processing {
        a.close().await.unwrap();
        b.close().await.unwrap();
    } else {
        panic!("both queued Formation workers must remain owned");
    }
    assert_eq!(app.llm_semaphore().available_permits(), 1);
}
#[tokio::test]
async fn compaction_consumes_the_last_recall_round() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(calls.clone(), false);
    let space = space(&app, "review_turns").await;
    space
        .update(
            UpdateSpaceInput {
                memory_policy: Some(MemoryPolicy {
                    recall_max_rounds: 2,
                    ..Default::default()
                }),
                ..Default::default()
            },
            unix_ms(),
        )
        .await
        .unwrap();
    let output = space
        .query(SELF_USER_ID, StringOr::String("What does Ada like?".into()))
        .await
        .unwrap();
    assert!(
        output
            .failed_reason
            .as_deref()
            .unwrap_or_default()
            .contains("turn limit of 2")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    space.close().await.unwrap();
}
