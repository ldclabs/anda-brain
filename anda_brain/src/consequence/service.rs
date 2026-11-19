use super::*;
use crate::runtime_api::{PROFILE, RuntimeError, RuntimeResult, full_read, key, principal_valid};
use anda_cognitive_nexus::{
    CognitiveNexus, content_digest,
    governance::{AuthContext, Permission, ResourceContext},
    nexus::DEFAULT_SPACE,
};
use object_store::PutMode;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ConsequenceRuntime {
    pub(super) nexus: Arc<CognitiveNexus>,
    pub(super) directory: Arc<crate::attention::Directory>,
    pub(super) scope: RuntimeScope,
    observers: Vec<ObserverContract>,
    inbox_validation: bool,
    gate: Mutex<()>,
    tasks: crate::runtime::DurableTasks,
    #[cfg(feature = "learning")]
    learning: Option<std::sync::Weak<crate::learning::LearningRuntime>>,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    allocated: u64,
    published: u64,
}
#[derive(Serialize, Deserialize)]
struct Pointer {
    receipt: ObservationReceipt,
    storage_key: String,
    attempt_ref: String,
}

impl ConsequenceRuntime {
    /// Authenticated outcomes may announce a separately authenticated native factual
    /// verification. The outcome status is never a source-reliability score.
    pub(crate) async fn trust_page(
        &self,
        after: u64,
        limit: usize,
        observer: &str,
    ) -> RuntimeResult<(Vec<String>, u64)> {
        if !(1..=8).contains(&limit) {
            return Err(RuntimeError::Invalid("trust discovery page limit".into()));
        }
        let root = key(&self.scope, "observation-index", "root")?;
        let end = self
            .directory
            .read::<Index>(&root)
            .await?
            .map_or(0, |r| r.value.published)
            .min(after.saturating_add(limit as u64));
        let mut references = vec![];
        for n in after + 1..=end {
            if let Some(pointer) = self
                .directory
                .read::<Pointer>(&format!("{root}/{n:020}"))
                .await?
            {
                let receipt = pointer.value.receipt;
                if receipt.scope == self.scope
                    && receipt.observer == observer
                    && receipt.native_committed
                    && let Some(id) = receipt.outcome_ref
                {
                    let row = full_read(&self.nexus.system_session(), &id).await?;
                    let input: OutcomeInput =
                        serde_json::from_str(row["payload"]["inline"].as_str().ok_or_else(
                            || RuntimeError::Invalid("observation payload missing".into()),
                        )?)
                        .map_err(|e| RuntimeError::Storage(e.into()))?;
                    if let Observation::Measurement { payload, .. } = input.observation
                        && let Some(reference) = payload["trust_verification_ref"].as_str()
                    {
                        references.push(reference.into());
                    }
                }
            }
        }
        Ok((references, end))
    }
    pub(crate) async fn utility_page(
        &self,
        after: u64,
        limit: usize,
        config: &UtilityConfig,
    ) -> RuntimeResult<(Vec<String>, u64)> {
        if !(1..=8).contains(&limit) {
            return Err(RuntimeError::Invalid("utility page limit".into()));
        }
        if config.method == AttributionMethod::PairedRevisionV1 {
            #[cfg(feature = "learning")]
            {
                let learning = self
                    .learning
                    .as_ref()
                    .and_then(|l| l.upgrade())
                    .ok_or_else(|| {
                        RuntimeError::Unavailable("paired utility controller missing".into())
                    })?;
                let page = learning.jobs_page(after, limit).await?;
                return Ok((
                    page.items
                        .into_iter()
                        .filter_map(|j| j.evaluation_ref)
                        .collect(),
                    if page.complete { 0 } else { page.next_after },
                ));
            }
            #[cfg(not(feature = "learning"))]
            {
                return Err(RuntimeError::Unavailable(
                    "paired utility requires learning".into(),
                ));
            }
        }
        let root = key(&self.scope, "observation-index", "root")?;
        let end = self
            .directory
            .read::<Index>(&root)
            .await?
            .map_or(0, |r| r.value.published)
            .min(after.saturating_add(limit as u64));
        let mut outcomes = vec![];
        for n in after + 1..=end {
            if let Some(pointer) = self
                .directory
                .read::<Pointer>(&format!("{root}/{n:020}"))
                .await?
            {
                let receipt = pointer.value.receipt;
                if receipt.scope == self.scope
                    && receipt.observer == config.observer.principal_id
                    && receipt.native_committed
                    && let Some(id) = receipt.outcome_ref
                {
                    outcomes.push(id);
                }
            }
        }
        Ok((outcomes, end))
    }
    pub(crate) async fn correction(&self, outcome: &str) -> RuntimeResult<Option<String>> {
        #[allow(unused_mut)] // The learning namespace is appended only with that feature.
        let mut scopes = vec![self.scope.clone()];
        #[cfg(feature = "learning")]
        if let Some(rt) = self.learning.as_ref().and_then(|l| l.upgrade())
            && rt.is_configured()
        {
            scopes.push(RuntimeScope {
                space_id: self.scope.space_id.clone(),
                space_instance: rt.observation_instance().await?,
            });
        }
        for scope in scopes {
            if let Some(r) = self
                .directory
                .read::<String>(&key(&scope, "outcome-corrections", outcome)?)
                .await?
            {
                return Ok(Some(r.value));
            }
        }
        Ok(None)
    }
    /// Consume authenticated safety obligations, including late/conflicting
    /// audit receipts. Cursor advancement follows the native revocation and
    /// receipt readback; failures retain the same work for a fresh-auth retry.
    #[cfg(feature = "learning")]
    pub(crate) async fn consume_learning_safety(
        self: &Arc<Self>,
        auth: AuthContext,
        limit: usize,
    ) -> RuntimeResult<usize> {
        let this = self.clone();
        self.tasks
            .run(async move { Ok(this.consume_learning_safety_inner(auth, limit).await) })
            .await
            .map_err(RuntimeError::Storage)?
    }
    #[cfg(feature = "learning")]
    async fn consume_learning_safety_inner(
        &self,
        auth: AuthContext,
        limit: usize,
    ) -> RuntimeResult<usize> {
        let _g = self.gate.lock().await;
        self.authorize(&auth).await?;
        if !(1..=8).contains(&limit) {
            return Err(RuntimeError::Invalid(
                "safety consumer bound must be 1..=8".into(),
            ));
        }
        let learning = self
            .learning
            .as_ref()
            .and_then(|l| l.upgrade())
            .ok_or_else(|| RuntimeError::Unavailable("learning runtime unavailable".into()))?;
        let scope = RuntimeScope {
            space_id: self.scope.space_id.clone(),
            space_instance: learning.observation_instance().await?,
        };
        let cursor_key = key(&scope, "learning-safety-cursor", &auth.principal_id)?;
        let after = self
            .directory
            .read::<u64>(&cursor_key)
            .await?
            .map_or(0, |r| r.value);
        let index_key = key(&scope, "observation-index", "root")?;
        let published = self
            .directory
            .read::<Index>(&index_key)
            .await?
            .map_or(0, |r| r.value.published);
        let mut resolved = 0;
        for n in after + 1..=published.min(after.saturating_add(limit as u64)) {
            if let Some(pointer) = self
                .directory
                .read::<Pointer>(&format!("{index_key}/{n:020}"))
                .await?
            {
                let pointer = pointer.value;
                if pointer.receipt.scope == scope
                    && pointer.receipt.observer == auth.principal_id
                    && pointer.receipt.safety_pending
                {
                    let mut stored = self
                        .directory
                        .read::<StoredReceipt>(&pointer.storage_key)
                        .await?
                        .ok_or_else(|| {
                            RuntimeError::Unavailable("safety receipt unavailable".into())
                        })?
                        .value;
                    self.validate_stored(&stored, &scope, &auth.principal_id)?;
                    if stored.receipt.safety_pending {
                        full_read(&self.nexus.session(auth.clone()), &pointer.attempt_ref).await?;
                        let route = learning
                            .observation_route(&pointer.attempt_ref, &auth)
                            .await?
                            .ok_or(RuntimeError::NotFound)?;
                        let reason = stored.input.safety_signal.clone().ok_or_else(|| {
                            RuntimeError::Invalid("safety audit lacks signal".into())
                        })?;
                        let (target, covered) =
                            learning.safety_target(&route.job_id, &auth).await?;
                        let evaluation = if let Some(reference) = covered {
                            reference
                        } else {
                            let report = learning
                                .submit_safety_signal(
                                    auth.clone(),
                                    crate::learning::SafetySubmission {
                                        space_instance: scope.space_instance.clone(),
                                        job_id: target,
                                        signal_key: stored.receipt.receipt_id.clone(),
                                        observed_at: stored.receipt.observed_at.clone(),
                                        evidence_digest: stored.receipt.body_digest.clone(),
                                        reason,
                                    },
                                )
                                .await?;
                            report
                                .safety
                                .and_then(|s| s.evaluation_ref)
                                .ok_or_else(|| {
                                    RuntimeError::Unavailable(
                                        "safety native revocation unresolved".into(),
                                    )
                                })?
                        };
                        stored.receipt.safety_pending = false;
                        stored.receipt.safety_evaluation_ref = Some(evaluation);
                        self.save(&pointer.storage_key, &stored).await?;
                        self.index_at(&stored, &pointer.storage_key).await?;
                        resolved += 1;
                    }
                }
            }
            let old = self.directory.read::<u64>(&cursor_key).await?;
            self.directory
                .put(
                    &cursor_key,
                    &n,
                    old.map_or(PutMode::Create, |r| PutMode::Update(r.version)),
                )
                .await?;
        }
        Ok(resolved)
    }
    pub(crate) fn new(
        nexus: Arc<CognitiveNexus>,
        directory: Arc<crate::attention::Directory>,
        scope: RuntimeScope,
        observers: Vec<ObserverContract>,
        inbox_validation: bool,
        #[cfg(feature = "learning")] learning: Option<
            std::sync::Weak<crate::learning::LearningRuntime>,
        >,
    ) -> Arc<Self> {
        Arc::new(Self {
            nexus,
            directory,
            scope,
            observers,
            inbox_validation,
            gate: Mutex::new(()),
            tasks: Default::default(),
            #[cfg(feature = "learning")]
            learning,
        })
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.tasks.is_busy()
    }
    pub(crate) async fn shutdown(&self) {
        self.tasks.shutdown().await;
    }
    pub(crate) fn has_learning(&self) -> bool {
        #[cfg(feature = "learning")]
        {
            self.learning
                .as_ref()
                .and_then(|l| l.upgrade())
                .is_some_and(|l| l.is_configured())
        }
        #[cfg(not(feature = "learning"))]
        {
            false
        }
    }

