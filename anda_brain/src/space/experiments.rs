//! Isolated, exclusively owned runs for trusted local hosts. No HTTP handler
//! returns a run's Space, store or clock. Snapshotting a live service is not
//! supported here: all writes must pass through this serialized owner.
use super::*;
use futures::TryStreamExt;
use object_store::ObjectStoreExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod procedure_audit;
pub use procedure_audit::{ProcedureAudit, ProcedureAuditSkill};

const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;
const MAX_SNAPSHOT_OBJECTS: usize = 100_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    Persistent,
    SessionOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExperimentIdentity {
    pub model_digest: String,
    pub tools_digest: String,
    pub budget_digest: String,
}

impl ExperimentIdentity {
    fn validate(&self) -> Result<(), BoxError> {
        for digest in [&self.model_digest, &self.tools_digest, &self.budget_digest] {
            if !digest.strip_prefix("sha256:").is_some_and(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            }) {
                return Err("experiment requires explicit SHA-256 configuration identities".into());
            }
        }
        Ok(())
    }
}

/// Model/tool digests are attested by the trusted launcher. Prompt identities
/// and snapshot bytes are measured by Brain; secrets are never serialized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentManifest {
    pub format: String,
    pub memory_mode: MemoryMode,
    pub app_name: String,
    pub app_version: String,
    pub identity: ExperimentIdentity,
    pub prompts_digest: String,
    pub business_time_ms: u64,
    pub state_digest: String,
    pub objects: usize,
    pub bytes: usize,
}

/// Immutable material captured only after work is quiescent and fully flushed.
/// Both arms fork these same bytes; each gets its own store, engine and clock.
pub struct ExperimentSnapshot {
    manifest: ExperimentManifest,
    objects: BTreeMap<String, Vec<u8>>,
}

impl ExperimentSnapshot {
    pub fn manifest(&self) -> &ExperimentManifest {
        &self.manifest
    }

    pub async fn fork(
        &self,
        template: &AppState,
        identity: &ExperimentIdentity,
    ) -> Result<Experiment, BoxError> {
        if identity != &self.manifest.identity
            || self.manifest.prompts_digest != prompts_digest(template)?
            || self.manifest.app_name != template.app_name
            || self.manifest.app_version != template.app_version
        {
            return Err("snapshot runtime or prompt identity changed".into());
        }
        let app = isolated_app(template, self.manifest.business_time_ms)?;
        for (path, bytes) in &self.objects {
            app.object_store
                .put(
                    &object_store::path::Path::from(path.as_str()),
                    bytes.clone().into(),
                )
                .await?;
        }
        let space = app.load_space_with("experiment", false, false).await?;
        space
            .db
            .save_extension_from("experiment_costs".into(), &Vec::<StageCost>::new())
            .await?;
        space
            .db
            .save_extension_from("experiment_costs_truncated".into(), &false)
            .await?;
        Ok(Experiment {
            shutdown: CancellationToken::new(),
            run: tokio::sync::Mutex::new(Some(Run {
                app,
                space,
                mode: self.manifest.memory_mode,
                identity: self.manifest.identity.clone(),
                prompts_digest: self.manifest.prompts_digest.clone(),
                costs: Vec::new(),
            })),
        })
    }
}

struct Run {
    app: AppState,
    space: Arc<Space>,
    mode: MemoryMode,
    identity: ExperimentIdentity,
    prompts_digest: String,
    costs: Vec<StageCost>,
}

pub struct Experiment {
    shutdown: CancellationToken,
    run: tokio::sync::Mutex<Option<Run>>,
}

fn isolated_app(template: &AppState, now_ms: u64) -> Result<AppState, BoxError> {
    let mut app = template.fork_with_store(Arc::new(InMemory::new()));
    app.clock = crate::runtime::BusinessClock::manual(now_ms)?;
    Ok(app)
}

async fn fresh_space(app: &AppState) -> Result<Arc<Space>, BoxError> {
    app.admin_create_space(
        SELF_USER_ID,
        SELF_USER_ID,
        "experiment".into(),
        1,
        unix_ms(),
    )
    .await?;
    app.load_space_with("experiment", false, false).await
}

