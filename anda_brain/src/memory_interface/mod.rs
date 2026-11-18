//! The KIP 2.0 Memory Interface binding (`KIP-2.0-Memory-Interface.md`, wire
//! shapes in `kip-memory.schema.json`): five intents — observe, recall,
//! revise, feedback, forget — over one request shape, one intent per request.
//!
//! This Brain is the Adapter. Host code captures sources, retains idempotency
//! keys, resolves scope, issues receipts, tracks progress and reports coverage;
//! the models only do semantics (MI §7):
//!
//! - **observe / revise** become ordinary Formation conversations carrying the
//!   intent. Formation is sequential per Space, so a receipt's progress is read
//!   from its conversation and the trace the pass recorded; the first terminal
//!   progress is written back once so it never moves backwards.
//! - **feedback** is captured as attributed Evidence by the host, never a
//!   grade: no model call, no Outcome.
//! - **forget** runs an ErasurePlan over the target's owned closure and reports
//!   `completed` only after the Nexus validated every surface.
//! - **recall** waits on the `after` barrier, runs a Recall pass for the
//!   query, and assembles a Briefing whose items carry host-computed final
//!   belief, whose channels come from exact host reads, and whose basis is
//!   retained for `detail: "evidence"`.
//!
//! The binding advertises `memory_basic` only: the Nexus claims KIP-Core, so
//! `memory_experience` and `memory_learning` are refused as unadvertised.

// KIP errors carry their structured details by value, as the engine's do.
#![allow(clippy::result_large_err)]

use anda_cognitive_nexus::{ElementId, nexus::DEFAULT_SPACE, store::Element};
use anda_core::BoxError;
use anda_kip::{
    ErrorObject, KipError, KipErrorCode, SpaceSelector,
    memory::binding::{self as wire, Bundle, Descriptor, Operation, Request, Response, Status},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::sync::Arc;

use crate::space::Space;

mod forget;
mod intake;
mod recall;
mod sources;
#[cfg(test)]
mod tests;

pub(crate) use intake::{FormationTrace, INTENT_KEY, IntentState, MemoryIntent};
pub use sources::{
    HostSource, SourceKind, SourceOrder, StageSourceInput, StagedSource, StagedSourceRef,
};

/// Keep requested level names until caller admission. MI §3 assigns an
/// unknown level a capability error that echoes the original request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IncomingRequest {
    #[serde(default)]
    requires: Vec<String>,
    #[serde(flatten)]
    pub request: Request,
}

impl IncomingRequest {
    pub fn resolve(mut self) -> Result<Request, Response> {
        for level in self.requires {
            let Some(bundle) = Bundle::from_wire(&level) else {
                return Err(Response::failed(
                    &self.request,
                    &KipError::unsupported_capability(format!("unknown memory level {level}")),
                ));
            };
            self.request.requires.push(bundle);
        }
        Ok(self.request)
    }
}

/// Where this binding keeps its private orchestration state: staged sources,
/// idempotency keys, receipts, retained recall bases and erasure plans.
/// Native facts stay in the Nexus.
pub(crate) const JOURNAL_PREFIX: &str = "memory-interface/v1";

/// How many times a conditional update of one journal record is tried. A lost
/// race resolves on the next read; a write that keeps failing is a storage
/// fault, reported instead of retried forever.
pub(crate) const JOURNAL_UPDATE_ATTEMPTS: usize = 8;

/// The error after [`JOURNAL_UPDATE_ATTEMPTS`] failed conditional updates.
pub(crate) fn journal_update_failed(last: Option<BoxError>) -> KipError {
    KipError::internal_error(format!(
        "a memory journal record could not be updated after {JOURNAL_UPDATE_ATTEMPTS} attempts: {}",
        last.map(|error| error.to_string())
            .unwrap_or_else(|| "no attempt was made".into())
    ))
}

/// The output budget a request gets when it names none.
pub const DEFAULT_OUTPUT_TOKENS: u64 = 4096;
/// The deadline a recall gets when it names none.
pub const DEFAULT_DEADLINE_MS: u64 = 30_000;
/// The longest deadline a request may ask for.
pub const MAX_DEADLINE_MS: u64 = 120_000;
/// The smallest useful response this binding promises to fit.
pub const MINIMUM_RESPONSE_TOKENS: u64 = 256;

