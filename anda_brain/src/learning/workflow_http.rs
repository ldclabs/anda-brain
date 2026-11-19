//! Registered business-host transport for tool_workflow.precondition.v1.
//! The host implements real reset/inspect/prepare/commit and records actual
//! state transitions. A separate credential reads the instrument's journal.
//! This adapter supplies no business model, test labels or calibration scores.
use super::{runtime::time_ms, *};
use anda_cognitive_nexus::{content_digest, governance::AuthContext};
use anda_core::{BoxError, BoxPinFut, Json};
use anda_engine::model::reqwest::{self, Url};
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const FORMAT: &str = "anda-brain:workflow-http-v1";
const MAX_RESPONSE: usize = 262_144;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHttpConfig {
    pub id: String,
    pub registration: LearningConfig,
    pub identity: ExecutorIdentity,
    pub automation: LearningAutomation,
    #[serde(default)]
    pub storage: LearningStoragePolicy,
    pub executor_endpoint: String,
    pub observer_endpoint: String,
    pub source_endpoint: String,
    pub executor_token_env: String,
    pub observer_token_env: String,
    pub source_token_env: String,
    pub source_id: String,
    pub source_digest: String,
    pub calibration: Json,
    pub callback_timeout_ms: u64,
}

#[derive(Clone)]
struct Channel {
    client: reqwest::Client,
    base: Url,
    token: String,
}
impl Channel {
    fn new(endpoint: &str, token: String, timeout: u64) -> Result<Self, BoxError> {
        let base = Url::parse(endpoint)?;
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.path().ends_with('/')
            || !(base.scheme() == "https"
                || (base.scheme() == "http"
                    && matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
        {
            return Err(
                "workflow endpoints need a credential-free HTTPS base URL (loopback HTTP allowed)"
                    .into(),
            );
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_millis(timeout))
                .build()?,
            base,
            token,
        })
    }
    async fn call<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Option<Json>,
        cancel: CancellationToken,
    ) -> Result<T, BoxError> {
        let request = if let Some(body) = body {
            if serde_json::to_vec(&body)?.len() > MAX_RESPONSE {
                return Err("workflow request exceeds bound".into());
            }
            self.client.post(self.base.join(path)?).json(&body)
        } else {
            self.client.get(self.base.join(path)?)
        };
        let request = request.bearer_auth(&self.token);
        let work = async {
            // Transport errors are deliberately redacted; reqwest errors can
            // include URLs. Credentials never enter persisted plans/receipts.
            let mut response = request
                .send()
                .await
                .map_err(|_| "workflow host transport unavailable")?;
            if !response.status().is_success() {
                return Err("workflow host rejected request".into());
            }
            if response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE as u64)
            {
                return Err("workflow response exceeds bound".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "workflow host response interrupted")?
            {
                if bytes.len() + chunk.len() > MAX_RESPONSE {
                    return Err("workflow response exceeds bound".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice(&bytes).map_err(|_| "invalid workflow host response".into())
        };
        tokio::select! { _ = cancel.cancelled() => Err("workflow request cancelled".into()), r = work => r }
    }
}

/// The host must enforce these properties in its own persistent state machine.
/// A successful handshake binds the contract; it is not empirical calibration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCapabilities {
    pub format: String,
    pub identity: ExecutorIdentity,
    pub reset_isolated: bool,
    pub request_idempotency: bool,
    pub authoritative_status: bool,
    pub fence_and_deadline_enforced: bool,
    pub cancellation: bool,
    pub complete_instrumented_journal: bool,
    pub calibration_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRequirement {
    Required,
    Unnecessary,
    Forbidden,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkflowState {
    pub preparation_requirement: WorkflowRequirement,
    pub prepared: bool,
    pub committed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAction {
    Inspect,
    Prepare,
    Commit,
}
impl WorkflowAction {
    fn path(&self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Prepare => "prepare",
            Self::Commit => "commit",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEvent {
    pub sequence: u64,
    pub action: WorkflowAction,
    pub before: WorkflowState,
    pub after: WorkflowState,
    pub succeeded: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowJournal {
    pub format: String,
    pub dispatch_id: String,
    pub attempt_ref: String,
    pub fencing_token: u64,
    pub identity: ExecutorIdentity,
    pub case: PairCase,
    pub initial_state: WorkflowState,
    pub events: Vec<WorkflowEvent>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub finished: bool,
    pub accounting_complete: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReset {
    pub request_digest: String,
    pub identity: ExecutorIdentity,
    pub initial_state_digest: String,
    pub task: Json,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowChoice {
    pub action: Option<WorkflowAction>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReply {
    pub public: Json,
    pub terminal: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStatus {
    pub request_digest: String,
    pub state: ReconcileResult,
}

pub struct WorkflowHttpExecutor {
    channel: Channel,
    config: WorkflowHttpConfig,
}
struct WorkflowHttpObserver {
    channel: Channel,
    config: WorkflowHttpConfig,
}
struct WorkflowHttpFactory {
    channel: Channel,
    source_id: String,
    source_digest: String,
}

impl WorkflowHttpConfig {
    pub fn resolve(
        &self,
        mut secret: impl FnMut(&str) -> Option<String>,
    ) -> Result<LearningBindings, BoxError> {
        if self.id != "workflow_http_v1" {
            return Err("unknown learning adapter; compiled adapter: workflow_http_v1".into());
        }
        let mut secrets = Vec::new();
        for name in [
            &self.executor_token_env,
            &self.observer_token_env,
            &self.source_token_env,
        ] {
            if name.is_empty()
                || name.len() > 128
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            {
                return Err("invalid workflow credential environment reference".into());
            }
            secrets.push(
                secret(name)
                    .filter(|v| !v.is_empty())
                    .ok_or("workflow credential unavailable")?,
            );
        }
        if secrets[0] == secrets[1] || secrets[1] == secrets[2] || secrets[0] == secrets[2] {
            return Err(
                "workflow executor, observer and source require distinct credentials".into(),
            );
        }
        let executor = Arc::new(WorkflowHttpExecutor {
            channel: Channel::new(
                &self.executor_endpoint,
                secrets[0].clone(),
                self.callback_timeout_ms,
            )?,
            config: self.clone(),
        });
        let observer = Arc::new(WorkflowHttpObserver {
            channel: Channel::new(
                &self.observer_endpoint,
                secrets[1].clone(),
                self.callback_timeout_ms,
            )?,
            config: self.clone(),
        });
        let plans = Arc::new(WorkflowHttpFactory {
            channel: Channel::new(
                &self.source_endpoint,
                secrets[2].clone(),
                self.callback_timeout_ms,
            )?,
            source_id: self.source_id.clone(),
            source_digest: self.source_digest.clone(),
        });
        let bindings = LearningBindings {
            registration: self.registration.clone(),
            automation: self.automation.clone(),
            storage: self.storage.clone(),
            executor,
            observer,
            plans,
            calibration: self.calibration.clone(),
            callback_timeout_ms: self.callback_timeout_ms,
        };
        bindings.validate()?;
        Ok(bindings)
    }
}

fn dispatch_header(ticket: &DispatchTicket) -> Result<Json, BoxError> {
    let case = ticket
        .plan
        .pairs
        .get(&ticket.pair_id)
        .ok_or("workflow pair is not frozen")?;
    let budget_deadline =
        time_ms(&ticket.started_at)?.saturating_add(ticket.plan.execution.budget.elapsed_ms);
    Ok(
        json!({"format":FORMAT,"dispatch_id":ticket.dispatch_id,"attempt_ref":ticket.attempt_ref,
        "task_ref":ticket.task_ref,"fencing_token":ticket.fencing_token,
        "deadline_ms": ticket.deadline_ms.min(budget_deadline),
        "case":case,"environment_digest":ticket.plan.environment_digest,"model_digest":ticket.plan.model_digest,
        "base_memory_digest":ticket.plan.base_memory_digest,"tool_versions":ticket.plan.tool_versions,"budget":ticket.plan.execution.budget}),
    )
}
impl LearningExecutor for WorkflowHttpExecutor {
    fn identity(&self) -> ExecutorIdentity {
        self.config.identity.clone()
    }
    fn preflight(&self, cancel: CancellationToken) -> BoxPinFut<Result<(), BoxError>> {
        let channel = self.channel.clone();
        let config = self.config.clone();
        Box::pin(async move {
            let caps: WorkflowCapabilities = channel.call("capabilities", None, cancel).await?;
            if caps.format != FORMAT
                || caps.identity != config.identity
                || caps.calibration_digest != config.registration.calibration_digest
                || !caps.reset_isolated
                || !caps.request_idempotency
                || !caps.authoritative_status
                || !caps.fence_and_deadline_enforced
                || !caps.cancellation
                || !caps.complete_instrumented_journal
            {
                return Err("workflow host lacks pinned reset, journal, cancellation or execution capabilities".into());
            }
            Ok(())
        })
    }
    fn dispatch(
        &self,
        ticket: DispatchTicket,
        cancel: CancellationToken,
    ) -> BoxPinFut<Result<(), BoxError>> {
        let channel = self.channel.clone();
        let config = self.config.clone();
        Box::pin(async move {
            config
                .registration
                .validate_executor(&ticket.plan, &config.identity)?;
            let header = dispatch_header(&ticket)?;
            let request_digest = content_digest(&header)?;
            if ticket.fencing_token == 0
                || header["deadline_ms"].as_u64().unwrap_or(0) <= anda_engine::unix_ms()
            {
                return Err("workflow dispatch has no live native fence".into());
            }
            let reset: WorkflowReset = channel
                .call("reset", Some(header.clone()), cancel.clone())
                .await?;
            if reset.request_digest != request_digest
                || reset.identity != config.identity
                || reset.initial_state_digest
                    != ticket.plan.pairs[&ticket.pair_id].initial_state_digest
                || content_digest(&reset.task)? != ticket.plan.pairs[&ticket.pair_id].task_digest
            {
                return Err(
                    "workflow reset did not reproduce the frozen task/state/environment".into(),
                );
            }
            let mut feedback = Vec::<Json>::new();
            let mut input_tokens = 0_u64;
            let mut output_tokens = 0_u64;
            let budget = &ticket.plan.execution.budget;
            for step in 0..budget.tool_calls {
                if cancel.is_cancelled()
                    || anda_engine::unix_ms() >= header["deadline_ms"].as_u64().unwrap_or(0)
                {
                    return Err("workflow deadline/cancellation reached".into());
                }
                // The model endpoint sees the public task and actual replies.
                // No cohort label, seed, requirement, score or native graph is
                // sent; the host selects pinned factual memory by digest.
                let choice: WorkflowChoice = channel.call("decide", Some(json!({"dispatch_id":ticket.dispatch_id,"step":step,
                    "task":reset.task,"procedure":ticket.revision.as_ref().map(|r| &r["attributes"]["procedure"]),
                    "feedback":feedback,"model_digest":ticket.plan.model_digest,"base_memory_digest":ticket.plan.base_memory_digest,
                    "remaining_input_tokens":budget.input_tokens.saturating_sub(input_tokens),"remaining_output_tokens":budget.output_tokens.saturating_sub(output_tokens)})), cancel.clone()).await?;
                input_tokens = input_tokens
                    .checked_add(choice.input_tokens)
                    .ok_or("workflow usage overflow")?;
                output_tokens = output_tokens
                    .checked_add(choice.output_tokens)
                    .ok_or("workflow usage overflow")?;
                if input_tokens > budget.input_tokens || output_tokens > budget.output_tokens {
                    return Err("workflow model budget exceeded".into());
                }
                let Some(action) = choice.action else {
                    break;
                };
                let reply: WorkflowReply = channel
                    .call(
                        action.path(),
                        Some(json!({"request":header,"request_digest":request_digest,
                    "sequence":step,"idempotency_key":format!("{}:{step}",ticket.dispatch_id)})),
                        cancel.clone(),
                    )
                    .await?;
                feedback.push(json!({"tool":action,"reply":reply.public}));
                if reply.terminal {
                    break;
                }
            }
            let _: Json = channel
                .call(
                    "finish",
                    Some(json!({"request":header,"request_digest":request_digest})),
                    cancel,
                )
                .await?;
            Ok(())
        })
    }
    fn cancel(&self, ticket: DispatchTicket) -> BoxPinFut<Result<(), BoxError>> {
        let channel = self.channel.clone();
        Box::pin(async move {
            let _: Json = channel
                .call(
                    "cancel",
                    Some(dispatch_header(&ticket)?),
                    CancellationToken::new(),
                )
                .await?;
            Ok(())
        })
    }
    fn reconcile(
        &self,
        ticket: DispatchTicket,
        cancel: CancellationToken,
    ) -> BoxPinFut<Result<ReconcileResult, BoxError>> {
        let channel = self.channel.clone();
        Box::pin(async move {
            let header = dispatch_header(&ticket)?;
            let status: WorkflowStatus =
                channel.call("status", Some(header.clone()), cancel).await?;
            if status.request_digest != content_digest(&header)? {
                return Err("workflow status belongs to another dispatch".into());
            }
            Ok(status.state)
        })
    }
}

#[async_trait]
impl LearningObserver for WorkflowHttpObserver {
    fn control(&self) -> anda_kip::cognitive::ObserverControl {
        self.config.registration.observer.clone()
    }
    async fn authenticate(&self, cancel: CancellationToken) -> Result<AuthContext, BoxError> {
        let control: anda_kip::cognitive::ObserverControl =
            self.channel.call("identity", None, cancel).await?;
        if json!(control) != json!(self.control()) {
            return Err("workflow instrument identity changed".into());
        }
        let mut auth = AuthContext::principal(&control.principal_id);
        auth.auth_method = "brain:workflow-http-authenticated-instrument".into();
        Ok(auth)
    }
    async fn observe(
        &self,
        ticket: &DispatchTicket,
        cancel: CancellationToken,
    ) -> Result<Option<LearningObservation>, BoxError> {
        let journal: Option<WorkflowJournal> = self
            .channel
            .call("journal", Some(dispatch_header(ticket)?), cancel)
            .await?;
        let Some(journal) = journal else {
            return Ok(None);
        };
        if !journal.finished {
            return Ok(None);
        }
        let measurements = verify_journal(ticket, &self.config.identity, &journal)?;
        Ok(Some(LearningObservation {
            input: crate::consequence::OutcomeInput {
                utility: None,
                space_instance: ticket.space_instance.clone(),
                attempt_ref: ticket.attempt_ref.clone(),
                observer_configuration_digest: self
                    .config
                    .registration
                    .observer
                    .configuration_digest
                    .clone(),
                event_key: format!("workflow:{}:terminal", ticket.dispatch_id),
                observed_at: crate::kip::timestamp(
                    journal
                        .finished_at_ms
                        .ok_or("terminal journal has no timestamp")?,
                ),
                metric: "success".into(),
                window: ticket.plan.observation_window.clone(),
                safety_signal: measurements
                    .unsafe_actions
                    .filter(|n| *n > 0)
                    .map(|_| "independent workflow journal recorded forbidden preparation".into()),
                observation: crate::consequence::Observation::Learning {
                    measurements: json!(measurements),
                },
                correction_of: None,
            },
            replay: json!(journal),
        }))
    }
}

/// Recompute measurements from ordered observed state transitions. Executor
/// prose and final-state-only success claims are never accepted as evidence.
pub fn verify_journal(
    ticket: &DispatchTicket,
    identity: &ExecutorIdentity,
    journal: &WorkflowJournal,
) -> Result<OutcomeMeasurements, BoxError> {
    if journal.format != FORMAT
        || journal.dispatch_id != ticket.dispatch_id
        || journal.attempt_ref != ticket.attempt_ref
        || journal.fencing_token != ticket.fencing_token
        || &journal.identity != identity
        || Some(&journal.case) != ticket.plan.pairs.get(&ticket.pair_id)
        || content_digest(&json!(journal.initial_state))? != journal.case.initial_state_digest
        || journal.initial_state.prepared
        || journal.initial_state.committed
        || journal.events.len() > 64
        || journal.started_at_ms < time_ms(&ticket.started_at)?
        || journal
            .finished_at_ms
            .is_some_and(|n| n < journal.started_at_ms || n > anda_engine::unix_ms())
    {
        return Err("workflow journal does not match its actual frozen dispatch/reset".into());
    }
    let mut state = journal.initial_state.clone();
    let mut first_commit = None;
    let mut failed = 0_u64;
    let mut unsafe_actions = 0_u64;
    for (i, e) in journal.events.iter().enumerate() {
        if e.sequence != i as u64
            || e.before != state
            || e.after.preparation_requirement != state.preparation_requirement
        {
            return Err("workflow journal ordering or state continuity is invalid".into());
        }
        let mut expected = state.clone();
        match e.action {
            WorkflowAction::Inspect => {}
            WorkflowAction::Prepare => {
                if state.preparation_requirement == WorkflowRequirement::Forbidden {
                    unsafe_actions += 1;
                }
                if e.succeeded {
                    expected.prepared = true;
                }
            }
            WorkflowAction::Commit => {
                if e.succeeded
                    && state.preparation_requirement == WorkflowRequirement::Required
                    && !state.prepared
                {
                    return Err(
                        "workflow commit violates the registered precondition contract".into(),
                    );
                }
                first_commit.get_or_insert(e.succeeded);
                if e.succeeded {
                    expected.committed = true;
                } else {
                    failed += 1;
                }
            }
        }
        if e.after != expected {
            return Err("workflow log omits or invents a state mutation".into());
        }
        state = e.after.clone();
    }
    Ok(OutcomeMeasurements {
        finished: journal.finished,
        accounting_complete: journal.accounting_complete,
        first_commit_success: first_commit.or(Some(false)),
        final_committed: Some(state.committed),
        failed_commits: Some(failed),
        unsafe_actions: Some(unsafe_actions),
        tool_calls: Some(journal.events.len() as u64),
        elapsed_ms: journal.finished_at_ms.map(|n| n - journal.started_at_ms),
        input_tokens: journal.input_tokens,
        output_tokens: journal.output_tokens,
        journal_digest: content_digest(&json!(journal))?,
    })
}
#[async_trait]
impl LearningPlanFactory for WorkflowHttpFactory {
    fn source(&self) -> (String, String) {
        (self.source_id.clone(), self.source_digest.clone())
    }
    async fn next(
        &self,
        instance: &str,
        after: Option<&str>,
        review: Option<&ReviewStatus>,
        cancel: CancellationToken,
    ) -> Result<Option<LearningEnrollment>, BoxError> {
        self.channel
            .call(
                "enrollment",
                Some(
                    json!({"format":FORMAT,"space_instance":instance,"source_id":self.source_id,
            "source_digest":self.source_digest,"after":after,"review":review}),
                ),
                cancel,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn r5_example_resolves_without_enabling_trials_and_requires_reviewed_matching_material() {
        let config: crate::runtime_api::config::RuntimeConfig =
            serde_json::from_str(include_str!("../../learning.runtime.example.json")).unwrap();
        let mut adapter: WorkflowHttpConfig =
            serde_json::from_value(config.spaces["example_space"].learning.clone().unwrap())
                .unwrap();
        let resolve = |name: &str| Some(format!("test-secret-{name}"));
        let bindings = adapter.resolve(resolve).unwrap();
        assert!(!bindings.automation.trials && !bindings.automation.reviews);
        assert!(adapter.resolve(|_| None).is_err());
        adapter.automation.trials = true;
        assert!(
            adapter.resolve(resolve).is_err(),
            "a digest alone does not authorize automatic trials"
        );
        adapter.calibration["approved_for_automatic_trials"] = true.into();
        assert!(
            adapter.resolve(resolve).is_err(),
            "approval is part of the pinned material"
        );
        adapter.registration.calibration_digest = content_digest(&adapter.calibration).unwrap();
        assert!(adapter.resolve(resolve).is_ok());
        adapter.registration.minimum_effect = 0.2;
        assert!(
            adapter.resolve(resolve).is_err(),
            "changing calibrated bounds requires fresh approval"
        );
    }
}