    /// Direct Rust hosts supply genuinely authenticated observer contexts.
    /// HTTP admission additionally requires the explicit signed-subject mapping.
    pub async fn submit(
        self: &Arc<Self>,
        auth: AuthContext,
        input: OutcomeInput,
    ) -> RuntimeResult<ObservationReceipt> {
        let this = self.clone();
        self.tasks
            .run(async move { Ok(this.submit_owned(auth, input).await) })
            .await
            .map_err(RuntimeError::Storage)?
    }
    async fn authorize(&self, auth: &AuthContext) -> RuntimeResult<()> {
        if !principal_valid(&auth.principal_id)
            || auth.auth_method.is_empty()
            || auth.auth_strength == "none"
            || !auth.delegation_chain.is_empty()
        {
            return Err(RuntimeError::Unauthorized);
        }
        let session = self.nexus.session(auth.clone());
        session
            .effective_authority(DEFAULT_SPACE)
            .await?
            .authorize(Permission::RecordOutcome, &ResourceContext::default(), auth)
            .into_result()?;
        Ok(())
    }
    async fn submit_owned(
        &self,
        auth: AuthContext,
        mut input: OutcomeInput,
    ) -> RuntimeResult<ObservationReceipt> {
        let _guard = self.gate.lock().await;
        self.authorize(&auth).await?;
        if serde_json::to_vec(&input)
            .map_err(|e| RuntimeError::Storage(e.into()))?
            .len()
            > 16_384
            || input.event_key.is_empty()
            || input.event_key.len() > 256
            || input.space_instance.is_empty()
            || input.space_instance.len() > 256
            || input
                .correction_of
                .as_ref()
                .is_some_and(|k| k.is_empty() || k.len() > 256 || k == &input.event_key)
            || input
                .safety_signal
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 1024)
        {
            return Err(RuntimeError::Invalid(
                "invalid bounded observation identity/payload".into(),
            ));
        }
        input.observed_at =
            anda_cognitive_nexus::time::normalize(&input.observed_at, "observed_at")?;
        let observed = time_ms(&input.observed_at)?;
        let now = anda_engine::unix_ms();
        if observed > now {
            return Err(RuntimeError::Invalid(
                "observation cannot be future dated".into(),
            ));
        }
        let session = self.nexus.session(auth.clone());
        let attempt = full_read(&session, &input.attempt_ref).await?;
        let record = &attempt["facets"][format!("{PROFILE}AttemptRecord")];
        let decision_ref = record["decision_ref"]
            .as_str()
            .ok_or_else(|| {
                RuntimeError::Invalid("observation requires a native AttemptRecord".into())
            })?
            .to_string();
        let decision = full_read(&session, &decision_ref).await?;
        if decision["facets"][format!("{PROFILE}DecisionRecord")]["decision"] != "act" {
            return Err(RuntimeError::Invalid(
                "observation must bind an acting decision".into(),
            ));
        }
        let tx_id = attempt["_system"]["created_tx"]
            .as_str()
            .ok_or_else(|| RuntimeError::Invalid("attempt creation transaction missing".into()))?;
        let transaction = self
            .nexus
            .store
            .find_transaction(tx_id)
            .await?
            .ok_or_else(|| {
                RuntimeError::Invalid("authenticated attempt provenance unavailable".into())
            })?;
        if transaction.space != DEFAULT_SPACE
            || transaction.status != "committed"
            || !transaction.changed_ids.contains(&input.attempt_ref)
        {
            return Err(RuntimeError::Invalid("invalid attempt provenance".into()));
        }
        if transaction.origin["principal_id"].as_str().is_none() {
            return Err(RuntimeError::Invalid(
                "authenticated attempt principal missing".into(),
            ));
        }
        if transaction.origin["principal_id"] == auth.principal_id {
            return Err(RuntimeError::Forbidden);
        }
        let started = time_ms(
            record["started_at"]
                .as_str()
                .ok_or_else(|| RuntimeError::Invalid("attempt start time missing".into()))?,
        )?;
        if observed < started {
            return Err(RuntimeError::Invalid(
                "observation predates its Attempt".into(),
            ));
        }

