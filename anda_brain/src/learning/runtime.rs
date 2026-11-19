//! Persistent, host-owned trial orchestration. The journal stores dispatch
//! state, never a parallel success ledger. Nexus owns all learning records.
use super::{
    ExecutorIdentity, LearningConfig, PairedTrialPlan,
    journal::{Journal, Versioned},
    native::*,
};
use anda_cognitive_nexus::{
    CognitiveNexus, content_digest, governance::AuthContext, nexus::DEFAULT_SPACE,
};
use anda_core::{BoxError, BoxPinFut};
use anda_kip::{Json, Request, TopLevelStatus};
use futures::FutureExt;
use object_store::ObjectStore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Mutex;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

const FORMAT: &str = "anda-brain:learning-runtime-v1";

mod applicability;
mod catalog;
mod scheduler;
mod utility;
pub use catalog::{ArchiveStamp, JobPage, LearningCapacity, LearningStoragePolicy};
mod observation;
pub use applicability::{
    ApplicationContext, ProcedureStatus, ReviewPage, ReviewReason, ReviewSchedule, ReviewStatus,
};
pub(crate) use observation::{LateOutcome, ObservationRoute};
mod settlement;
use settlement::{PendingSafety, PendingVerdict};
pub use settlement::{SafetyReport, SafetySubmission};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Registration {
    format: String,
    instance: String,
    enabled: bool,
    config: LearningConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Installing,
    Baseline,
    OpeningTrial,
    Treatment,
    ReadyForEvaluation,
    Expired,
    Settled,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    Prepared,
    Authorizing,
    Dispatched,
    AwaitingOutcome,
    Reconcile,
    Observed,
}

/// Trusted executor input. Keep cohort labels, seeds and comparison metadata
/// outside model context. The executor forwards only public task/tool inputs
/// and, in treatment, the frozen revision's actual behavior.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchTicket {
    pub space_instance: String,
    pub job_id: String,
    pub pair_id: String,
    pub arm: NativeArm,
    pub dispatch_id: String,
    pub decision_ref: String,
    pub attempt_ref: String,
    pub task_ref: String,
    pub fencing_token: u64,
    pub started_at: String,
    pub plan: PairedTrialPlan,
    pub revision: Option<Json>,
    pub deadline_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<super::EnrollmentOrigin>,
}