/// The Memory Interface state of one Space.
pub(crate) struct MemoryInterface {
    pub journal: crate::journal::Journal,
    /// Serializes intake, so a key is bound to one receipt.
    pub gate: tokio::sync::Mutex<()>,
    /// Owns host writes (feedback Evidence, forget, repair) so a dropped
    /// request never cancels a native commit halfway.
    pub tasks: crate::runtime::DurableTasks,
}

impl MemoryInterface {
    pub fn new(db: &anda_db::database::AndaDB) -> Arc<Self> {
        Arc::new(Self {
            journal: crate::journal::Journal::new(db.object_store(), JOURNAL_PREFIX.into()),
            gate: tokio::sync::Mutex::new(()),
            tasks: Default::default(),
        })
    }

    pub fn is_busy(&self) -> bool {
        self.tasks.is_busy()
    }
}

/// What a deployment of this Brain advertises (MI §2).
pub fn descriptor(space_id: &str) -> Descriptor {
    Descriptor {
        kip_memory: wire::KIP_MEMORY_VERSION.into(),
        bundles: vec![Bundle::MemoryBasic],
        default_scope: None,
        default_budget: wire::Budget {
            max_output_tokens: Some(DEFAULT_OUTPUT_TOKENS),
            deadline_ms: Some(DEFAULT_DEADLINE_MS),
            tokenizer: None,
        },
        tokenizer: crate::recall_budget::TOKENIZER.into(),
        minimum_response_tokens: MINIMUM_RESPONSE_TOKENS,
        default_space: Some(SpaceSelector {
            id: Some(space_id.to_string()),
            uri: None,
        }),
    }
}

/// The descriptor without a Space: what the service-level `/info` reports.
pub fn descriptor_template() -> Descriptor {
    Descriptor {
        default_space: None,
        ..descriptor("")
    }
}

/// The host capabilities the connected Nexus reports alongside its own
/// registry (Spec §67.4): the binding and nothing this Brain does not serve.
pub(crate) fn host_capabilities(space_id: &str) -> anda_cognitive_nexus::meta::HostCapabilities {
    anda_cognitive_nexus::meta::HostCapabilities {
        memory_interface: Some(descriptor(space_id)),
        durable_brain_runtime: false,
        receiver_fencing: false,
    }
}

/// A resolved scope: the handles the caller named and the canonical context
/// set they resolve to (MI §3, Spec §25.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedScope {
    /// The scope as the caller named it, canonicalized.
    pub requested: wire::Scope,
    /// The task handle's Concept, when a task was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The canonical context set: the task's Concept and every context's,
    /// sorted and deduplicated. Empty means general memory.
    #[serde(default)]
    pub contexts: Vec<String>,
}

impl ResolvedScope {
    /// Whether memory recorded under `stored` is eligible here: a stored task
    /// must be this task, and every stored context must be in this set
    /// (Spec §25.3 set inclusion; an unscoped record is eligible everywhere).
    pub fn admits(&self, stored_task: Option<&str>, stored_contexts: &[String]) -> bool {
        if let Some(task) = stored_task
            && self.task.as_deref() != Some(task)
        {
            return false;
        }
        stored_contexts
            .iter()
            .all(|context| self.contexts.iter().any(|c| c == context))
    }

    /// The MemoryScope Facet value this scope writes (Profile §20.3).
    pub fn memory_scope(&self) -> Json {
        json!({
            "task_ref": self.task,
            "context_refs": self.contexts,
        })
    }
}

/// The key a host-created scope Concept is upserted under.
fn scope_key(handle: &str) -> String {
    format!("memory_scope:{handle}")
}

/// A handle a host may name a scope with: an element id, or an opaque host
/// string of printable characters.
fn check_handle(handle: &str) -> Result<(), KipError> {
    if handle.is_empty() || handle.len() > 256 || handle.chars().any(char::is_control) {
        return Err(KipError::invalid_request_envelope(
            "a scope handle is 1..=256 printable characters",
        ));
    }
    Ok(())
}

/// Maps an internal failure onto a KIP error, keeping a KIP error's own code.
pub(crate) fn kip_error(error: BoxError) -> KipError {
    match error.downcast::<KipError>() {
        Ok(error) => *error,
        Err(error) => {
            let message = error.to_string();
            match message.as_str() {
                "source_suppressed" => KipError::not_found_or_not_visible(
                    "the source is excluded from memory by an earlier forget",
                ),
                "memory_change_pending" => KipError::new(
                    KipErrorCode::PreconditionFailed,
                    "a managed memory change is reconciling; retry after it finishes",
                ),
                _ => KipError::internal_error(message),
            }
        }
    }
}