fn prompts_digest(app: &AppState) -> Result<String, BoxError> {
    use crate::agents::prompts::{PromptTarget, budgeted_recall_prompt, system_prompt};
    let prompts = [
        PromptTarget::Formation,
        PromptTarget::Recall,
        PromptTarget::Maintenance,
    ]
    .map(|target| system_prompt(target, &app.prompts.prompt(target)));
    let recall = app.prompts.prompt(PromptTarget::Recall);
    Ok(anda_cognitive_nexus::content_digest(&serde_json::json!({
        "agents": prompts,
        "budgeted_recall": budgeted_recall_prompt(&recall, false),
        "budgeted_selection": budgeted_recall_prompt(&recall, true),
    }))?)
}

async fn quiescent(run: &Run) -> Result<(), BoxError> {
    if run.prompts_digest != prompts_digest(&run.app)? {
        return Err("experiment prompt identity changed".into());
    }
    let space = &run.space;
    if space.is_processing() || !space.tasks.is_idle() {
        return Err("experiment has outstanding work; await its terminal result first".into());
    }
    Ok(())
}

impl Experiment {
    async fn lock_run(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Run>>, BoxError> {
        let guard = self.run.lock().await;
        if self.shutdown.is_cancelled() {
            return Err("experiment is closing".into());
        }
        Ok(guard)
    }

    async fn interruptible<T>(
        &self,
        work: impl std::future::Future<Output = Result<T, BoxError>>,
    ) -> Result<T, BoxError> {
        tokio::select! { biased; _ = self.shutdown.cancelled() => Err("experiment is closing; reconcile any submitted operation before retry".into()), result = work => result }
    }

    pub async fn create(
        template: &AppState,
        mode: MemoryMode,
        identity: ExperimentIdentity,
        now_ms: u64,
    ) -> Result<Self, BoxError> {
        identity.validate()?;
        let app = isolated_app(template, now_ms)?;
        let space = fresh_space(&app).await?;
        Ok(Self {
            shutdown: CancellationToken::new(),
            run: tokio::sync::Mutex::new(Some(Run {
                prompts_digest: prompts_digest(&app)?,
                app,
                space,
                mode,
                identity,
                costs: vec![],
            })),
        })
    }

    /// Creates an isolated run with a forced Recall budget before exposing
    /// it to the caller. The policy survives session boundaries and snapshot
    /// forks; callers can tighten but cannot omit or raise these limits.
    /// Include this budget in the launcher's `ExperimentIdentity` as well.
    pub async fn create_with_recall_budget(
        template: &AppState,
        mode: MemoryMode,
        identity: ExperimentIdentity,
        now_ms: u64,
        budget: crate::recall_budget::RecallBudget,
    ) -> Result<Self, BoxError> {
        budget.validate()?;
        let run = Self::create(template, mode, identity, now_ms).await?;
        {
            let guard = run.lock_run().await?;
            let space = &guard.as_ref().ok_or("experiment is closed")?.space;
            let policy = MemoryPolicy {
                recall_budget: Some(budget),
                ..space.memory_policy()
            };
            space
                .db
                .save_extension_from(MemoryPolicy::EXTENSION_KEY.into(), &policy)
                .await?;
            space.db.flush_metadata(unix_ms()).await?;
        }
        Ok(run)
    }

    /// Enqueues actual Formation. A timeout leaves its id addressable for
    /// reconciliation; never silently retries the submission.
    pub async fn observe(
        &self,
        mut input: FormationInput,
        wait: Duration,
    ) -> Result<ProcessingWait, BoxError> {
        let mut guard = self.lock_run().await?;
        let run = guard.as_mut().ok_or("experiment is closed")?;
        quiescent(run).await?;
        input
            .timestamp
            .get_or_insert_with(|| kip::timestamp(run.space.clock.now_ms()));
        let out = run
            .space
            .ingest(SELF_USER_ID, StringOr::Value(input))
            .await?;
        let id = out
            .conversation
            .ok_or("formation did not return a conversation id")?;
        self.interruptible(
            run.space
                .wait_for_processing(ProcessingKind::Formation, id, wait),
        )
        .await
    }

    pub async fn wait(
        &self,
        kind: ProcessingKind,
        id: u64,
        wait: Duration,
    ) -> Result<ProcessingWait, BoxError> {
        let guard = self.lock_run().await?;
        self.interruptible(
            guard
                .as_ref()
                .ok_or("experiment is closed")?
                .space
                .wait_for_processing(kind, id, wait),
        )
        .await
    }

    pub async fn recall(&self, input: RecallInput) -> Result<AgentOutput, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        self.interruptible(run.space.query(SELF_USER_ID, StringOr::Value(input)))
            .await
    }

