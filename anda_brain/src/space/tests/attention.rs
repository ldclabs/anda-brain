use super::*;
mod r7;
use crate::attention::AttentionPolicy;
use anda_cognitive_nexus::nexus::DEFAULT_SPACE;
use anda_object_store::fault::{FaultGate, FaultHandle, FaultKind, FaultOp, FaultRule, FaultStore};
use serde_json::{Value, json};

fn runtime_app(store: Arc<dyn object_store::ObjectStore>, policy: AttentionPolicy) -> AppState {
    let template = test_app_state("r2");
    let mut app = template.fork_with_store(store);
    app.automatic = true;
    app.attention_policy = policy;
    app
}

fn fast_policy() -> AttentionPolicy {
    AttentionPolicy {
        tick_ms: 20,
        reconcile_ms: 100,
        blocked_retry_ms: 200,
        ..Default::default()
    }
}

async fn new_watch(
    space: &Space,
    target: &str,
    class: &str,
    due: Option<u64>,
    text: bool,
) -> String {
    let mut condition = json!({"element":target,"ops":["update"]});
    if text {
        condition["text"] = json!("a verified reply");
    }
    let mut params = kip::param("condition", condition);
    params.insert("class".into(), json!(class));
    params.insert(
        "due".into(),
        due.map(|t| json!(kip::timestamp(t))).unwrap_or(Value::Null),
    );
    let reference = created_ref(space, r#"CREATE CONCEPT ?item {TYPE "Watch" SET ATTRIBUTES {watch_class: :class, summary:"R2 deadline",status:"disarmed",condition: :condition,due_at: :due}}"#, params).await;
    runtime_work(space, "arm_watch", &reference).await;
    reference
}

async fn target(space: &Space) -> String {
    created_ref(
        space,
        r#"CREATE CONCEPT ?item {TYPE "Person" NAME "before"}"#,
        Default::default(),
    )
    .await
}

async fn evict_all(app: &AppState) {
    for entry in app.spaces.read().await.values() {
        entry.last_access_ms.store(0, Ordering::Relaxed);
    }
    app.flush_and_evict_once(unix_ms(), 1).await;
    assert!(
        app.spaces.read().await.is_empty(),
        "fixture must actually evict"
    );
}

async fn wake_count(space: &Space) -> usize {
    space
        .memory
        .nexus()
        .system_session()
        .list_wakes(DEFAULT_SPACE, None, 200)
        .await
        .unwrap()
        .items
        .len()
}

fn fault_app() -> (AppState, FaultHandle) {
    let (store, fault) = FaultStore::wrap(InMemory::new());
    (runtime_app(Arc::new(store), fast_policy()), fault)
}

#[tokio::test]
async fn r2_background_finds_evicted_space_without_an_api_load() {
    let app = runtime_app(Arc::new(InMemory::new()), fast_policy());
    let space = create_loaded_space(&app, "r2_cold").await;
    let target = target(&space).await;
    let watch = new_watch(&space, &target, "silence", Some(unix_ms() + 500), false).await;
    drop(space);
    evict_all(&app).await;
    let restarted = runtime_app(app.object_store(), fast_policy());
    assert!(restarted.spaces.read().await.is_empty());
    let cancel = CancellationToken::new();
    let worker = {
        let host = restarted.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { host.start_background_tasks(cancel).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            // Inspect the cache only: the worker must be the caller that loads it.
            let loaded = restarted
                .spaces
                .read()
                .await
                .get("r2_cold")
                .and_then(|e| e.cell.get())
                .cloned();
            if let Some(space) = loaded
                && watch_state(&space, &watch).await[0] == "fired"
            {
                assert_eq!(restarted.spaces.read().await["r2_cold"].last_access_ms(), 0);
                assert_eq!(wake_count(&space).await, 1);
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn r2_registration_failure_prevents_arming_and_lost_scan_ack_is_recoverable() {
    let (app, faults) = fault_app();
    let space = create_loaded_space(&app, "r2_fault").await;
    let target = target(&space).await;
    let watch = created_ref(&space, r#"CREATE CONCEPT ?item {TYPE "Watch" SET ATTRIBUTES {watch_class:"delta",summary:"register first",status:"disarmed",condition:{element: :target}}}"#, kip::param("target", target.clone())).await;
    faults.push_rule(FaultRule::fail_once(FaultOp::Put, "/index/"));
    assert!(space.attention().arm_watch(watch.clone(), 1).await.is_err());
    assert_eq!(watch_state(&space, &watch).await[0], "disarmed");
    faults.reset();
    space.attention().arm_watch(watch.clone(), 1).await.unwrap();
    seed_kip(
        &space,
        kip::request_with(
            r#"UPDATE :id SET FIELDS {name:"changed"}"#,
            kip::param("id", target),
        ),
    )
    .await;
    faults.push_rule(FaultRule::fail_once(
        FaultOp::Put,
        format!("__brain_runtime__/v1/shards/{}/spaces/", app.sharding),
    ));
    let result = space.attention().tick().await;
    assert!(result.is_err(), "{result:?}");
    assert_eq!(watch_state(&space, &watch).await[0], "fired");
    assert_eq!(wake_count(&space).await, 1);
    faults.reset();
    drop(space);
    evict_all(&app).await;
    let restarted = runtime_app(app.object_store(), fast_policy());
    restarted.attention_tick().await.unwrap();
    let loaded = restarted
        .load_space_with("r2_fault", false, false)
        .await
        .unwrap();
    assert_eq!(wake_count(&loaded).await, 1);
    let status = loaded.attention().status().await.unwrap().unwrap();
    assert_eq!(
        status.registration.dirty_generation,
        status.registration.reconciled_generation
    );
    loaded.close().await.unwrap();
}

#[tokio::test]
async fn r2_dropped_waiter_does_not_cancel_native_write_and_close_drains_it() {
    let (app, faults) = fault_app();
    let space = create_loaded_space(&app, "r2_drain").await;
    let target = target(&space).await;
    let watch = created_ref(&space, r#"CREATE CONCEPT ?item {TYPE "Watch" SET ATTRIBUTES {watch_class:"delta",summary:"drain",status:"disarmed",condition:{element: :target}}}"#, kip::param("target",target)).await;
    // Complete directory setup so the paused native PUT belongs to arm itself.
    space.attention().register_work().await.unwrap();
    let gate = FaultGate::new();
    faults.push_rule(FaultRule {
        kind: FaultKind::PauseAfter(gate.clone()),
        ..FaultRule::fail_once(FaultOp::Put, "kip_control_records")
    });
    let waiter = {
        let runtime = space.attention();
        let watch = watch.clone();
        tokio::spawn(async move { runtime.arm_watch(watch, 1).await })
    };
    tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
        .await
        .unwrap();
    waiter.abort();
    let _ = waiter.await;
    assert!(space.is_busy());
    let closing = {
        let space = space.clone();
        tokio::spawn(async move { space.close().await })
    };
    sleep(Duration::from_millis(20)).await;
    assert!(!closing.is_finished());
    gate.release();
    closing.await.unwrap().unwrap();
    let restarted = runtime_app(app.object_store(), fast_policy());
    let loaded = restarted
        .load_space_with("r2_drain", false, false)
        .await
        .unwrap();
    assert_eq!(watch_state(&loaded, &watch).await[0], "armed");
    loaded.close().await.unwrap();
}

#[tokio::test]
async fn r2_slot_cursor_and_structured_cursor_are_fair_across_restart() {
    let policy = AttentionPolicy {
        spaces_per_tick: 1,
        watches_per_space: 1,
        ..fast_policy()
    };
    let app = runtime_app(Arc::new(InMemory::new()), policy.clone());
    for id in ["r2_one", "r2_two", "r2_three"] {
        let space = create_loaded_space(&app, id).await;
        let target = target(&space).await;
        for _ in 0..5 {
            new_watch(&space, &target, "delta", None, true).await;
        }
        for _ in 0..3 {
            new_watch(&space, &target, "delta", None, false).await;
        }
        seed_kip(
            &space,
            kip::request_with(
                r#"UPDATE :id SET FIELDS {name:"ready"}"#,
                kip::param("id", target),
            ),
        )
        .await;
    }
    evict_all(&app).await;
    // Each restart must continue both the shard and per-Space cursors.
    for _ in 0..9 {
        let restarted = runtime_app(app.object_store(), policy.clone());
        let report = restarted.attention_tick().await.unwrap();
        assert!(report.loaded <= 1);
        assert_eq!(report.fired, 1, "{report:?}");
        evict_all(&restarted).await;
    }
    let restarted = runtime_app(app.object_store(), policy);
    for id in ["r2_one", "r2_two", "r2_three"] {
        let space = restarted.load_space_with(id, false, false).await.unwrap();
        assert_eq!(wake_count(&space).await, 3);
        assert!(
            !space
                .attention()
                .status()
                .await
                .unwrap()
                .unwrap()
                .last_report
                .scan_complete
        );
        space.close().await.unwrap();
    }
}

#[tokio::test]
async fn r2_disabled_registrations_and_isolated_hosts_never_autoload() {
    let app = runtime_app(Arc::new(InMemory::new()), fast_policy());
    let space = create_loaded_space(&app, "r2_disabled").await;
    let target = target(&space).await;
    new_watch(&space, &target, "silence", Some(unix_ms() + 100), false).await;
    space.attention().set_enabled(false).await.unwrap();
    drop(space);
    evict_all(&app).await;
    let isolated = app.fork_with_store(app.object_store());
    assert_eq!(isolated.attention_tick().await.unwrap().visited, 0);
    let restarted = runtime_app(app.object_store(), fast_policy());
    let report = restarted.attention_tick().await.unwrap();
    assert_eq!(report.loaded, 0);
    assert_eq!(report.skipped, 1);
    assert!(restarted.spaces.read().await.is_empty());
}

#[tokio::test]
async fn r2_native_expiry_schedules_revalidation_without_a_cognitive_write() {
    let app = runtime_app(Arc::new(InMemory::new()), fast_policy());
    let space = create_loaded_space(&app, "r2_expiry").await;
    declare_types(&space, &["AnswerStyle"]).await;
    space.attention().register_work().await.unwrap();
    let until = unix_ms() + 10_000;
    seed_kip(&space, kip::request_with(r#"MUTATE {
      CREATE CONCEPT ?person {TYPE "Person" NAME "Expiry actor"}
      CREATE CONCEPT ?pref {TYPE "AnswerStyle" NAME "Short preference"}
      ENSURE PROPOSITION ?p (?person,"prefers",?pref)
      CREATE ASSERTION ?a {SET FIELDS {proposition:?p,asserted_by:?person,stance:"support",mode:"stated",confidence:0.9,valid_time:{until: :until}}}
    }"#,kip::param("until",kip::timestamp(until)))).await;
    let before = space
        .memory
        .nexus()
        .store
        .get_space(DEFAULT_SPACE)
        .await
        .unwrap()
        .seq;
    let req = kip::request(r#"FIND(?b) WHERE {?p (?s,"prefers",?o) ?b BELIEF(?p)}"#);
    let result = space.execute_kip_readonly(req.clone()).await.unwrap();
    let projection: anda_kip::Projection =
        serde_json::from_value(kip::ok_result(&result).unwrap()[0].clone()).unwrap();
    space
        .attention()
        .observe_basis(projection.basis.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        space
            .memory
            .nexus()
            .store
            .get_space(DEFAULT_SPACE)
            .await
            .unwrap()
            .seq,
        before
    );
    let status = space.attention().status().await.unwrap().unwrap();
    assert!(status.rechecks.iter().any(|r| r.due_at_ms == until
        && r.source == crate::attention::RecheckSource::DependencyInvalidation));
    space
        .attention()
        .schedule_recheck(crate::attention::Recheck {
            key: "due-review".into(),
            source: crate::attention::RecheckSource::SkillReview,
            due_at_ms: unix_ms(),
            notified: false,
        })
        .await
        .unwrap();
    let seq = space
        .memory
        .nexus()
        .store
        .get_space(DEFAULT_SPACE)
        .await
        .unwrap()
        .seq;
    let report = space.attention().tick().await.unwrap();
    assert_eq!(report.rechecks_due, 1);
    assert_eq!(
        space
            .memory
            .nexus()
            .store
            .get_space(DEFAULT_SPACE)
            .await
            .unwrap()
            .seq,
        seq
    );
    assert!(
        space
            .attention()
            .status()
            .await
            .unwrap()
            .unwrap()
            .rechecks
            .iter()
            .any(|r| r.key == "due-review" && r.notified)
    );
    space.close().await.unwrap();
}

#[tokio::test]
async fn r2_timed_wake_resume_and_live_lease_are_observed() {
    use anda_cognitive_nexus::attention::{WakeResume, WakeRetry, WakeState};
    let app = runtime_app(Arc::new(InMemory::new()), fast_policy());
    let space = create_loaded_space(&app, "r2_resume").await;
    let target = target(&space).await;
    new_watch(&space, &target, "delta", None, false).await;
    seed_kip(
        &space,
        kip::request_with(
            r#"UPDATE :id SET FIELDS {name:"resume event"}"#,
            kip::param("id", target),
        ),
    )
    .await;
    space.attention().tick().await.unwrap();
    let nexus = space.memory.nexus();
    let session = nexus.system_session();
    let wake = session
        .list_wakes(DEFAULT_SPACE, None, 200)
        .await
        .unwrap()
        .items
        .remove(0);
    session
        .claim_wake(
            DEFAULT_SPACE,
            &wake.wake_ref,
            1,
            0,
            &kip::timestamp(unix_ms() + 5_000),
        )
        .await
        .unwrap();
    space.attention().tick().await.unwrap();
    assert!(space.is_busy(), "a retained live lease prevents eviction");
    let due = unix_ms() + 100;
    session
        .block_wake(
            DEFAULT_SPACE,
            &wake.wake_ref,
            2,
            1,
            WakeRetry {
                reason: "budget_exhausted".into(),
                resume: WakeResume::At { not_before_ms: due },
            },
        )
        .await
        .unwrap();
    space.attention().tick().await.unwrap();
    assert!(matches!(
        session
            .read_wake(DEFAULT_SPACE, &wake.wake_ref)
            .await
            .unwrap()
            .state,
        WakeState::Blocked { .. }
    ));
    sleep(Duration::from_millis(120)).await;
    assert_eq!(space.attention().tick().await.unwrap().resumed, 1);
    assert!(matches!(
        session
            .read_wake(DEFAULT_SPACE, &wake.wake_ref)
            .await
            .unwrap()
            .state,
        WakeState::Pending { .. }
    ));
    space.close().await.unwrap();
}

struct SlowResume(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl anda_cognitive_nexus::attention::WakeResumeVerifier for SlowResume {
    async fn verify(
        &self,
        _: anda_cognitive_nexus::attention::WakeResumeInput,
    ) -> Result<bool, anda_kip::KipError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        sleep(Duration::from_secs(1)).await;
        Ok(true)
    }
}

#[tokio::test]
async fn r2_resume_budget_retains_unprocessed_page_and_persistent_backoff() {
    use anda_cognitive_nexus::attention::{RuntimePin, WakeResume, WakeRetry};
    let policy = AttentionPolicy {
        wall_time_ms: 100,
        blocked_retry_ms: 60_000,
        ..fast_policy()
    };
    let app = runtime_app(Arc::new(InMemory::new()), policy);
    let space = create_loaded_space(&app, "r2_resume_page").await;
    let target = target(&space).await;
    for _ in 0..3 {
        new_watch(&space, &target, "delta", None, false).await;
    }
    seed_kip(
        &space,
        kip::request_with(
            r#"UPDATE :id SET FIELDS {name:"page"}"#,
            kip::param("id", target),
        ),
    )
    .await;
    // Native fires first, then independent blocked work is installed by a host.
    for _ in 0..4 {
        space.attention().tick().await.unwrap();
        if wake_count(&space).await == 3 {
            break;
        }
    }
    assert_eq!(wake_count(&space).await, 3);
    let verifier = Arc::new(SlowResume(std::sync::atomic::AtomicUsize::new(0)));
    let digest = space
        .attention()
        .register_resume_verifier(
            json!({"configured":true}),
            RuntimePin {
                id: "bounded-test".into(),
                digest: format!("sha256:{}", "c".repeat(64)),
            },
            verifier.clone(),
        )
        .unwrap();
    let nexus = space.memory.nexus();
    let session = nexus.system_session();
    for wake in session
        .list_wakes(DEFAULT_SPACE, None, 200)
        .await
        .unwrap()
        .items
    {
        session
            .claim_wake(
                DEFAULT_SPACE,
                &wake.wake_ref,
                1,
                0,
                &kip::timestamp(unix_ms() + 5_000),
            )
            .await
            .unwrap();
        session
            .block_wake(
                DEFAULT_SPACE,
                &wake.wake_ref,
                2,
                1,
                WakeRetry {
                    reason: "binding_unavailable".into(),
                    resume: WakeResume::OnChange {
                        condition_digest: digest.clone(),
                    },
                },
            )
            .await
            .unwrap();
    }
    let first = space.attention().tick().await.unwrap();
    assert!(first.budget_exhausted);
    assert_eq!(verifier.0.load(Ordering::SeqCst), 1);
    for _ in 0..4 {
        space.attention().tick().await.unwrap();
    }
    assert_eq!(
        verifier.0.load(Ordering::SeqCst),
        3,
        "each page item tried once; expired model calls never recover work"
    );
    space.attention().tick().await.unwrap();
    assert_eq!(
        verifier.0.load(Ordering::SeqCst),
        3,
        "persisted backoff suppresses another call"
    );
    space.close().await.unwrap();
}

#[tokio::test]
async fn r2_twenty_spaces_two_hundred_watches_meet_deadline_p95() {
    let policy = AttentionPolicy::default();
    let app = runtime_app(Arc::new(InMemory::new()), policy.clone());
    let mut deadlines = Vec::new();
    for n in 0..20 {
        let id = format!("r2_load_{n:02}");
        let space = create_loaded_space(&app, &id).await;
        let target = target(&space).await;
        let due = unix_ms() + 500;
        for _ in 0..10 {
            new_watch(&space, &target, "silence", Some(due), false).await;
        }
        deadlines.push((id, due));
    }
    evict_all(&app).await;
    let restarted = runtime_app(app.object_store(), policy.clone());
    let started = tokio::time::Instant::now();
    let mut fired = 0;
    while fired < 200 {
        let report = restarted.attention_tick().await.unwrap();
        fired += report.fired;
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{report:?}, fired={fired}"
        );
        if fired < 200 {
            sleep(Duration::from_millis(policy.tick_ms)).await;
        }
    }
    let mut lags = Vec::new();
    for (id, due) in deadlines {
        let space = restarted.load_space_with(&id, false, false).await.unwrap();
        assert_eq!(wake_count(&space).await, 10);
        let result=space.execute_kip_readonly(kip::request(r#"FIND(?a._system.created_at) WHERE {?a ACTIVITY {activity_class:"watch_fire"}} LIMIT 20"#)).await.unwrap();
        for at in kip::ok_result(&result).unwrap().as_array().unwrap() {
            let utc = anda_cognitive_nexus::time::parse(at.as_str().unwrap()).unwrap();
            let at = kip::unix_time_ms(utc).unwrap();
            lags.push(at.saturating_sub(due));
        }
        space.close().await.unwrap();
    }
    lags.sort_unstable();
    assert_eq!(lags.len(), 200);
    let p95 = lags[189];
    println!(
        "R2_PROFILE spaces=20 watches=200 tick_ms=5000 deadline_to_wake_p95_ms={p95} max_ms={}",
        lags[199]
    );
    assert!(p95 <= 60_000, "deadline P95: {p95}ms");
}

async fn watch_state(space: &Space, reference: &str) -> Value {
    let response = space.execute_kip_readonly(kip::request_with(
        "FIND(?w.attributes.status, ?w.facets[\"WatchState\"]) WHERE {?w CONCEPT {id: :id}} LIMIT 1",
        kip::param("id", reference),
    )).await.unwrap();
    assert!(
        kip::succeeded(&response),
        "{}",
        kip::error_message(&response)
    );
    kip::ok_result(&response).unwrap()[0].clone()
}

#[tokio::test]
async fn eviction_preserves_watch_generation_and_unconsumed_changes() {
    let app = test_app_state("attention_eviction");
    let id = "attention_eviction_space";
    let space = create_loaded_space(&app, id).await;
    let target = created_ref(
        &space,
        r#"CREATE CONCEPT ?item {TYPE "Person" NAME "before"}"#,
        Default::default(),
    )
    .await;
    let watch = created_ref(&space, r#"CREATE CONCEPT ?item {TYPE "Watch" SET ATTRIBUTES {watch_class:"delta",summary:"retain attention",status:"disarmed",condition:{element: :target,ops:["update"]},due_at:"2099-01-01T00:00:00.000Z"}}"#, kip::param("target", target.as_str())).await;
    runtime_work(&space, "arm_watch", &watch).await;
    let armed = watch_state(&space, &watch).await;
    seed_kip(
        &space,
        kip::request_with(
            r#"UPDATE :id SET FIELDS {name:"after"}"#,
            kip::param("id", target),
        ),
    )
    .await;
    {
        let spaces = app.spaces.read().await;
        spaces
            .get(id)
            .unwrap()
            .last_access_ms
            .store(0, Ordering::Relaxed);
    }
    drop(space);
    app.flush_and_evict_once(unix_ms(), 1).await;
    assert!(
        !app.spaces.read().await.contains_key(id),
        "test must actually evict the Space"
    );
    // A new AppState has no resident-space map. This checks persistence, not
    // automatic deadline scheduling (R2 owns that separate acceptance case).
    let restarted = app.fork_with_store(app.object_store());
    let reloaded = restarted.load_space_with(id, false, false).await.unwrap();
    assert_eq!(watch_state(&reloaded, &watch).await, armed);
    let report = settlement::sweep_watches(reloaded.as_ref()).await;
    assert_eq!(report.fired, 1, "{report:?}");
    assert!(report.error.is_none(), "{report:?}");
    let fired = watch_state(&reloaded, &watch).await;
    assert_eq!(fired[0], json!("fired"));
    assert_eq!(fired[1]["arm_generation"], armed[1]["arm_generation"]);
    assert_eq!(fired[1]["matched"], true);
    reloaded.close().await.unwrap();
}

#[tokio::test]
async fn forks_cannot_duplicate_native_wake_instances() {
    let app = test_app_state("attention_fork");
    let space = create_loaded_space(&app, "attention_fork_space").await;
    let target = created_ref(
        &space,
        r#"CREATE CONCEPT ?item {TYPE "Person" NAME "source"}"#,
        Default::default(),
    )
    .await;
    let watch = created_ref(&space,r#"CREATE CONCEPT ?item {TYPE "Watch" SET ATTRIBUTES {watch_class:"delta",summary:"wait",status:"disarmed",condition:{element: :target}}}"#,kip::param("target",target)).await;
    runtime_work(&space, "arm_watch", &watch).await;
    space.flush().await.unwrap();
    let error = match app.fork_space("attention_fork_space", None).await {
        Ok(_) => {
            panic!("a copied native wake identity must not be exposed as an independent Space")
        }
        Err(error) => error.to_string(),
    };
    assert!(error.contains("native attention state"), "{error}");
    assert_eq!(watch_state(&space, &watch).await[0], "armed");
    space.close().await.unwrap();
}
