//! Native fenced dispatch admission. This is not external tool authorization:
//! callbacks must also enforce their actual target-system permissions.
use super::*;
use anda_kip::cognitive::DispatchRequest;

/// The host's actual read coordinate, used as context rather than truth or an
/// action prerequisite. The default workflow has no extra cognitive premises.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeContextPin {
    pub id: String,
    pub version: u64,
}
impl NativeContextPin {
    pub(super) fn validate(&self) -> Result<(), KipError> {
        if !self.id.strip_prefix("P-").is_some_and(|id| {
            id.parse::<u64>()
                .is_ok_and(|n| n > 0 && n.to_string() == id)
        }) || self.version == 0
            || self.version > 9_007_199_254_740_991
        {
            return Err(invalid(
                "learning context must be a real pinned Proposition read",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDispatchInput {
    pub plan: PairedTrialPlan,
    pub frozen: FrozenNativePlan,
    pub pair_id: String,
    pub arm: NativeArm,
    pub attempt_id: String,
    pub attempt_ref: String,
    pub task_ref: String,
    /// Absolute real wall time, frozen before acquisition. Never a simulated
    /// business-clock expiry; it cannot exceed started_at + the attempt budget.
    pub lease_expires_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeDispatchAction {
    Dispatch,
    Lookup,
    Done,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDispatchAuthorization {
    pub action: NativeDispatchAction,
    pub fencing_token: u64,
    pub task_version: u64,
    pub native_dispatch_version: u64,
    pub idempotency_key: String,
    pub lease_expires_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDispatchReconciliation {
    pub state: String,
    pub native_dispatch_version: u64,
    pub task_completed: bool,
    pub task_completion_pending: Option<String>,
}

impl NativeLearning {
    /// Acquire/recover the attempt's own task lease and pass through native
    /// enqueue_dispatch + begin_dispatch. Only action=Dispatch permits the
    /// external callback. Repeated calls never silently upgrade lookup to send.
    pub async fn authorize_dispatch(
        &self,
        input: &NativeDispatchInput,
    ) -> Result<NativeDispatchAuthorization, KipError> {
        self.validate_frozen(&input.plan, &input.frozen).await?;
        let session = self.writer(None)?;
        let attempt = self
            .dispatch_view(&session, &input.attempt_ref, "ACTIVITY")
            .await?;
        let record = &attempt["facets"][format!("{PROFILE}AttemptRecord")];
        self.validate_attempt(&input.plan, &input.pair_id, &input.arm, record)?;
        if record["attempt_id"] != input.attempt_id
            || !attempt["outputs"].as_array().is_some_and(|outputs| {
                outputs
                    .iter()
                    .any(|r| reference(r) == Some(input.task_ref.as_str()))
            })
        {
            return Err(invalid(
                "dispatch task/identity is not the committed attempt's output",
            ));
        }
        let mut task = self
            .dispatch_view(&session, &input.task_ref, "CONCEPT")
            .await?;
        if task["schema_ref"] != format!("{PROFILE}SleepTask")
            || task["attributes"]["task_class"] != "review_skill"
            || task["attributes"]["summary"]
                != format!("Learning controller dispatch: {}", input.attempt_id)
            || task["_system"]["created_tx"] != attempt["_system"]["created_tx"]
        {
            return Err(invalid(
                "dispatch requires its exact controller-created SleepTask",
            ));
        }
        let expires = anda_cognitive_nexus::time::normalize(
            &input.lease_expires_at,
            "learning lease expiry",
        )?;
        let started = anda_cognitive_nexus::time::parse(
            record["started_at"]
                .as_str()
                .ok_or_else(|| invalid("attempt start missing"))?,
        )?;
        let max_expiry = started
            .checked_add(std::time::Duration::from_millis(
                input.plan.execution.budget.elapsed_ms,
            ))
            .map_err(|_| invalid("attempt expiry is outside the KIP calendar range"))?;
        if expires != input.lease_expires_at
            || anda_cognitive_nexus::time::parse(&expires)? > max_expiry
            || expires <= anda_cognitive_nexus::time::now()
        {
            return Err(KipError::version_conflict(
                "dispatch lease is outside the fixed real-time attempt budget",
            ));
        }
        match task["attributes"]["status"].as_str() {
            Some("pending") => {
                let version = task["_system"]["version"]
                    .as_u64()
                    .ok_or_else(|| invalid("task version unavailable"))?;
                session
                    .lease_task(&self.space_id, &input.task_ref, version, &expires)
                    .await?;
                task = self
                    .dispatch_view(&session, &input.task_ref, "CONCEPT")
                    .await?;
            }
            Some("running") => {}
            _ => {
                return Err(KipError::version_conflict(
                    "learning dispatch task is no longer pending/running",
                ));
            }
        }
        let lease = &task["facets"][format!("{PROFILE}LeaseState")];
        if lease["owner"] != self.host.principal_id || lease["expires_at"] != expires {
            return Err(KipError::version_conflict(
                "task has a different owner or frozen lease expiry",
            ));
        }
        let fence = lease["fencing_token"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or_else(|| invalid("native lease fence missing"))?;
        let request = DispatchRequest {
            attempt_ref: input.attempt_ref.clone(),
            task_ref: input.task_ref.clone(),
            fencing_token: fence,
            supports_idempotency: false,
            supports_outcome_lookup: true,
        };
        let key = format!("dispatch/{}", input.attempt_id);
        let version = if let Some(old) = session.read_control(&self.space_id, &key, None).await? {
            let saved: DispatchRequest = serde_json::from_value(old.value["request"].clone())
                .map_err(|e| invalid(e.to_string()))?;
            if saved.attempt_ref != request.attempt_ref
                || saved.task_ref != request.task_ref
                || saved.supports_idempotency
                || !saved.supports_outcome_lookup
                || old.value["attempt_id"] != input.attempt_id
            {
                return Err(invalid("attempt has a different native dispatch intent"));
            }
            old.version
        } else {
            let queued = session.enqueue_dispatch(&self.space_id, request).await?;
            queued["version"]
                .as_u64()
                .ok_or_else(|| invalid("native enqueue version missing"))?
        };
        let begun = session
            .begin_dispatch(&self.space_id, &input.attempt_id, version, fence)
            .await?;
        let action =
            serde_json::from_value(begun["action"].clone()).map_err(|e| invalid(e.to_string()))?;
        if begun["idempotency_key"] != input.attempt_id {
            return Err(invalid("native dispatch identity changed"));
        }
        Ok(NativeDispatchAuthorization {
            action,
            fencing_token: fence,
            task_version: task["_system"]["version"]
                .as_u64()
                .ok_or_else(|| invalid("task version missing"))?,
            native_dispatch_version: begun["version"]
                .as_u64()
                .ok_or_else(|| invalid("native dispatch version missing"))?,
            idempotency_key: input.attempt_id.clone(),
            lease_expires_at: expires,
        })
    }

    /// Revalidate an existing first-dispatch permit after the host checkpoint,
    /// immediately before constructing the external callback. The existing
    /// enqueue path reruns native check_dispatch under Nexus's write guard and
    /// returns the same outbox value; this never invokes begin or mints a permit.
    pub async fn revalidate_dispatch(
        &self,
        input: &NativeDispatchInput,
        authorization: &NativeDispatchAuthorization,
    ) -> Result<(), KipError> {
        if authorization.action != NativeDispatchAction::Dispatch
            || authorization.idempotency_key != input.attempt_id
            || authorization.lease_expires_at != input.lease_expires_at
            || authorization.fencing_token == 0
            || authorization.lease_expires_at <= anda_cognitive_nexus::time::now()
        {
            return Err(KipError::version_conflict(
                "first-dispatch permit is absent or expired",
            ));
        }
        self.validate_frozen(&input.plan, &input.frozen).await?;
        let session = self.writer(None)?;
        let key = format!("dispatch/{}", input.attempt_id);
        let current = session
            .read_control(&self.space_id, &key, None)
            .await?
            .ok_or_else(|| {
                KipError::not_found_or_not_visible("native dispatch permit unavailable")
            })?;
        let request = DispatchRequest {
            attempt_ref: input.attempt_ref.clone(),
            task_ref: input.task_ref.clone(),
            fencing_token: authorization.fencing_token,
            supports_idempotency: false,
            supports_outcome_lookup: true,
        };
        if current.version != authorization.native_dispatch_version
            || current.value["state"] != "dispatching"
            || current.value["attempt_id"] != input.attempt_id
            || current.value["request"] != json!(request)
        {
            return Err(KipError::version_conflict(
                "native dispatch permit changed before callback",
            ));
        }
        let checked = session.enqueue_dispatch(&self.space_id, request).await?;
        if checked["version"] != authorization.native_dispatch_version
            || checked["intent"] != current.value
        {
            return Err(KipError::version_conflict(
                "native dispatch intent changed during revalidation",
            ));
        }
        Ok(())
    }

    /// Record the native outbox's link to an already independently observed
    /// terminal Outcome. This does not re-observe or reinterpret its success.
    /// A known outbox outcome can survive an expired worker lease; a task's
    /// terminal transition cannot bypass that lease or its fencing token.
    pub async fn reconcile_dispatch(
        &self,
        attempt_id: &str,
        expected_version: u64,
        outcome_ref: &str,
    ) -> Result<NativeDispatchReconciliation, KipError> {
        let session = self.writer(None)?;
        let key = format!("dispatch/{attempt_id}");
        let old = session
            .read_control(&self.space_id, &key, None)
            .await?
            .ok_or_else(|| {
                KipError::not_found_or_not_visible("native dispatch intent unavailable")
            })?;
        let observed = self
            .read_record(&session, outcome_ref, "OutcomeRecord")
            .await?;
        if observed["principal_id"] != self.observer.principal_id
            || observed["record"]["observer_config_digest"] != self.observer.configuration_digest
            || observed["record"]["attempt_ref"] != old.value["request"]["attempt_ref"]
            || observed["record"]["terminal"] != true
        {
            return Err(invalid(
                "outbox reconciliation needs its independently authenticated terminal Outcome",
            ));
        }
        if old
            .value
            .get("outcome_ref")
            .is_some_and(|v| v != outcome_ref)
        {
            return Err(invalid(
                "native dispatch already refers to a different terminal outcome",
            ));
        }
        if !(matches!(
            old.value["state"].as_str(),
            Some("completed" | "outcome_unknown")
        ) && old.value["outcome_ref"] == outcome_ref)
        {
            session
                .reconcile_dispatch(&self.space_id, attempt_id, expected_version, outcome_ref)
                .await?;
        }
        let current = session
            .read_control(&self.space_id, &key, None)
            .await?
            .ok_or_else(|| {
                KipError::outcome_unknown("native dispatch reconciliation disappeared")
            })?;
        let state = current.value["state"]
            .as_str()
            .ok_or_else(|| invalid("native dispatch state missing"))?
            .to_string();
        let mut result = NativeDispatchReconciliation {
            state: state.clone(),
            native_dispatch_version: current.version,
            task_completed: false,
            task_completion_pending: None,
        };
        if state == "outcome_unknown" {
            result.task_completion_pending = Some("external outcome remains unknown".into());
            return Ok(result);
        }
        let task_ref = current.value["request"]["task_ref"]
            .as_str()
            .ok_or_else(|| invalid("dispatch task missing"))?;
        let fence = current.value["request"]["fencing_token"]
            .as_u64()
            .ok_or_else(|| invalid("dispatch fence missing"))?;
        match self
            .complete_dispatch_task(&session, task_ref, fence, attempt_id)
            .await
        {
            Ok(()) => result.task_completed = true,
            Err(error) => result.task_completion_pending = Some(error.to_string()),
        }
        Ok(result)
    }

    async fn complete_dispatch_task(
        &self,
        session: &Session,
        task_ref: &str,
        fence: u64,
        attempt_id: &str,
    ) -> Result<(), KipError> {
        let mut task = self.dispatch_view(session, task_ref, "CONCEPT").await?;
        if task["attributes"]["status"] == "completed" {
            return Ok(());
        }
        let lease = &task["facets"][format!("{PROFILE}LeaseState")];
        if lease["owner"] != self.host.principal_id
            || lease["fencing_token"].as_u64().is_none_or(|n| n < fence)
        {
            return Err(KipError::version_conflict(
                "task completion has lost its lease fence",
            ));
        }
        // The independent terminal Outcome already closed the native outbox.
        // A fresh lease here permits only completion bookkeeping; no begin or
        // external execution is called. Unknown outcomes never reach this path.
        if task["attributes"]["status"] == "running"
            && lease["expires_at"]
                .as_str()
                .is_some_and(|t| t <= anda_cognitive_nexus::time::now().as_str())
        {
            let version = task["_system"]["version"]
                .as_u64()
                .ok_or_else(|| invalid("task version missing"))?;
            session
                .lease_task(
                    &self.space_id,
                    task_ref,
                    version,
                    &crate::kip::timestamp(anda_engine::unix_ms() + 30_000),
                )
                .await?;
            task = self.dispatch_view(session, task_ref, "CONCEPT").await?;
        }
        let lease = &task["facets"][format!("{PROFILE}LeaseState")];
        if task["attributes"]["status"] != "running"
            || lease["expires_at"]
                .as_str()
                .is_none_or(|t| t <= anda_cognitive_nexus::time::now().as_str())
        {
            return Err(KipError::version_conflict(
                "task completion requires its current unexpired lease",
            ));
        }
        let version = task["_system"]["version"]
            .as_u64()
            .ok_or_else(|| invalid("task version missing"))?;
        let mut request = self.request(format!(
            "UPDATE {} SET ATTRIBUTES {{status:\"completed\"}} EXPECT VERSION {version}",
            literal(task_ref)
        ));
        request.operations[0].idempotency_key = Some(format!(
            "learning-task-completion:{attempt_id}:{}",
            lease["fencing_token"]
        ));
        successful(&anda_kip::execute_request(session, &request).await)?;
        Ok(())
    }

    async fn dispatch_view(
        &self,
        session: &Session,
        reference: &str,
        kind: &str,
    ) -> Result<Json, KipError> {
        let response = anda_kip::execute_request(
            session,
            &self.request(format!(
                "FIND(?element) WHERE {{?element {kind} {{id:{}}}}}",
                literal(reference)
            )),
        )
        .await;
        let rows = successful(&response)?
            .as_array()
            .filter(|r| r.len() == 1)
            .ok_or_else(|| KipError::not_found_or_not_visible("dispatch record unavailable"))?;
        Ok(rows[0].clone())
    }
}

fn reference(value: &Json) -> Option<&str> {
    value.as_str().or_else(|| value["id"].as_str())
}