/// Compact durable locator. The plan and revision live once in Job rather
/// than being duplicated for every pair/arm in the persistent document.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredTicket {
    space_instance: String,
    job_id: String,
    pair_id: String,
    arm: NativeArm,
    dispatch_id: String,
    decision_ref: String,
    attempt_ref: String,
    task_ref: String,
    started_at: String,
    deadline_ms: u64,
}
impl StoredTicket {
    fn hydrate(
        &self,
        job: &Job,
        authorization: Option<&NativeDispatchAuthorization>,
        revision: Option<Json>,
    ) -> DispatchTicket {
        DispatchTicket {
            space_instance: self.space_instance.clone(),
            job_id: self.job_id.clone(),
            pair_id: self.pair_id.clone(),
            arm: self.arm.clone(),
            dispatch_id: self.dispatch_id.clone(),
            decision_ref: self.decision_ref.clone(),
            attempt_ref: self.attempt_ref.clone(),
            task_ref: self.task_ref.clone(),
            fencing_token: authorization.map_or(0, |a| a.fencing_token),
            started_at: self.started_at.clone(),
            deadline_ms: self.deadline_ms,
            plan: job.plan.clone(),
            revision,
            origin: job.origin.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Attempt {
    ticket: StoredTicket,
    state: DispatchState,
    authorization: Option<NativeDispatchAuthorization>,
    outcome_ref: Option<String>,
    receipt_digest: Option<String>,
    dispatch_reconciled: bool,
    task_completed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replay_key: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    intent: NativeRequest,
    attempt_key: Option<String>,
    receipt_digest: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    format: String,
    instance: String,
    id: String,
    registration_digest: String,
    plan: PairedTrialPlan,
    basis_proposition: String,
    context_pin: NativeContextPin,
    basis: Json,
    candidate_digest: String,
    candidate_version: u64,
    stage: JobStage,
    frozen: Option<FrozenNativePlan>,
    trial_ref: Option<String>,
    attempts: BTreeMap<String, Attempt>,
    pending: Option<Pending>,
    outcome_cursor: u64,
    #[serde(default)]
    activation_ref: Option<String>,
    #[serde(default)]
    evaluation_ref: Option<String>,
    #[serde(default)]
    review: Option<ReviewSchedule>,
    #[serde(default)]
    activation_complete: bool,
    #[serde(default)]
    verdict_pending: Option<PendingVerdict>,
    #[serde(default)]
    verdict_generation: u64,
    #[serde(default)]
    safety: Option<PendingSafety>,
    #[serde(default)]
    archive: Option<ArchiveStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<super::EnrollmentOrigin>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobReport {
    pub space_instance: String,
    pub job_id: String,
    pub candidate_revision: String,
    pub cutoff: String,
    pub stage: JobStage,
    pub trial_ref: Option<String>,
    /// An ingestion cursor, not a success count or a learning score.
    pub outcome_cursor: u64,
    pub attempts: Vec<AttemptReport>,
    pub activation_ref: Option<String>,
    pub activation_complete: bool,
    pub evaluation_ref: Option<String>,
    pub review: Option<ReviewSchedule>,
    pub safety: Option<SafetyReport>,
    #[serde(default)]
    pub archive: Option<ArchiveStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<super::EnrollmentOrigin>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttemptReport {
    pub dispatch_id: String,
    pub pair_id: String,
    pub arm: NativeArm,
    pub state: DispatchState,
    pub attempt_ref: String,
    pub outcome_ref: Option<String>,
    pub task_ref: String,
    pub native_dispatch_version: Option<u64>,
    pub task_completed: bool,
    pub dispatch_reconciled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_key: Option<String>,
}
impl Job {
    fn report(&self) -> JobReport {
        JobReport {
            space_instance: self.instance.clone(),
            job_id: self.id.clone(),
            candidate_revision: self.plan.candidate_revision.clone(),
            cutoff: self.plan.execution.cutoff.clone(),
            stage: self.stage.clone(),
            trial_ref: self.trial_ref.clone(),
            outcome_cursor: self.outcome_cursor,
            activation_ref: self.activation_ref.clone(),
            activation_complete: self.activation_complete,
            evaluation_ref: self.evaluation_ref.clone(),
            review: self.review.clone(),
            safety: self.safety.as_ref().map(PendingSafety::report),
            archive: self.archive.clone(),
            origin: self.origin.clone(),
            attempts: self
                .attempts
                .values()
                .map(|a| AttemptReport {
                    dispatch_id: a.ticket.dispatch_id.clone(),
                    pair_id: a.ticket.pair_id.clone(),
                    arm: a.ticket.arm.clone(),
                    state: a.state.clone(),
                    attempt_ref: a.ticket.attempt_ref.clone(),
                    outcome_ref: a.outcome_ref.clone(),
                    task_ref: a.ticket.task_ref.clone(),
                    native_dispatch_version: a
                        .authorization
                        .as_ref()
                        .map(|v| v.native_dispatch_version),
                    task_completed: a.task_completed,
                    dispatch_reconciled: a.dispatch_reconciled,
                    replay_key: a.replay_key.clone(),
                })
                .collect(),
        }
    }
}

/// `NotStarted` is an authoritative executor lookup, not a network error or
/// timeout. All other uncertain cases keep the original dispatch identity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileResult {
    NotStarted,
    Running,
    Finished,
    Unknown,
}

/// Installed by trusted host code; never built from a model-supplied tool URL.
/// Implementations must enforce the ticket's actual snapshot/model/tool/budget
/// pins, honor cancellation, and make lookup authoritative for dispatch_id.
/// Neither dispatch acknowledgement nor lookup creates an OutcomeRecord.
pub trait LearningExecutor: Send + Sync {
    fn identity(&self) -> ExecutorIdentity;
    fn preflight(&self, _cancel: CancellationToken) -> BoxPinFut<Result<(), BoxError>> {
        Box::pin(async { Err("executor has no automatic readiness probe".into()) })
    }
    fn cancel(&self, _ticket: DispatchTicket) -> BoxPinFut<Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }
    fn dispatch(
        &self,
        ticket: DispatchTicket,
        cancel: CancellationToken,
    ) -> BoxPinFut<Result<(), BoxError>>;
    fn reconcile(
        &self,
        ticket: DispatchTicket,
        cancel: CancellationToken,
    ) -> BoxPinFut<Result<ReconcileResult, BoxError>>;
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DriveResult {
    Dispatched(DispatchTicket),
    Reconciled(DispatchTicket, ReconcileResult),
    Waiting(JobReport),
    Ready(JobReport),
    Expired(JobReport),
}

/// The independent instrument supplies measurements, not a self-reported
/// success label. Missing telemetry is unknown even when the task finished.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeMeasurements {
    pub finished: bool,
    /// The independent instrument explicitly attests complete accounting,
    /// including failures and retries. Partial SDK counters are insufficient.
    pub accounting_complete: bool,
    pub first_commit_success: Option<bool>,
    pub final_committed: Option<bool>,
    pub failed_commits: Option<u64>,
    pub unsafe_actions: Option<u64>,
    pub tool_calls: Option<u64>,
    pub elapsed_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub journal_digest: String,
}
impl OutcomeMeasurements {
    pub fn classify(&self, plan: &PairedTrialPlan) -> Result<NativeOutcomeStatus, BoxError> {
        if !super::is_digest(&self.journal_digest) {
            return Err("independent journal digest is required".into());
        }
        if !self.accounting_complete {
            return Ok(NativeOutcomeStatus::Unknown);
        }
        let (
            Some(first),
            Some(committed),
            Some(failed),
            Some(unsafe_actions),
            Some(calls),
            Some(elapsed),
            Some(input),
            Some(output),
        ) = (
            self.first_commit_success,
            self.final_committed,
            self.failed_commits,
            self.unsafe_actions,
            self.tool_calls,
            self.elapsed_ms,
            self.input_tokens,
            self.output_tokens,
        )
        else {
            return Ok(NativeOutcomeStatus::Unknown);
        };
        let b = &plan.execution.budget;
        Ok(
            if self.finished
                && first
                && committed
                && failed == 0
                && unsafe_actions == 0
                && calls > 0
                && calls <= b.tool_calls as u64
                && elapsed <= b.elapsed_ms
                && input <= b.input_tokens
                && output <= b.output_tokens
            {
                NativeOutcomeStatus::Success
            } else {
                NativeOutcomeStatus::Failure
            },
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeSubmission {
    pub space_instance: String,
    pub job_id: String,
    pub dispatch_id: String,
    pub observer_configuration_digest: String,
    pub observation_key: String,
    pub observed_at: String,
    pub measurements: OutcomeMeasurements,
}

/// One owner per Space. Public mutations spawn tracked work before awaiting it,
/// so dropped HTTP/API waiters cannot interrupt a native or journal write.
pub struct LearningRuntime {
    attention: parking_lot::RwLock<Option<std::sync::Weak<crate::attention::AttentionRuntime>>>,
    journal: Journal,
    nexus: Arc<CognitiveNexus>,
    clock: Arc<crate::runtime::BusinessClock>,
    gate: Mutex<()>,
    native: Mutex<Option<(String, NativeLearning)>>,
    configured: AtomicBool,
    closing: AtomicBool,
    cancel: CancellationToken,
    tasks: TaskTracker,
    queue: Arc<tokio::sync::Semaphore>,
    application_context: parking_lot::RwLock<Option<ApplicationContext>>,
    active_dispatch: parking_lot::RwLock<Option<(String, CancellationToken)>>,
    /// Authenticated safety submissions enter here before waiting for the
    /// serialized journal gate. Dispatch registers its cancellation token and
    /// checks this map as one handshake, closing the pre-callback race.
    safety_barriers: parking_lot::RwLock<BTreeMap<String, String>>,
    bindings: parking_lot::RwLock<Option<Arc<super::LearningBindings>>>,
    scheduler_gate: Mutex<()>,
    scheduler_running: AtomicBool,
}
impl LearningRuntime {
    pub(crate) async fn connect(
        store: Arc<dyn ObjectStore>,
        prefix: String,
        nexus: Arc<CognitiveNexus>,
        clock: Arc<crate::runtime::BusinessClock>,
    ) -> Result<Arc<Self>, BoxError> {
        let this = Arc::new(Self {
            attention: Default::default(),
            journal: Journal::new(store, prefix),
            nexus,
            clock,
            gate: Mutex::new(()),
            native: Mutex::new(None),
            configured: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            queue: Arc::new(tokio::sync::Semaphore::new(64)),
            application_context: parking_lot::RwLock::new(None),
            active_dispatch: parking_lot::RwLock::new(None),
            safety_barriers: parking_lot::RwLock::new(BTreeMap::new()),
            bindings: Default::default(),
            scheduler_gate: Mutex::new(()),
            scheduler_running: AtomicBool::new(false),
        });
        if let Some(reg) = this.journal.read::<Registration>("registration").await? {
            this.validate_registration(&reg.value)?;
            this.native_for(&reg.value.config).await?;
            this.configured.store(true, Ordering::SeqCst);
            this.recover_catalog().await?;
        }
        Ok(this)
    }
    pub fn is_configured(&self) -> bool {
        self.configured.load(Ordering::SeqCst)
    }
    pub(crate) fn bind_attention(
        &self,
        attention: std::sync::Weak<crate::attention::AttentionRuntime>,
    ) {
        *self.attention.write() = Some(attention);
    }
    pub(crate) async fn attention_reviews(
        &self,
    ) -> Result<Vec<crate::attention::Recheck>, BoxError> {
        if !self.is_configured() || !self.registration(false).await?.enabled {
            return Ok(vec![]);
        }
        let Ok(_g) = self.gate.try_lock() else {
            return Ok(vec![]);
        };
        let old = self.journal.read::<u64>("scheduler/hint-cursor").await?;
        let after = old.as_ref().map_or(0, |r| r.value);
        let (ids, next, complete) = self.archived_review_ids(after, 8).await?;
        let reg = self.registration(false).await?;
        let mut jobs = self.jobs().await?;
        for id in ids {
            if !jobs.iter().any(|r| r.job_id == id) {
                jobs.push(self.load_job(&reg, &id).await?.value.report());
            }
        }
        let mut result = vec![];
        for job in jobs {
            if let Some(review) = job.review {
                if let Some(next) = &review.next_job_id {
                    let child = self.load_job(&reg, next).await?.value;
                    if child.stage != JobStage::Expired || child.activation_complete {
                        let attention = self.attention.read().as_ref().and_then(|a| a.upgrade());
                        if let Some(attention) = attention {
                            let key = format!("skill-review:{}", job.job_id);
                            let due = time_ms(&review.due_at)?;
                            if attention.status().await?.is_some_and(|s| {
                                s.rechecks
                                    .iter()
                                    .any(|r| r.key == key && r.due_at_ms == due)
                            }) {
                                attention.acknowledge_recheck(key, due).await?;
                            }
                        }
                        continue;
                    }
                }
                result.push(crate::attention::Recheck {
                    key: format!("skill-review:{}", job.job_id),
                    source: crate::attention::RecheckSource::SkillReview,
                    due_at_ms: time_ms(&review.due_at)?,
                    notified: false,
                });
            }
        }
        let cursor = if complete { 0 } else { next };
        if let Some(mut old) = old {
            old.value = cursor;
            self.journal.save("scheduler/hint-cursor", &old).await?;
        } else {
            self.journal
                .create("scheduler/hint-cursor", &cursor)
                .await?;
        }
        Ok(result)
    }
    pub fn is_busy(&self) -> bool {
        !self.tasks.is_empty()
    }
    fn ensure_open(&self) -> Result<(), BoxError> {
        if self.closing.load(Ordering::SeqCst) {
            Err("learning runtime is closed".into())
        } else {
            Ok(())
        }
    }
    fn validate_registration(&self, reg: &Registration) -> Result<(), BoxError> {
        if reg.format != FORMAT || !super::is_digest(&reg.instance) {
            return Err("unsupported learning registration format".into());
        }
        reg.config.validate()?;
        Ok(())
    }
    async fn registration(&self, enabled: bool) -> Result<Registration, BoxError> {
        let r = self
            .journal
            .read::<Registration>("registration")
            .await?
            .ok_or("learning is not configured")?
            .value;
        self.validate_registration(&r)?;
        if enabled && !r.enabled {
            return Err("learning is disabled".into());
        }
        Ok(r)
    }
    async fn native_for(&self, cfg: &LearningConfig) -> Result<NativeLearning, BoxError> {
        let digest = content_digest(&json!(cfg))?;
        let mut native = self.native.lock().await;
        if let Some((old, n)) = &*native {
            if old != &digest {
                return Err("learning registration is immutable for this Space".into());
            }
            return Ok(n.clone());
        }
        let n = NativeLearning::new(
            self.nexus.clone(),
            DEFAULT_SPACE.into(),
            AuthContext::system(),
            cfg.observer.clone(),
        )?;
        n.restore_rule()?;
        *native = Some((digest, n.clone()));
        Ok(n)
    }
    async fn owned<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(Arc<Self>) -> BoxPinFut<Result<T, BoxError>> + Send + 'static,
    ) -> Result<T, BoxError> {
        self.ensure_open()?;
        let this = self.clone();
        let permit = self
            .queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| "learning operation queue is full")?;
        self.tasks.spawn(async move {
            let _permit=permit;
            let attention = this.attention.read().as_ref().and_then(|a| a.upgrade());
            if let Some(attention) = attention { attention.register_work().await?; }
            let result=std::panic::AssertUnwindSafe(work(this)).catch_unwind().await;
            match result {
                Ok(result)=>result,
                Err(_)=>Err("learning operation panicked; reconcile persisted native intent/dispatch before retry".into()),
            }
        }).await?
    }
    fn require_host(auth: &AuthContext) -> Result<(), BoxError> {
        if auth.principal_id != "kip:principal:system"
            || auth.auth_method != "engine"
            || auth.auth_strength == "none"
            || !auth.delegation_chain.is_empty()
        {
            return Err(
                "learning configuration requires the directly authenticated engine host".into(),
            );
        }
        Ok(())
    }
    pub async fn configure(
        self: &Arc<Self>,
        auth: AuthContext,
        config: LearningConfig,
    ) -> Result<String, BoxError> {
        Self::require_host(&auth)?;
        config.validate()?;
        self.owned(move |this| Box::pin(async move { this.configure_inner(config).await }))
            .await
    }
    async fn configure_inner(self: &Arc<Self>, config: LearningConfig) -> Result<String, BoxError> {
        let this = self;
        let _g = this.gate.lock().await;
        this.ensure_open()?;
        if let Some(old) = this.journal.read::<Registration>("registration").await? {
            if content_digest(&json!(old.value.config))? != content_digest(&json!(config))? {
                return Err("registration cannot be replaced while records exist".into());
            }
            this.configured.store(true, Ordering::SeqCst);
            this.recover_catalog().await?;
            return Ok(old.value.instance);
        }
        this.native_for(&config).await?;
        let instance = content_digest(
            &json!({"nonce":rand::random::<[u8;32]>(),"registered_at":anda_cognitive_nexus::time::now()}),
        )?;
        this.journal
            .create(
                "registration",
                &Registration {
                    format: FORMAT.into(),
                    instance: instance.clone(),
                    enabled: true,
                    config,
                },
            )
            .await?;
        this.configured.store(true, Ordering::SeqCst);
        this.recover_catalog().await?;
        Ok(instance)
    }
    pub async fn set_enabled(
        self: &Arc<Self>,
        auth: AuthContext,
        enabled: bool,
    ) -> Result<(), BoxError> {
        Self::require_host(&auth)?;
        self.owned(move |this| {
            Box::pin(async move {
                let _g = this.gate.lock().await;
                this.ensure_open()?;
                let mut reg = this
                    .journal
                    .read::<Registration>("registration")
                    .await?
                    .ok_or("learning not configured")?;
                this.validate_registration(&reg.value)?;
                reg.value.enabled = enabled;
                this.journal.save("registration", &reg).await
            })
        })
        .await
    }
    fn key(id: &str) -> Result<String, BoxError> {
        if id.is_empty() || id.len() > 128 {
            return Err("bounded job ID required".into());
        }
        Ok(format!(
            "jobs/{}",
            content_digest(&json!(id))?.trim_start_matches("sha256:")
        ))
    }
    fn native_key(instance: &str, job: &str, step: &str) -> Result<String, BoxError> {
        Ok(format!(
            "brain-learning:{}",
            content_digest(&json!([instance, job, step]))?
        ))
    }
    async fn load_job(&self, reg: &Registration, id: &str) -> Result<Versioned<Job>, BoxError> {
        let state = self
            .journal
            .read::<Job>(&Self::key(id)?)
            .await?
            .ok_or("learning job not found")?;
        let j = &state.value;
        if j.format != FORMAT
            || j.instance != reg.instance
            || j.id != id
            || j.registration_digest != content_digest(&json!(reg.config))?
        {
            return Err("learning job belongs to a different Space/registration".into());
        }
        reg.config.validate_plan(&j.plan)?;
        self.validate_archive(j).await?;
        Ok(state)
    }
    async fn read_one(&self, command: String) -> Result<Json, BoxError> {
        let mut req = Request::single(command);
        self.clock.bind_read(&mut req)?;
        let result =
            anda_kip::execute_request(&self.nexus.session(AuthContext::system()), &req).await;
        if result.status != TopLevelStatus::Succeeded {
            return Err(format!("learning authenticated read failed: {result:?}").into());
        }
        result
            .first_result()
            .and_then(Json::as_array)
            .filter(|r| r.len() == 1)
            .map(|r| r[0].clone())
            .ok_or_else(|| "learning requires one exact authenticated record".into())
    }
    pub async fn enroll(
        self: &Arc<Self>,
        job_id: String,
        plan: PairedTrialPlan,
        basis_proposition: String,
    ) -> Result<JobReport, BoxError> {
        if plan.execution.review_of.is_some() {
            return Err("use enroll_review to bind the persistent acquisition schedule".into());
        }
        self.owned(move |this| {
            Box::pin(async move {
                let _g = this.gate.lock().await;
                this.enroll_inner(job_id, plan, basis_proposition).await
            })
        })
        .await
    }
    async fn enroll_inner(
        self: &Arc<Self>,
        job_id: String,
        plan: PairedTrialPlan,
        basis_proposition: String,
    ) -> Result<JobReport, BoxError> {
        self.ensure_open()?;
        let reg = self.registration(true).await?;
        self.recover_catalog().await?;
        let mut prepared = self
            .prepare_enrollment(&reg, &job_id, plan, basis_proposition)
            .await?;
        self.bind_enrollment_origin(&reg, &job_id, &mut prepared, None)
            .await?;
        self.commit_enrollment(&reg, &job_id, prepared).await
    }

    // Read-only preparation runs before a review source points at its child.
    // None is an exact existing enrollment; it never authorizes reusing an ID
    // with different frozen inputs, including after that job has expired.
    async fn prepare_enrollment(
        &self,
        reg: &Registration,
        job_id: &str,
        plan: PairedTrialPlan,
        basis_proposition: String,
    ) -> Result<Option<Job>, BoxError> {
        let this = self;
        reg.config.validate_plan(&plan)?;
        let key = Self::key(job_id)?;
        if let Some(old) = this.journal.read::<Job>(&key).await? {
            if old.value.plan != plan || old.value.basis_proposition != basis_proposition {
                return Err("job ID conflicts with frozen plan".into());
            }
            this.load_job(reg, job_id).await?;
            return Ok(None);
        }
        if let Some(old) = this
            .journal
            .read::<Job>(&format!("enrollments/{}", &key[5..]))
            .await?
        {
            if old.value.plan != plan || old.value.basis_proposition != basis_proposition {
                return Err("job ID conflicts with reserved frozen plan".into());
            }
            return Ok(Some(old.value));
        }
        this.check_capacity(reg.config.maximum_jobs).await?;
        for other in this.jobs().await? {
            if other.stage != JobStage::Settled
                && !(other.stage == JobStage::Expired && !other.activation_complete)
            {
                return Err(
                    "one active trial per Space is supported; settle the existing job first".into(),
                );
            }
        }
        if this.clock.now_ms() >= time_ms(&plan.execution.cutoff)?
            || anda_engine::unix_ms() >= time_ms(&plan.execution.cutoff)?
        {
            return Err("cannot enroll after cutoff".into());
        }
        if !basis_proposition
            .strip_prefix("P-")
            .is_some_and(|s| s.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == s))
        {
            return Err("canonical basis Proposition reference required".into());
        }
        let projection = this
                    .read_one(format!(
                        "FIND(?p,?b,?r) WHERE {{?p PROPOSITION (id:{}) ?b BELIEF (?p) ?r CONCEPT {{id:{},type:\"SkillRevision\"}}}} LIMIT 1",
                        serde_json::to_string(&basis_proposition)?,serde_json::to_string(&plan.candidate_revision)?
                    ))
                    .await?;
        let projection = projection
            .as_array()
            .filter(|v| v.len() == 3)
            .ok_or("exact context, belief and revision snapshot required")?;
        let context_pin = NativeContextPin {
            id: basis_proposition.clone(),
            version: projection[0]["_system"]["version"]
                .as_u64()
                .ok_or("context version absent")?,
        };
        let basis = projection[1]
            .get("basis")
            .filter(|v| v.is_object())
            .ok_or("BELIEF basis absent")?
            .clone();
        let candidate = projection[2].clone();
        if candidate["attributes"]["task_family"] != plan.task_family {
            return Err("candidate task family differs from registration".into());
        }
        let policy_id = format!(
            "brain-learning-{}",
            &content_digest(&json!([reg.instance, job_id]))?[7..31]
        );
        let intent = NativeRequest {
            space_id: DEFAULT_SPACE.into(),
            idempotency_key: Self::native_key(&reg.instance, job_id, "install")?,
            operation: NativeOperation::InstallPlan(InstallPlanInput {
                plan: plan.clone(),
                policy_id,
                policy_version: "1".into(),
                expected_policy_version: 0,
                allowed_parameters: vec![],
            }),
        };
        let job = Job {
            format: FORMAT.into(),
            instance: reg.instance.clone(),
            id: job_id.to_string(),
            registration_digest: content_digest(&json!(reg.config))?,
            plan,
            basis_proposition,
            context_pin,
            basis,
            candidate_digest: content_digest(&candidate["attributes"])?,
            candidate_version: candidate["_system"]["version"]
                .as_u64()
                .ok_or("candidate version missing")?,
            stage: JobStage::Installing,
            frozen: None,
            trial_ref: None,
            attempts: BTreeMap::new(),
            pending: Some(Pending {
                intent,
                attempt_key: None,
                receipt_digest: None,
            }),
            outcome_cursor: 0,
            activation_ref: None,
            evaluation_ref: None,
            review: None,
            activation_complete: false,
            verdict_pending: None,
            verdict_generation: 0,
            safety: None,
            archive: None,
            origin: None,
        };
        Ok(Some(job))
    }

    async fn commit_enrollment(
        &self,
        reg: &Registration,
        job_id: &str,
        prepared: Option<Job>,
    ) -> Result<JobReport, BoxError> {
        if let Some(job) = prepared {
            self.create_job(&job).await?;
            self.resume_native(reg, job_id, None).await?;
        }
        Ok(self.load_job(reg, job_id).await?.value.report())
    }
    async fn resume_native(
        &self,
        reg: &Registration,
        id: &str,
        observer: Option<&AuthContext>,
    ) -> Result<(), BoxError> {
        let state = self.load_job(reg, id).await?;
        let Some(p) = state.value.pending.clone() else {
            return Ok(());
        };
        if matches!(p.intent.operation, NativeOperation::Outcome(_)) && observer.is_none() {
            return Err("pending outcome needs freshly authenticated observer to resume".into());
        }
        let native = self.native_for(&reg.config).await?;
        self.nexus.recover().await?;
        let receipt = native.execute(&p.intent, observer).await?;
        self.accept_native_receipt(reg, id, p, receipt).await
    }

    async fn accept_native_receipt(
        &self,
        reg: &Registration,
        id: &str,
        p: Pending,
        receipt: NativeReceipt,
    ) -> Result<(), BoxError> {
        let mut state = self.load_job(reg, id).await?;
        if state
            .value
            .pending
            .as_ref()
            .is_none_or(|v| content_digest(&json!(v)).ok() != content_digest(&json!(p)).ok())
        {
            return Err("pending intent changed during native commit".into());
        }
        match p.intent.operation {
            NativeOperation::InstallPlan(_) => {
                state.value.frozen = Some(receipt.frozen_plan.ok_or("native policy pin missing")?);
                state.value.stage = JobStage::Baseline;
            }
            NativeOperation::Attempt(_) => {
                let a = state
                    .value
                    .attempts
                    .get_mut(
                        p.attempt_key
                            .as_ref()
                            .ok_or("pending attempt missing key")?,
                    )
                    .ok_or("attempt missing")?;
                a.ticket.decision_ref = receipt
                    .handles
                    .get("decision")
                    .ok_or("decision handle missing")?
                    .clone();
                a.ticket.task_ref = receipt
                    .handles
                    .get("task")
                    .ok_or("dispatch task handle missing")?
                    .clone();
                a.ticket.attempt_ref = receipt
                    .handles
                    .get("attempt")
                    .ok_or("attempt handle missing")?
                    .clone();
            }
            NativeOperation::OpenTrial(_) => {
                state.value.trial_ref = Some(
                    receipt
                        .handles
                        .get("trial")
                        .ok_or("trial handle missing")?
                        .clone(),
                );
                state.value.stage = JobStage::Treatment;
            }
            NativeOperation::Outcome(_) => {
                let a = state
                    .value
                    .attempts
                    .get_mut(
                        p.attempt_key
                            .as_ref()
                            .ok_or("outcome attempt key missing")?,
                    )
                    .ok_or("outcome attempt missing")?;
                a.outcome_ref = Some(
                    receipt
                        .handles
                        .get("outcome")
                        .ok_or("outcome handle missing")?
                        .clone(),
                );
                a.receipt_digest = p.receipt_digest;
                a.state = DispatchState::Observed;
                state.value.outcome_cursor += 1;
            }
            NativeOperation::Decision(_) => {
                return Err("standalone Decision is not a coordinator intent".into());
            }
        }
        state.value.pending = None;
        self.journal.save(&Self::key(id)?, &state).await?;
        self.reconcile_outboxes(reg, id).await
    }
    async fn reconcile_outboxes(&self, reg: &Registration, id: &str) -> Result<(), BoxError> {
        let initial = self.load_job(reg, id).await?;
        let work = initial
            .value
            .attempts
            .iter()
            .filter(|(_, a)| {
                a.state == DispatchState::Observed && (!a.dispatch_reconciled || !a.task_completed)
            })
            .map(|(key, a)| (key.clone(), a.clone()))
            .collect::<Vec<_>>();
        for (key, attempt) in work {
            let authorization = attempt
                .authorization
                .as_ref()
                .ok_or("observed attempt has no native dispatch authorization")?;
            let result = self
                .native_for(&reg.config)
                .await?
                .reconcile_dispatch(
                    &attempt.ticket.dispatch_id,
                    authorization.native_dispatch_version,
                    attempt
                        .outcome_ref
                        .as_deref()
                        .ok_or("observed attempt has no Outcome")?,
                )
                .await?;
            let mut current = self.load_job(reg, id).await?;
            let saved = current
                .value
                .attempts
                .get_mut(&key)
                .ok_or("dispatch disappeared")?;
            saved.dispatch_reconciled = true;
            saved.task_completed = result.task_completed;
            saved
                .authorization
                .as_mut()
                .unwrap()
                .native_dispatch_version = result.native_dispatch_version;
            self.journal.save(&Self::key(id)?, &current).await?;
        }
        Ok(())
    }
    /// Read the bounded hot working set. Use jobs_page for retained history;
    /// report(job_id) resolves both hot and archived identities.
    pub async fn jobs(&self) -> Result<Vec<JobReport>, BoxError> {
        self.ensure_open()?;
        let reg = self.registration(false).await?;
        let mut jobs = Vec::new();
        for key in self.hot_keys().await? {
            let row = self
                .journal
                .read::<Job>(&key)
                .await?
                .ok_or("learning job disappeared")?;
            jobs.push(self.load_job(&reg, &row.value.id).await?.value.report());
        }
        jobs.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        Ok(jobs)
    }
    pub async fn report(&self, job_id: &str) -> Result<JobReport, BoxError> {
        self.ensure_open()?;
        let reg = self.registration(false).await?;
        Ok(self.load_job(&reg, job_id).await?.value.report())
    }
    async fn hydrate_ticket(
        &self,
        job: &Job,
        attempt: &Attempt,
    ) -> Result<DispatchTicket, BoxError> {
        let revision = if matches!(attempt.ticket.arm, NativeArm::Baseline)
            || !matches!(
                attempt.state,
                DispatchState::Prepared | DispatchState::Authorizing
            ) {
            None
        } else {
            let revision = self
                .read_one(format!(
                    "FIND(?r) WHERE {{?r CONCEPT {{id:{},type:\"SkillRevision\"}}}} LIMIT 1",
                    serde_json::to_string(&job.plan.candidate_revision)?
                ))
                .await?;
            if content_digest(&revision["attributes"])? != job.candidate_digest {
                return Err("frozen revision content changed".into());
            }
            Some(revision)
        };
        Ok(attempt
            .ticket
            .hydrate(job, attempt.authorization.as_ref(), revision))
    }
    pub async fn drive(
        self: &Arc<Self>,
        job_id: String,
        executor: Arc<dyn LearningExecutor>,
    ) -> Result<DriveResult, BoxError> {
        self.owned(move |this| Box::pin(async move { this.drive_inner(job_id, executor).await }))
            .await
    }
    async fn drive_inner(
        self: &Arc<Self>,
        job_id: String,
        executor: Arc<dyn LearningExecutor>,
    ) -> Result<DriveResult, BoxError> {
        let this = self;
        let _g = this.gate.lock().await;
        this.ensure_open()?;
        let reg = this.registration(true).await?;
        loop {
            this.reconcile_outboxes(&reg, &job_id).await?;
            let mut state = this.load_job(&reg, &job_id).await?;
            reg.config
                .validate_executor(&state.value.plan, &executor.identity())?;
            if matches!(
                state.value.pending.as_ref().map(|p| &p.intent.operation),
                Some(NativeOperation::Outcome(_))
            ) {
                return Ok(DriveResult::Waiting(state.value.report()));
            }
            if state.value.pending.is_some() {
                this.resume_native(&reg, &job_id, None).await?;
                continue;
            }
            if state.value.stage == JobStage::Settled {
                return Ok(DriveResult::Ready(state.value.report()));
            }
            if this
                .pending_safety(&state.value.plan.candidate_revision)
                .await?
            {
                return Err("independent safety signal requires revocation recovery before further dispatch".into());
            }
            for report in this.jobs().await? {
                let other = this.load_job(&reg, &report.job_id).await?.value;
                if other.plan.candidate_revision == state.value.plan.candidate_revision
                    && other
                        .safety
                        .as_ref()
                        .is_some_and(|s| s.evaluation_ref.is_none())
                {
                    return Err("independent safety signal requires revocation recovery before further dispatch".into());
                }
            }
            if state.value.verdict_pending.is_some() {
                this.resume_verdict(&reg, &job_id).await?;
                continue;
            }
            if state.value.stage == JobStage::ReadyForEvaluation {
                return Ok(DriveResult::Ready(state.value.report()));
            }
            if this.clock.now_ms() >= time_ms(&state.value.plan.execution.cutoff)?
                || state.value.stage == JobStage::Expired
            {
                state.value.stage = JobStage::Expired;
                this.journal.save(&Self::key(&job_id)?, &state).await?;
                return Ok(DriveResult::Expired(state.value.report()));
            }
            if state.value.trial_ref.is_some() && !state.value.activation_complete {
                this.activate_trial(&reg, &job_id).await?;
                continue;
            }
            if let Some((key, a)) = state
                .value
                .attempts
                .iter()
                .find(|(_, a)| a.state != DispatchState::Observed)
                .map(|(k, a)| (k.clone(), a.clone()))
            {
                let mut ticket = this.hydrate_ticket(&state.value, &a).await?;
                if matches!(
                    a.state,
                    DispatchState::Prepared | DispatchState::Authorizing
                ) && this.clock.now_ms() >= a.ticket.deadline_ms
                {
                    state.value.stage = JobStage::Expired;
                    this.journal.save(&Self::key(&job_id)?, &state).await?;
                    return Ok(DriveResult::Expired(state.value.report()));
                }
                if a.state == DispatchState::AwaitingOutcome {
                    return Ok(DriveResult::Waiting(state.value.report()));
                }
                if a.state == DispatchState::Dispatched || a.state == DispatchState::Reconcile {
                    let lookup_cancel = this.cancel.child_token();
                    let result = tokio::select! {_ = this.cancel.cancelled()=>Ok(ReconcileResult::Unknown),r=tokio::time::timeout(std::time::Duration::from_millis(reg.config.reconcile_timeout_ms),executor.reconcile(ticket.clone(),lookup_cancel.clone()))=>r.unwrap_or_else(|_|Ok(ReconcileResult::Unknown))};
                    lookup_cancel.cancel();
                    let result = result.unwrap_or(ReconcileResult::Unknown);
                    state.value.attempts.get_mut(&key).unwrap().state = match result {
                        ReconcileResult::NotStarted => DispatchState::Reconcile,
                        ReconcileResult::Running | ReconcileResult::Finished => {
                            DispatchState::AwaitingOutcome
                        }
                        ReconcileResult::Unknown => DispatchState::Reconcile,
                    };
                    this.journal.save(&Self::key(&job_id)?, &state).await?;
                    return Ok(DriveResult::Reconciled(ticket, result));
                }
                if a.ticket.attempt_ref.is_empty() || a.ticket.decision_ref.is_empty() {
                    return Err("dispatch cannot precede native attempt commit".into());
                }
                this.native_for(&reg.config)
                    .await?
                    .validate_frozen(
                        &state.value.plan,
                        state.value.frozen.as_ref().ok_or("policy pin missing")?,
                    )
                    .await?;
                if matches!(a.ticket.arm, NativeArm::Treatment { .. }) {
                    this.native_for(&reg.config)
                        .await?
                        .validate_active_trial(&Self::verdict_input(
                            &reg,
                            &state.value,
                            "dispatch-read",
                        )?)
                        .await?;
                }
                // The complete native authorization input is derivable from
                // this persisted locator and the immutable per-job plan.
                state.value.attempts.get_mut(&key).unwrap().state = DispatchState::Authorizing;
                this.journal.save(&Self::key(&job_id)?, &state).await?;
                let input = NativeDispatchInput {
                    plan: state.value.plan.clone(),
                    frozen: state.value.frozen.clone().ok_or("policy pin missing")?,
                    pair_id: a.ticket.pair_id.clone(),
                    arm: a.ticket.arm.clone(),
                    attempt_id: a.ticket.dispatch_id.clone(),
                    attempt_ref: a.ticket.attempt_ref.clone(),
                    task_ref: a.ticket.task_ref.clone(),
                    lease_expires_at: crate::kip::timestamp(
                        time_ms(&a.ticket.started_at)?
                            .saturating_add(state.value.plan.execution.budget.elapsed_ms),
                    ),
                };
                let authorization = this
                    .native_for(&reg.config)
                    .await?
                    .authorize_dispatch(&input)
                    .await?;
                let dispatch = matches!(authorization.action, NativeDispatchAction::Dispatch);
                ticket.fencing_token = authorization.fencing_token;
                let attempt = state.value.attempts.get_mut(&key).unwrap();
                attempt.state = if dispatch {
                    DispatchState::Dispatched
                } else {
                    DispatchState::Reconcile
                };
                attempt.authorization = Some(authorization);
                // If this checkpoint fails, no external callback runs. A later
                // native begin can return Lookup, never a second first permit.
                let mut current = this.load_job(&reg, &job_id).await?;
                current.value = state.value;
                this.journal.save(&Self::key(&job_id)?, &current).await?;
                if !dispatch {
                    return Ok(DriveResult::Waiting(current.value.report()));
                }
                let auth = current.value.attempts[&key]
                    .authorization
                    .as_ref()
                    .ok_or("dispatch authorization missing")?;
                this.native_for(&reg.config)
                    .await?
                    .revalidate_dispatch(&input, auth)
                    .await?;
                if matches!(ticket.arm, NativeArm::Treatment { .. }) {
                    this.native_for(&reg.config)
                        .await?
                        .validate_active_trial(&Self::verdict_input(
                            &reg,
                            &current.value,
                            "dispatch-read",
                        )?)
                        .await?;
                }
                // Native preparation/checkpoints already consumed part of the
                // real lease. A paused experiment clock must never give the
                // callback that time back or extend its authenticated fence.
                let dispatch_timeout_ms = ticket
                    .plan
                    .execution
                    .budget
                    .elapsed_ms
                    .min(ticket.deadline_ms.saturating_sub(this.clock.now_ms()))
                    .min(time_ms(&auth.lease_expires_at)?.saturating_sub(anda_engine::unix_ms()));
                if this.cancel.is_cancelled() || dispatch_timeout_ms == 0 {
                    let mut parked = this.load_job(&reg, &job_id).await?;
                    parked.value.attempts.get_mut(&key).unwrap().state = DispatchState::Reconcile;
                    this.journal.save(&Self::key(&job_id)?, &parked).await?;
                    return Err("dispatch permit expired/cancelled before external callback; reconcile required".into());
                }
                let dispatch_cancel = this.cancel.child_token();
                *this.active_dispatch.write() = Some((
                    ticket.plan.candidate_revision.clone(),
                    dispatch_cancel.clone(),
                ));
                let safety_blocked = this
                    .safety_barriers
                    .read()
                    .values()
                    .any(|revision| revision == &ticket.plan.candidate_revision);
                if safety_blocked {
                    dispatch_cancel.cancel();
                    *this.active_dispatch.write() = None;
                    let mut parked = this.load_job(&reg, &job_id).await?;
                    parked.value.attempts.get_mut(&key).unwrap().state = DispatchState::Reconcile;
                    this.journal.save(&Self::key(&job_id)?, &parked).await?;
                    return Err(
                        "dispatch blocked by an authenticated safety signal; reconcile required"
                            .into(),
                    );
                }
                let result = tokio::select! {_ = dispatch_cancel.cancelled()=>Err("dispatch cancelled; reconcile required".into()),r=tokio::time::timeout(std::time::Duration::from_millis(dispatch_timeout_ms),executor.dispatch(ticket.clone(),dispatch_cancel.clone()))=>r.unwrap_or_else(|_|Err("dispatch deadline; reconcile required".into()))};
                *this.active_dispatch.write() = None;
                if result.is_err() {
                    dispatch_cancel.cancel();
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_millis(reg.config.reconcile_timeout_ms),
                        executor.cancel(ticket.clone()),
                    )
                    .await;
                }
                let mut state = this.load_job(&reg, &job_id).await?;
                state.value.attempts.get_mut(&key).unwrap().state = if result.is_ok() {
                    DispatchState::AwaitingOutcome
                } else {
                    DispatchState::Reconcile
                };
                this.journal.save(&Self::key(&job_id)?, &state).await?;
                return match result {
                    Ok(()) => Ok(DriveResult::Dispatched(ticket)),
                    Err(e) => Err(e),
                };
            }
            if state.value.stage == JobStage::Baseline
                && state.value.attempts.len() == state.value.plan.pairs.len()
            {
                let native = this.native_for(&reg.config).await?;
                let input = native
                    .prepare_trial(
                        state.value.plan.clone(),
                        state.value.frozen.clone().ok_or("plan not installed")?,
                        state.value.basis.clone(),
                        state
                            .value
                            .attempts
                            .values()
                            .map(|a| a.ticket.attempt_ref.clone())
                            .collect(),
                        state
                            .value
                            .attempts
                            .values()
                            .map(|a| a.outcome_ref.clone().ok_or("baseline outcome missing"))
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                    .await?;
                state.value.pending = Some(Pending {
                    intent: NativeRequest {
                        space_id: DEFAULT_SPACE.into(),
                        idempotency_key: Self::native_key(&reg.instance, &job_id, "trial")?,
                        operation: NativeOperation::OpenTrial(input),
                    },
                    attempt_key: None,
                    receipt_digest: None,
                });
                state.value.stage = JobStage::OpeningTrial;
                this.journal.save(&Self::key(&job_id)?, &state).await?;
                continue;
            }
            if state.value.stage == JobStage::Treatment
                && state.value.attempts.len() == 2 * state.value.plan.pairs.len()
            {
                state.value.stage = JobStage::ReadyForEvaluation;
                this.journal.save(&Self::key(&job_id)?, &state).await?;
                return Ok(DriveResult::Ready(state.value.report()));
            }
            this.native_for(&reg.config)
                .await?
                .validate_frozen(
                    &state.value.plan,
                    state.value.frozen.as_ref().ok_or("policy pin missing")?,
                )
                .await?;
            let arm = if state.value.stage == JobStage::Baseline {
                NativeArm::Baseline
            } else {
                NativeArm::Treatment {
                    trial_ref: state
                        .value
                        .trial_ref
                        .clone()
                        .ok_or("treatment needs committed trial")?,
                }
            };
            let prefix = if matches!(arm, NativeArm::Baseline) {
                "baseline"
            } else {
                "treatment"
            };
            let pair = state
                .value
                .plan
                .pairs
                .keys()
                .find(|p| !state.value.attempts.contains_key(&format!("{prefix}:{p}")))
                .ok_or("no eligible enrolled pair")?
                .clone();
            let attempt_key = format!("{prefix}:{pair}");
            let dispatch_id = Self::native_key(&reg.instance, &job_id, &attempt_key)?;
            let ticket = StoredTicket {
                space_instance: reg.instance.clone(),
                job_id: job_id.clone(),
                pair_id: pair.clone(),
                arm: arm.clone(),
                dispatch_id: dispatch_id.clone(),
                decision_ref: String::new(),
                attempt_ref: String::new(),
                task_ref: String::new(),
                started_at: anda_cognitive_nexus::time::now(),
                deadline_ms: this
                    .clock
                    .now_ms()
                    .saturating_add(state.value.plan.execution.budget.elapsed_ms)
                    .min(time_ms(&state.value.plan.execution.cutoff)?),
            };
            state.value.pending = Some(Pending {
                intent: NativeRequest {
                    space_id: DEFAULT_SPACE.into(),
                    idempotency_key: dispatch_id.clone(),
                    operation: NativeOperation::Attempt(AttemptInput {
                        decision: DecisionInput {
                            plan: state.value.plan.clone(),
                            pair_id: pair,
                            arm,
                            basis: state.value.basis.clone(),
                            context_pin: state.value.context_pin.clone(),
                            origin: state.value.origin.clone(),
                            applied_revision_version: if matches!(ticket.arm, NativeArm::Baseline) {
                                None
                            } else {
                                Some(state.value.candidate_version)
                            },
                        },
                        attempt_id: dispatch_id,
                        started_at: ticket.started_at.clone(),
                    }),
                },
                attempt_key: Some(attempt_key.clone()),
                receipt_digest: None,
            });
            state.value.attempts.insert(
                attempt_key,
                Attempt {
                    ticket,
                    state: DispatchState::Prepared,
                    authorization: None,
                    outcome_ref: None,
                    receipt_digest: None,
                    dispatch_reconciled: false,
                    task_completed: false,
                    replay_key: None,
                },
            );
            this.journal.save(&Self::key(&job_id)?, &state).await?;
        }
    }
    pub async fn submit_outcome(
        self: &Arc<Self>,
        auth: AuthContext,
        submission: OutcomeSubmission,
    ) -> Result<JobReport, BoxError> {
        self.owned(move |this| {
            Box::pin(async move { this.submit_outcome_inner(auth, submission).await })
        })
        .await
    }
    async fn submit_outcome_inner(
        self: &Arc<Self>,
        auth: AuthContext,
        submission: OutcomeSubmission,
    ) -> Result<JobReport, BoxError> {
        let this = self;
        let _g = this.gate.lock().await;
        this.ensure_open()?;
        let reg = this.registration(false).await?;
        if auth.principal_id != reg.config.observer.principal_id
            || auth.auth_method.is_empty()
            || auth.auth_strength == "none"
            || !auth.delegation_chain.is_empty()
        {
            return Err("direct authenticated observer is required".into());
        }
        if submission.space_instance != reg.instance
            || submission.observer_configuration_digest != reg.config.observer.configuration_digest
        {
            return Err("outcome belongs to a different Space or observer configuration".into());
        }
        let mut state = this.load_job(&reg, &submission.job_id).await?;
        let digest = content_digest(&json!(submission))?;
        let native = this.native_for(&reg.config).await?;
        let (key, a) = state
            .value
            .attempts
            .iter()
            .find(|(_, a)| a.ticket.dispatch_id == submission.dispatch_id)
            .map(|(k, a)| (k.clone(), a.clone()))
            .ok_or("outcome has no precommitted dispatch")?;
        if let Some(pending) = state.value.pending.clone() {
            if pending.receipt_digest.as_ref() != Some(&digest) {
                return Err("another native intent must be reconciled first".into());
            }
            if let Some(receipt) = native
                .recover_committed(&pending.intent, Some(&auth))
                .await?
            {
                this.accept_native_receipt(&reg, &submission.job_id, pending, receipt)
                    .await?;
                return Ok(this
                    .load_job(&reg, &submission.job_id)
                    .await?
                    .value
                    .report());
            }
        }
        native
            .validate_observer(
                &auth,
                state.value.frozen.as_ref().ok_or("policy pin missing")?,
            )
            .await?;
        if let Some(old) = &a.receipt_digest {
            if old == &digest {
                this.reconcile_outboxes(&reg, &submission.job_id).await?;
                return Ok(this
                    .load_job(&reg, &submission.job_id)
                    .await?
                    .value
                    .report());
            }
            return Err("conflicting terminal observation for dispatch".into());
        }
        if matches!(
            a.state,
            DispatchState::Prepared | DispatchState::Authorizing
        ) || a.ticket.attempt_ref.is_empty()
        {
            return Err("outcome cannot precede dispatch".into());
        }
        if submission.observation_key.is_empty() || submission.observation_key.len() > 256 {
            return Err("bounded observation key required".into());
        }
        let observed = time_ms(&submission.observed_at)?;
        if observed > anda_engine::unix_ms() {
            return Err(
                "observation time cannot be in the future; business time is separate".into(),
            );
        }
        if observed < time_ms(&a.ticket.started_at)? {
            return Err("outcome predates the committed attempt".into());
        }
        if this.clock.now_ms() > time_ms(&state.value.plan.execution.cutoff)?
            || observed > time_ms(&state.value.plan.execution.cutoff)?
            || state.value.stage == JobStage::Expired
        {
            return Err(LateOutcome.into());
        }
        if !submission.measurements.finished {
            return Err("nonterminal progress cannot close a learning attempt".into());
        }
        let outcome_status = submission.measurements.classify(&state.value.plan)?;
        let frozen = state.value.frozen.clone().ok_or("policy pin missing")?;
        let intent = NativeRequest {
            space_id: DEFAULT_SPACE.into(),
            idempotency_key: Self::native_key(
                &reg.instance,
                &submission.job_id,
                &format!("outcome:{}", a.ticket.dispatch_id),
            )?,
            operation: NativeOperation::Outcome(OutcomeInput {
                plan: state.value.plan.clone(),
                frozen,
                pair_id: a.ticket.pair_id,
                arm: a.ticket.arm,
                decision_ref: a.ticket.decision_ref,
                attempt_ref: a.ticket.attempt_ref,
                observation_key: format!("terminal:{}", submission.dispatch_id),
                observed_at: submission.observed_at.clone(),
                outcome_status,
                payload: json!({"dispatch_id":submission.dispatch_id,"space_instance":submission.space_instance,"observation_key":submission.observation_key,"measurements":submission.measurements}),
            }),
        };
        if let Some(p) = &state.value.pending {
            if p.receipt_digest.as_ref() != Some(&digest) {
                return Err("another native intent must be reconciled first".into());
            }
        } else {
            state.value.pending = Some(Pending {
                intent,
                attempt_key: Some(key),
                receipt_digest: Some(digest),
            });
            this.journal
                .save(&Self::key(&submission.job_id)?, &state)
                .await?;
        }
        this.resume_native(&reg, &submission.job_id, Some(&auth))
            .await?;
        Ok(this
            .load_job(&reg, &submission.job_id)
            .await?
            .value
            .report())
    }
    pub(crate) async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}
pub(super) fn time_ms(value: &str) -> Result<u64, BoxError> {
    let utc = anda_kip::timestamp::parse(value, "learning timestamp")?;
    Ok(crate::kip::unix_time_ms(utc)?)
}

#[cfg(test)]
mod tests;