pub(crate) fn error_object(error: &KipError) -> ErrorObject {
    ErrorObject::from(error.clone())
}

impl Space {
    /// This Space's Memory Interface descriptor.
    pub fn memory_descriptor(&self) -> Descriptor {
        descriptor(self.id())
    }

    /// Serves one Memory Interface request for `namespace`. Every failure is
    /// a `failed` Response carrying a KIP error, never a transport error.
    /// `owner` is whether the caller holds an owner credential, which a
    /// semantic forget needs.
    pub async fn memory_request(
        self: &Arc<Self>,
        namespace: &str,
        owner: bool,
        request: Request,
    ) -> Response {
        match crate::boxed(self.memory_request_inner(namespace, owner, &request)).await {
            Ok(response) => response,
            Err(error) => Response::failed(&request, &error),
        }
    }

    async fn memory_request_inner(
        self: &Arc<Self>,
        namespace: &str,
        owner: bool,
        request: &Request,
    ) -> Result<Response, KipError> {
        let intent = request.intent()?;
        if let Some(space) = &request.space
            && (space.uri.is_some() || space.id.as_deref() != Some(self.id()))
        {
            return Err(KipError::not_found_or_not_visible(
                "the requested Space is not this connection's Space",
            ));
        }
        self.memory_descriptor().check_requires(request)?;
        if let Some(budget) = &request.budget {
            check_budget(budget)?;
        }
        // Each intent is its own subsystem; see `crate::boxed`.
        match intent {
            wire::Intent::Recall(input) => {
                crate::boxed(self.memory_recall(namespace, request, input)).await
            }
            wire::Intent::Forget(input) => {
                crate::boxed(self.memory_forget(namespace, owner, request, input)).await
            }
            intent => crate::boxed(self.memory_intake(namespace, request, intent)).await,
        }
    }

    /// Reads a receipt's current progress for its owner. Another caller, or
    /// another Space, cannot tell it from a missing one (MI §3).
    pub async fn memory_progress(
        self: &Arc<Self>,
        namespace: &str,
        receipt_ref: &str,
    ) -> Result<wire::Progress, KipError> {
        let record = self.intake_record(namespace, receipt_ref).await?;
        self.progress_of(&record).await
    }

    /// The Space sequence now: the coordinate a new intake is accepted at.
    pub(crate) async fn memory_seq(&self) -> Result<u64, KipError> {
        self.memory.nexus().store.current_seq(DEFAULT_SPACE).await
    }

    /// Resolves a scope into Concepts. A handle that is an element id must be
    /// an active Concept of this Space; any other handle names a scope Concept
    /// the host keys by it. Mutations create that Concept on first use; a
    /// recall only reads it, so an unused handle scopes to nothing yet and
    /// never widens to other tasks (MI §3).
    pub(crate) async fn resolve_scope(
        &self,
        scope: Option<&wire::Scope>,
        create: bool,
    ) -> Result<ResolvedScope, KipError> {
        let requested = scope.map(wire::Scope::canonical).unwrap_or_default();
        let mut resolved = ResolvedScope {
            requested: requested.clone(),
            ..Default::default()
        };
        if let Some(task) = &requested.task_ref {
            let id = self.scope_concept(task, create).await?;
            resolved.task = Some(id.clone());
            resolved.contexts.push(id);
        }
        for context in &requested.context_refs {
            resolved
                .contexts
                .push(self.scope_concept(context, create).await?);
        }
        resolved.contexts.sort();
        resolved.contexts.dedup();
        Ok(resolved)
    }