        #[cfg(feature = "learning")]
        if let Some(learning) = self.learning.as_ref().and_then(|r| r.upgrade())
            && let Some(route) = learning
                .observation_route(&input.attempt_ref, &auth)
                .await
                .map_err(RuntimeError::Storage)?
        {
            return self
                .submit_learning(&auth, input, decision_ref, learning, route, now)
                .await;
        }
        if record["trial_ref"].is_string()
            || matches!(input.observation, Observation::Learning { .. })
        {
            return Err(RuntimeError::Unavailable(
                "registered learning controller required; no generic fallback is permitted".into(),
            ));
        }
        if input.space_instance != self.scope.space_instance {
            return Err(RuntimeError::Invalid(
                "observation Space instance mismatch".into(),
            ));
        }
        let family = record["context"]["task_family"]
            .as_str()
            .ok_or_else(|| RuntimeError::Invalid("Attempt task family missing".into()))?;
        let contract = self
            .observers
            .iter()
            .find(|c| {
                c.principal_id == auth.principal_id
                    && c.configuration_digest == input.observer_configuration_digest
                    && c.task_family == family
                    && c.metric == input.metric
                    && c.window == input.window
            })
            .ok_or(RuntimeError::Forbidden)?;
        let contract_digest = content_digest(&json!(contract))?;
        let Observation::Measurement {
            terminal,
            outcome_status,
            magnitude,
            ..
        } = &input.observation
        else {
            unreachable!()
        };
        if magnitude.is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
            || (!terminal && *outcome_status == OutcomeStatus::Success)
        {
            return Err(RuntimeError::Invalid(
                "invalid magnitude or nonterminal success claim".into(),
            ));
        }
        let attempt_id = record["attempt_id"]
            .as_str()
            .ok_or_else(|| RuntimeError::Invalid("Attempt identity missing".into()))?;
        let dispatch_ref = format!(
            "dispatch/v1/{}",
            &content_digest(&json!({"scope":self.scope,"attempt_id":attempt_id}))?[7..]
        );
        // Queue discovery belongs to the host. Observers do not receive
        // Maintain merely to inspect an outbox; their RecordOutcome and all
        // material reads were checked above, and native reconciliation checks
        // their current authority again at commit.
        let dispatch = self
            .nexus
            .system_session()
            .read_control(DEFAULT_SPACE, &dispatch_ref, None)
            .await?
            .ok_or_else(|| {
                RuntimeError::Invalid(
                    "observation requires a previously admitted native dispatch".into(),
                )
            })?;
        if dispatch.value["request"]["attempt_ref"] != input.attempt_ref
            || observed
                < time_ms(
                    dispatch.value["first_dispatch_at"]
                        .as_str()
                        .ok_or_else(|| RuntimeError::Invalid("dispatch start missing".into()))?,
                )?
        {
            return Err(RuntimeError::Invalid(
                "observation cannot precede dispatch".into(),
            ));
        }
        let digest = content_digest(&json!(input))?;
        let storage_key = self.receipt_key(&self.scope, &auth.principal_id, &input.event_key)?;
        if let Some(old) = self.directory.read::<StoredReceipt>(&storage_key).await? {
            let mut old = old.value;
            self.validate_stored(&old, &self.scope, &auth.principal_id)?;
            if old.receipt.body_digest != digest {
                return self.conflict(&old, &input, &digest).await;
            }
            if old.contract_digest != contract_digest {
                return Err(RuntimeError::Conflict(
                    "observer contract changed; receipt requires review".into(),
                ));
            }
            self.index(&old).await?;
            if old.command.is_some() && !old.receipt.native_committed {
                self.commit_native(&auth, &mut old).await?;
            }
            self.save(&storage_key, &old).await?;
            self.index(&old).await?;
            if old.receipt.native_committed {
                full_read(
                    &session,
                    old.receipt
                        .outcome_ref
                        .as_deref()
                        .ok_or(RuntimeError::NotFound)?,
                )
                .await?;
                self.reconcile(&auth, &old).await?;
            }
            self.save(&storage_key, &old).await?;
            return Ok(old.receipt);
        }
        let mut old_correction = None;
        if let Some(event) = &input.correction_of {
            let prior = self
                .directory
                .read::<StoredReceipt>(&self.receipt_key(&self.scope, &auth.principal_id, event)?)
                .await?
                .ok_or_else(|| RuntimeError::Invalid("correction receipt unavailable".into()))?
                .value;
            self.validate_stored(&prior, &self.scope, &auth.principal_id)?;
            if prior.input.attempt_ref != input.attempt_ref {
                return Err(RuntimeError::Invalid(
                    "correction belongs to another Attempt".into(),
                ));
            }
            if let Some(reference) = prior.receipt.outcome_ref {
                full_read(&session, &reference).await?;
                old_correction = Some(reference);
            }
        }
        let late = now > started.saturating_add(contract.maximum_delay_ms)
            || observed > started.saturating_add(contract.maximum_delay_ms);
        let terminal_conflict = *terminal
            && *outcome_status != OutcomeStatus::Unknown
            && dispatch.value["outcome_ref"].is_string()
            && input.correction_of.is_none();
        if !late
            && !terminal_conflict
            && self.inbox_validation
            && *outcome_status == OutcomeStatus::Success
        {
            if input.metric != "delivery" || input.window != "durable_inbox_v1" {
                return Err(RuntimeError::Invalid(
                    "compiled inbox observations measure durable delivery only".into(),
                ));
            }
            let delivery = self
                .directory
                .read::<crate::runtime_api::config::Delivery>(
                    &crate::runtime_api::config::delivery_key(&self.scope, attempt_id)?,
                )
                .await?
                .ok_or_else(|| RuntimeError::Invalid("inbox delivery has not committed".into()))?
                .value;
            let Observation::Measurement { payload, .. } = &input.observation else {
                unreachable!()
            };
            if delivery.scope != self.scope
                || delivery.attempt_ref != input.attempt_ref
                || payload["delivery_digest"] != delivery.request_digest
                || observed < delivery.committed_at_ms
            {
                return Err(RuntimeError::Invalid(
                    "inbox observation differs from the actual durable delivery".into(),
                ));
            }
        }
        let mut stored = self.prepare_receipt(
            &self.scope,
            &auth,
            &input,
            &digest,
            decision_ref,
            Some(dispatch_ref),
            contract_digest,
            now,
        )?;
        if late {
            stored.receipt.status = "late_audit".into();
            stored.receipt.reason = Some("outside the registered observation window".into());
        } else if terminal_conflict {
            stored.receipt.status = "conflict_audit".into();
            stored.receipt.reason =
                Some("a terminal observation already closes this Attempt".into());
        } else {
            stored.command = Some(super::native::command(
                &input,
                &stored.decision_ref,
                old_correction.as_deref(),
                family,
            )?);
        }
        self.save(&storage_key, &stored).await?;
        self.index(&stored).await?;
        if stored.command.is_some() {
            self.commit_native(&auth, &mut stored).await?;
            // Save the native acknowledgement before separately reconciling the
            // dispatch. A failed reconciliation is safely retried with fresh auth.
            self.save(&storage_key, &stored).await?;
            self.index(&stored).await?;
            self.reconcile(&auth, &stored).await?;
        }
        Ok(stored.receipt)
    }

    pub(super) fn receipt_key(
        &self,
        scope: &RuntimeScope,
        observer: &str,
        event: &str,
    ) -> Result<String, BoxError> {
        key(scope, "observations", &format!("{observer}\u{1f}{event}"))
    }
    #[allow(clippy::too_many_arguments)] // Keep the frozen provenance coordinates explicit.
    pub(super) fn prepare_receipt(
        &self,
        scope: &RuntimeScope,
        auth: &AuthContext,
        input: &OutcomeInput,
        digest: &str,
        decision_ref: String,
        dispatch_ref: Option<String>,
        contract_digest: String,
        now: u64,
    ) -> RuntimeResult<StoredReceipt> {
        let receipt_id=content_digest(&json!({"format":FORMAT,"scope":scope,"observer":auth.principal_id,"event":input.event_key}))?[7..].to_string();
        Ok(StoredReceipt {
            receipt: ObservationReceipt {
                format: FORMAT.into(),
                receipt_id: receipt_id.clone(),
                scope: scope.clone(),
                event_key: input.event_key.clone(),
                body_digest: digest.into(),
                observer: auth.principal_id.clone(),
                received_at_ms: now,
                observed_at: input.observed_at.clone(),
                status: "accepted".into(),
                native_committed: false,
                learning_eligible: false,
                outcome_status: match &input.observation {
                    Observation::Measurement { outcome_status, .. } => Some(outcome_status.clone()),
                    _ => None,
                },
                outcome_ref: None,
                observation_ref: None,
                reason: None,
                safety_pending: input.safety_signal.is_some(),
                safety_evaluation_ref: None,
            },
            input: input.clone(),
            decision_ref,
            dispatch_ref,
            command: None,
            native_key: format!("brain-observation-v1:{receipt_id}"),
            contract_digest,
        })
    }
    pub(super) fn validate_stored(
        &self,
        r: &StoredReceipt,
        scope: &RuntimeScope,
        observer: &str,
    ) -> RuntimeResult<()> {
        if r.receipt.format != FORMAT || &r.receipt.scope != scope || r.receipt.observer != observer
        {
            return Err(RuntimeError::Conflict(
                "observation journal identity mismatch".into(),
            ));
        }
        Ok(())
    }
    pub(super) async fn save(&self, key: &str, stored: &StoredReceipt) -> RuntimeResult<()> {
        let old = self.directory.read::<StoredReceipt>(key).await?;
        self.directory
            .put(
                key,
                stored,
                old.map_or(PutMode::Create, |v| PutMode::Update(v.version)),
            )
            .await?;
        Ok(())
    }
    pub(super) async fn index(&self, stored: &StoredReceipt) -> RuntimeResult<()> {
        if let Some(event) = &stored.input.correction_of
            && let Some(prior) = self
                .directory
                .read::<StoredReceipt>(&self.receipt_key(
                    &stored.receipt.scope,
                    &stored.receipt.observer,
                    event,
                )?)
                .await?
            && prior.value.input.attempt_ref == stored.input.attempt_ref
            && let Some(outcome) = prior.value.receipt.outcome_ref
        {
            let marker = key(&stored.receipt.scope, "outcome-corrections", &outcome)?;
            // Retain the first authenticated challenge. Later corrections
            // remain in the immutable receipt index; none silently clears it.
            if self.directory.read::<String>(&marker).await?.is_none() {
                self.directory
                    .put(&marker, &stored.receipt.receipt_id, PutMode::Create)
                    .await?;
            }
        }
        self.index_at(
            stored,
            &self.receipt_key(
                &stored.receipt.scope,
                &stored.receipt.observer,
                &stored.receipt.event_key,
            )?,
        )
        .await
    }
    async fn index_at(&self, stored: &StoredReceipt, storage_key: &str) -> RuntimeResult<()> {
        let marker = key(
            &stored.receipt.scope,
            "observation-published",
            &content_digest(&json!(stored.receipt))?,
        )?;
        if self.directory.read::<u64>(&marker).await?.is_some() {
            return Ok(());
        }
        let index_key = key(&stored.receipt.scope, "observation-index", "root")?;
        let old = self.directory.read::<Index>(&index_key).await?;
        let mut index = old.as_ref().map(|v| v.value.allocated).unwrap_or(0);
        if index >= 1_000_000 {
            return Err(RuntimeError::Unavailable(
                "observation journal capacity exhausted".into(),
            ));
        }
        index += 1;
        self.directory
            .put(
                &index_key,
                &Index {
                    allocated: index,
                    published: old.as_ref().map_or(0, |v| v.value.published),
                },
                old.map_or(PutMode::Create, |v| PutMode::Update(v.version)),
            )
            .await?;
        let pointer = Pointer {
            receipt: stored.receipt.clone(),
            storage_key: storage_key.into(),
            attempt_ref: stored.input.attempt_ref.clone(),
        };
        self.directory
            .put(
                &format!("{index_key}/{index:020}"),
                &pointer,
                PutMode::Create,
            )
            .await?;
        let head = self
            .directory
            .read::<Index>(&index_key)
            .await?
            .ok_or(RuntimeError::NotFound)?;
        self.directory
            .put(
                &index_key,
                &Index {
                    allocated: head.value.allocated,
                    published: index,
                },
                PutMode::Update(head.version),
            )
            .await?;
        self.directory.put(&marker, &index, PutMode::Create).await?;
        Ok(())
    }
    pub(super) async fn conflict(
        &self,
        old: &StoredReceipt,
        input: &OutcomeInput,
        digest: &str,
    ) -> RuntimeResult<ObservationReceipt> {
        let conflict_key = key(
            &old.receipt.scope,
            "observation-conflicts",
            &format!("{}:{digest}", old.receipt.receipt_id),
        )?;
        let mut row = old.clone();
        row.input = input.clone();
        row.command = None;
        row.receipt.body_digest = digest.into();
        row.receipt.status = "conflict_audit".into();
        row.receipt.native_committed = false;
        row.receipt.learning_eligible = false;
        row.receipt.outcome_ref = None;
        row.receipt.observation_ref = None;
        row.receipt.reason = Some("event key reused with different content".into());
        row.receipt.safety_pending = input.safety_signal.is_some();
        row.receipt.receipt_id =
            content_digest(&json!({"original":old.receipt.receipt_id,"conflict":digest}))?[7..]
                .into();
        row.receipt.observed_at = input.observed_at.clone();
        row.receipt.received_at_ms = anda_engine::unix_ms();
        if let Some(saved) = self.directory.read::<StoredReceipt>(&conflict_key).await? {
            row = saved.value;
        } else {
            self.directory
                .put(&conflict_key, &row, PutMode::Create)
                .await?;
        }
        self.index_at(&row, &conflict_key).await?;
        Err(RuntimeError::Conflict(format!(
            "event key conflict; audit retained as {}",
            row.receipt.receipt_id
        )))
    }

    /// Bounded audit discovery, including late/conflicting safety signals. Each
    /// immutable index event has a receipt id for consumer deduplication. A
    /// snapshot is not a command to reapply an Outcome or a learning verdict.
    pub async fn receipts(
        &self,
        auth: AuthContext,
        lane: ObservationLane,
        after: u64,
        limit: usize,
    ) -> RuntimeResult<ReceiptPage> {
        self.authorize(&auth).await?;
        if !(1..=100).contains(&limit) {
            return Err(RuntimeError::Invalid(
                "receipt limit must be 1..=100".into(),
            ));
        }
        let scope = match lane {
            ObservationLane::Action => self.scope.clone(),
            #[cfg(feature = "learning")]
            ObservationLane::Learning => {
                let runtime = self
                    .learning
                    .as_ref()
                    .and_then(|r| r.upgrade())
                    .ok_or_else(|| {
                        RuntimeError::Unavailable("learning is not configured".into())
                    })?;
                RuntimeScope {
                    space_id: self.scope.space_id.clone(),
                    space_instance: runtime
                        .observation_instance()
                        .await
                        .map_err(RuntimeError::Storage)?,
                }
            }
        };
        let index_key = key(&scope, "observation-index", "root")?;
        let published = self
            .directory
            .read::<Index>(&index_key)
            .await?
            .map_or(0, |v| v.value.published);
        if after > published {
            return Err(RuntimeError::Invalid(
                "receipt cursor is outside this journal".into(),
            ));
        }
        let through = published.min(after.saturating_add(limit as u64));
        let session = self.nexus.session(auth.clone());
        let mut items = Vec::new();
        for n in after + 1..=through {
            let Some(pointer) = self
                .directory
                .read::<Pointer>(&format!("{index_key}/{n:020}"))
                .await?
            else {
                continue;
            };
            let pointer = pointer.value;
            if pointer.receipt.scope != scope || pointer.receipt.observer != auth.principal_id {
                continue;
            }
            match full_read(&session, &pointer.attempt_ref).await {
                Ok(_) => {}
                Err(RuntimeError::Forbidden | RuntimeError::NotFound) => continue,
                Err(e) => return Err(e),
            }
            items.push(pointer.receipt);
        }
        Ok(ReceiptPage {
            items,
            next_after: through,
            complete: through == published,
        })
    }
}

pub(super) fn time_ms(t: &str) -> RuntimeResult<u64> {
    crate::kip::unix_time_ms(anda_cognitive_nexus::time::parse(t)?)
        .map_err(|_| RuntimeError::Invalid("invalid observation time".into()))
}