    pub async fn maintain(
        &self,
        input: MaintenanceInput,
        wait: Duration,
    ) -> Result<ProcessingWait, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        let out = run.space.maintenance(SELF_USER_ID, input).await?;
        let id = out
            .conversation
            .ok_or("maintenance did not return a conversation id")?;
        self.interruptible(
            run.space
                .wait_for_processing(ProcessingKind::Maintenance, id, wait),
        )
        .await
    }

    /// Raw KIP is for the trusted fixture/launcher, never exposed as an Agent
    /// tool. Model execution still uses the normal guarded Brain agents.
    pub async fn execute_fixture(&self, mut request: Request) -> Result<Response, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        run.space.clock.bind_read(&mut request)?;
        Ok(execute_request(run.space.memory.nexus().as_ref(), &request).await)
    }

    pub async fn advance_to(&self, now_ms: u64) -> Result<(), BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        run.space.clock.advance_to(now_ms)
    }

    /// Run deterministic decay/expiry at the current business time without a
    /// model. Runtime Watch/lease control remains on the engine's real clock.
    pub async fn settle(
        &self,
        scope: MaintenanceScope,
    ) -> Result<MemorySettlementReport, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        run.space
            .settle_memory_metabolism(scope, run.space.clock.now_ms())
            .await
    }

    pub async fn snapshot(&self) -> Result<ExperimentSnapshot, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        quiescent(run).await?;
        #[cfg(feature = "learning")]
        if run.space.learning().is_configured() {
            return Err("snapshot a factual baseline before configuring learning; operational dispatch state cannot be forked".into());
        }
        run.space.flush().await?;
        let mut objects = BTreeMap::new();
        let mut total = 0usize;
        let mut list = run
            .app
            .object_store
            .list(Some(&object_store::path::Path::from("experiment")));
        while let Some(meta) = list.try_next().await? {
            total = total
                .checked_add(meta.size as usize)
                .ok_or("snapshot size overflow")?;
            if total > MAX_SNAPSHOT_BYTES || objects.len() >= MAX_SNAPSHOT_OBJECTS {
                return Err("snapshot exceeds experiment limits".into());
            }
            let bytes = run
                .app
                .object_store
                .get(&meta.location)
                .await?
                .bytes()
                .await?;
            objects.insert(meta.location.to_string(), bytes.to_vec());
        }
        let mut hash = Sha256::new();
        hash.update(b"anda-brain:object-snapshot-v1\0");
        for (path, bytes) in &objects {
            hash.update((path.len() as u64).to_be_bytes());
            hash.update(path.as_bytes());
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes);
        }
        let manifest = ExperimentManifest {
            format: "anda-brain:object-snapshot-v1".into(),
            memory_mode: run.mode,
            app_name: run.app.app_name.clone(),
            app_version: run.app.app_version.clone(),
            identity: run.identity.clone(),
            prompts_digest: run.prompts_digest.clone(),
            business_time_ms: run.space.clock.now_ms(),
            state_digest: format!(
                "sha256:{}",
                hash.finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
            objects: objects.len(),
            bytes: total,
        };
        Ok(ExperimentSnapshot { manifest, objects })
    }

    /// Persistent mode clears transient agent history, retaining Notes and
    /// graph working state. SessionOnly discards the *entire* old Space.
    /// The business Agent must separately clear its own transient context.
    pub async fn session_boundary(&self) -> Result<(), BoxError> {
        let mut guard = self.lock_run().await?;
        let run = guard.as_mut().ok_or("experiment is closed")?;
        quiescent(run).await?;
        if run.mode == MemoryMode::SessionOnly {
            // A new Space must not erase the old receipt-overflow marker.
            // Retain the failed accounting state so the caller can invalidate
            // the run; resetting it would turn truncated costs into a full list.
            if run
                .space
                .db
                .get_extension_as::<bool>("experiment_costs_truncated")
                .unwrap_or(false)
            {
                return Err("cost receipts were truncated; accounting is incomplete".into());
            }
            let policy = run.space.memory_policy();
            let app = isolated_app(&run.app, run.space.clock.now_ms())?;
            let space = fresh_space(&app).await?;
            space
                .db
                .save_extension_from(MemoryPolicy::EXTENSION_KEY.into(), &policy)
                .await?;
            let previous: Vec<StageCost> = run
                .space
                .db
                .get_extension_as("experiment_costs")
                .unwrap_or_default();
            if previous.len() + run.costs.len() > 10_000 {
                return Err("cost receipt limit reached".into());
            }
            run.space.close().await?;
            run.costs.extend(previous);
            run.app = app;
            run.space = space;
        } else {
            for collection in [
                &run.space.conversations,
                &run.space.recall.conversations_collection,
                &run.space.maintenance.conversations_collection,
            ] {
                collection
                    .save_extension_from("history_boundary".into(), &collection.max_document_id())
                    .await?;
            }
            run.space.formation.clear_history();
            run.space.recall.clear_history();
            run.space.maintenance.clear_history();
            run.space.miss_cache.clear().await?;
            // Rebuild the engine as well: transient tool/context caches may
            // not survive merely because the three history rings were cleared.
            let app = run.app.fork_with_store(run.app.object_store.clone());
            run.space.close().await?;
            let space = app.load_space_with("experiment", false, false).await?;
            run.app = app;
            run.space = space;
        }
        Ok(())
    }

    pub async fn record_external_cost(&self, cost: StageCost) -> Result<(), BoxError> {
        if !matches!(
            cost.stage,
            CostStage::BusinessModel | CostStage::Tools | CostStage::Observer
        ) {
            return Err("Brain phase usage is collected by the runtime".into());
        }
        let mut guard = self.lock_run().await?;
        let run = guard.as_mut().ok_or("experiment is closed")?;
        if run.costs.len() >= 10_000 {
            return Err("cost receipt limit reached".into());
        }
        run.costs.push(cost);
        Ok(())
    }

    pub async fn costs(&self) -> Result<Vec<StageCost>, BoxError> {
        let guard = self.lock_run().await?;
        let run = guard.as_ref().ok_or("experiment is closed")?;
        if run
            .space
            .db
            .get_extension_as::<bool>("experiment_costs_truncated")
            .unwrap_or(false)
        {
            return Err("cost receipts were truncated; accounting is incomplete".into());
        }
        let mut costs: Vec<StageCost> = run
            .space
            .db
            .get_extension_as("experiment_costs")
            .unwrap_or_default();
        costs.extend(run.costs.clone());
        Ok(costs)
    }

    pub async fn cost_summary(&self) -> Result<CostSummary, BoxError> {
        let receipts = self.costs().await?;
        let unreported_stages = [
            CostStage::Formation,
            CostStage::Maintenance,
            CostStage::Recall,
            CostStage::BusinessModel,
            CostStage::Tools,
            CostStage::Observer,
        ]
        .into_iter()
        .filter(|stage| !receipts.iter().any(|receipt| receipt.stage == *stage))
        .collect::<Vec<_>>();
        let accounting_complete =
            unreported_stages.is_empty() && receipts.iter().all(|r| r.accounting_complete);
        Ok(CostSummary {
            receipts,
            unreported_stages,
            accounting_complete,
        })
    }

    /// Idempotent. Cancels and drains owned workers before closing storage.
    pub async fn close(&self) -> Result<(), BoxError> {
        self.shutdown.cancel();
        let mut guard = self.run.lock().await;
        if let Some(run) = guard.as_ref() {
            run.space.close().await?;
        }
        *guard = None;
        Ok(())
    }
}

impl Drop for Experiment {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(run) = self.run.get_mut().take() {
            run.space.engine.cancel();
            run.space.tasks.cancel();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = run.space.close().await;
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostStage {
    Formation,
    Maintenance,
    Recall,
    BusinessModel,
    Tools,
    Observer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub receipts: Vec<StageCost>,
    /// No receipt is an unmeasured stage, not a measured zero-cost stage.
    pub unreported_stages: Vec<CostStage>,
    pub accounting_complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageCost {
    pub stage: CostStage,
    pub conversation: Option<u64>,
    pub failed: bool,
    /// Mixed model/agent/tool requests in provider Usage, not model-only calls.
    pub requests: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub elapsed_ms: Option<u64>,
    /// False when a provider failure or missing telemetry may hide more cost.
    pub accounting_complete: bool,
}

#[cfg(test)]
mod tests;