    async fn scope_concept(&self, handle: &str, create: bool) -> Result<String, KipError> {
        check_handle(handle)?;
        let nexus = self.memory.nexus();
        if let Ok(id) = handle.parse::<ElementId>() {
            return match nexus.store.get_element(id).await {
                Ok(Element::Concept(row))
                    if row.space == DEFAULT_SPACE && row.state == "active" =>
                {
                    Ok(id.to_string())
                }
                _ => Err(KipError::not_found_or_not_visible(
                    "the scope handle does not name an active Concept of this Space",
                )),
            };
        }
        let key = scope_key(handle);
        // The handle is whichever Concept carries its key, in any lifecycle
        // state short of purged. Maintenance sees it as an ordinary Event and
        // may archive, tombstone or merge it (its retention sweep archives a
        // lapsed one), but memory stays scoped by the id it was written under
        // — and the upsert below keeps writing to that id — so resolving only
        // an active handle would take the whole scope out of recall for good.
        let found = crate::kip::ok_result(
            &anda_kip::execute_request(
                nexus.as_ref(),
                &crate::kip::request_with(
                    "FIND(?c.id) WHERE { ?c {type: \"Event\", key: :key, state: ?state} \
                     FILTER(?state != \"purged\") } LIMIT 1",
                    crate::kip::param("key", key.as_str()),
                ),
            )
            .await,
        )
        .and_then(|rows| rows.as_array().and_then(|rows| rows.first()).cloned())
        .and_then(|row| crate::agents::first_row(row).as_str().map(str::to_string));
        if let Some(id) = found {
            return Ok(id);
        }
        if !create {
            // A handle nobody has written under yet: nothing is scoped to it.
            // It stays a distinct scope, never the general one.
            return Ok(format!("unbound:{handle}"));
        }
        let response = anda_kip::execute_request(
            nexus.as_ref(),
            &crate::kip::request_with(
                r#"UPSERT CONCEPT ?scope {
                    MATCH {type: "Event", key: :key}
                    SET FIELDS {name: :name}
                    SET ATTRIBUTES {event_class: "memory_scope", summary: :summary}
                }"#,
                serde_json::Map::from_iter([
                    ("key".into(), json!(key)),
                    ("name".into(), json!(handle)),
                    (
                        "summary".into(),
                        json!(format!("Memory scope handle {handle}, named by the host")),
                    ),
                ]),
            ),
        )
        .await;
        crate::kip::ok_result(&response)
            .and_then(|result| result["handles"]["scope"].as_str())
            .map(str::to_string)
            .ok_or_else(|| crate::kip::response_error(&response, "scope Concept was not created"))
    }
}

/// A request budget this binding can honor: the advertised tokenizer (or its
/// legacy spelling), a positive output bound and a deadline within
/// [`MAX_DEADLINE_MS`].
fn check_budget(budget: &wire::Budget) -> Result<(), KipError> {
    if let Some(tokenizer) = &budget.tokenizer
        && crate::recall_budget::canonical_tokenizer(tokenizer).is_none()
    {
        return Err(KipError::unsupported_capability(format!(
            "tokenizer {tokenizer:?} is not supported; this binding counts with {}",
            crate::recall_budget::TOKENIZER
        )));
    }
    if budget.max_output_tokens == Some(0) || budget.deadline_ms == Some(0) {
        return Err(KipError::invalid_request_envelope(
            "budget bounds must be positive",
        ));
    }
    if budget.deadline_ms.is_some_and(|ms| ms > MAX_DEADLINE_MS) {
        return Err(KipError::result_limit_exceeded(format!(
            "deadline_ms is at most {MAX_DEADLINE_MS}"
        )));
    }
    Ok(())
}

/// The response for a mutation intent, from its current progress (MI §5):
/// `succeeded` needs available progress, recorded work is `pending`, and a
/// terminal failure is `failed` with its error.
pub(crate) fn mutation_response(
    request: &Request,
    receipt: wire::Receipt,
    progress: wire::Progress,
    result: Json,
    warnings: Vec<String>,
) -> Response {
    let status = match progress.phase {
        wire::Phase::Available => Status::Succeeded,
        wire::Phase::Recorded => Status::Pending,
        wire::Phase::Processed => Status::Partial,
        wire::Phase::Failed => Status::Failed,
    };
    let error = (status == Status::Failed).then(|| {
        progress
            .error
            .clone()
            .unwrap_or_else(|| error_object(&KipError::internal_error("processing failed")))
    });
    Response {
        kip_memory: wire::KIP_MEMORY_VERSION.into(),
        request_id: request.request_id.clone(),
        operation: request.operation,
        status,
        receipt: Some(receipt),
        progress: Some(progress),
        result: (status != Status::Failed).then_some(result),
        error,
        warnings,
    }
}

/// Keeps a response's operation consistent when a partial result is reported.
pub(crate) fn with_status(mut response: Response, status: Status) -> Response {
    response.status = status;
    response
}
