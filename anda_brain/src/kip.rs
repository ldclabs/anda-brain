//! The KIP 2.0 envelope seam.
//!
//! KIP 2.0 replaced the 1.x request/response pair — one `command` string in, an
//! `Ok`/`Err` enum out — with a batch envelope: `operations[]` in, a
//! [`Response`] carrying a request-level status plus one
//! [`OperationResult`](anda_kip::OperationResult) per operation out. Almost
//! every read and write the brain issues is one deterministic, code-built
//! command, so this module keeps that case a one-liner instead of spreading
//! envelope construction and two-level error handling over thirty call sites.
//!
//! Two distinctions the helpers deliberately preserve rather than flatten:
//!
//! - a failure lives at the operation level for an ordinary error and at the
//!   request level only for an envelope error, so reading just one of them
//!   would turn half of the failures into an empty success;
//! - [`TopLevelStatus::OutcomeUnknown`] is not a failure. A write may have
//!   committed, so [`succeeded`] answers `false` without licensing a caller to
//!   redo the work: the settlement passes recover by re-running an idempotent
//!   write, never by treating the memory as unwritten.

use anda_core::Message;
use anda_engine::{rfc3339_datetime, rfc3339_datetime_now};
use anda_kip::{
    Command, ElementReference, ErrorObject, Executor, IngestContext, IngestEvidence, Json,
    KipError, KipValue, Map, MutationClause, Request, Response, Scalar, TopLevelStatus,
    execute_request, transition_state,
};
use jiff::{SignedDuration, civil::DateTime, fmt::strtime::BrokenDownTime, tz::Offset};
use std::num::TryFromIntError;

const UNIX_EPOCH: DateTime = jiff::civil::datetime(1970, 1, 1, 0, 0, 0, 0);

/// Builds a single-operation request from one command string.
pub fn request(command: impl Into<String>) -> Request {
    Request::single(command)
}

/// Renders a KIP string literal with backslashes and quotes escaped.
/// The crate's single escaping implementation — reuse it instead of
/// inlining `.replace()` chains that can drift apart.
pub fn string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Graph metadata timestamps are RFC3339 strings (lexicographically
/// comparable in KQL filters).
pub fn timestamp(now_ms: u64) -> String {
    rfc3339_datetime(now_ms).unwrap_or_else(rfc3339_datetime_now)
}

/// Validates a caller's observation time at the API boundary and returns its
/// canonical millisecond UTC spelling (Spec §6.5). An RFC 3339 instant in
/// another offset or with fewer fraction digits names the same instant and is
/// canonicalized; an unparseable value or sub-millisecond precision is refused
/// rather than replaced, because the instant is the claim's start key.
pub(crate) fn source_timestamp(value: &str) -> Result<String, String> {
    let parsed = Rfc3339Instant::parse(value)
        .ok_or_else(|| format!("timestamp {value:?} is not an RFC 3339 instant"))?;
    if !parsed
        .local
        .subsec_nanosecond()
        .unsigned_abs()
        .is_multiple_of(1_000_000)
    {
        return Err(format!(
            "timestamp {value:?} is finer than milliseconds; KIP timestamps are millisecond UTC"
        ));
    }
    let utc = parsed.utc().ok_or_else(|| {
        format!("timestamp {value:?} cannot be represented as a KIP UTC timestamp")
    })?;
    let canonical = anda_cognitive_nexus::time::format(utc);
    anda_kip::timestamp::parse(&canonical, "timestamp").map_err(|err| err.to_string())?;
    Ok(canonical)
}

/// The observation time of a stored formation input. New input is validated
/// by [`source_timestamp`] on submission; a stored value that cannot be read
/// falls back to its durable receipt time, so a retry captures the same
/// observation rather than a new wall-clock instant.
pub(crate) fn observation_timestamp(value: Option<&str>, received_at: u64) -> String {
    value
        .and_then(Rfc3339Instant::parse)
        .and_then(Rfc3339Instant::utc)
        .map(anda_cognitive_nexus::time::format)
        .filter(|value| anda_kip::timestamp::parse(value, "timestamp").is_ok())
        .unwrap_or_else(|| timestamp(received_at))
}

/// The Unix-millisecond instant of an RFC 3339 timestamp stored in memory, or
/// `None` when it does not read as one.
pub(crate) fn time_ms(value: &str) -> Option<u64> {
    let instant = Rfc3339Instant::parse(value)?;
    let mut elapsed =
        instant.local.duration_since(UNIX_EPOCH) - instant.offset.duration_since(Offset::UTC);
    if instant.leap_second {
        elapsed += SignedDuration::from_secs(1);
    }
    // Check before truncating so a fractional instant just before 1970 is refused.
    if elapsed.is_negative() {
        return None;
    }
    u64::try_from(elapsed.as_millis()).ok()
}

/// Converts a parsed, millisecond-precision KIP UTC datetime to Unix milliseconds.
pub(crate) fn unix_time_ms(utc: DateTime) -> Result<u64, TryFromIntError> {
    u64::try_from(utc.duration_since(UNIX_EPOCH).as_millis())
}

struct Rfc3339Instant {
    local: DateTime,
    offset: Offset,
    leap_second: bool,
}

impl Rfc3339Instant {
    fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        let (whole, mut suffix) = value.split_at_checked(19)?;
        // Require the RFC 3339 calendar and clock spelling before parsing with Jiff.
        if !whole.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            10 => matches!(byte, b'T' | b't' | b' '),
            13 | 16 => byte == b':',
            _ => byte.is_ascii_digit(),
        }) {
            return None;
        }

        let mut end = whole.len();
        if let Some(fraction) = suffix.strip_prefix('.') {
            let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            // Existing stored strings can specify more than nine fractional digits.
            end += 1 + digits.min(9);
            suffix = &fraction[digits..];
        }
        let offset = match suffix.as_bytes() {
            [b'Z' | b'z'] => Offset::UTC,
            [b'+' | b'-', h1, h2, b':', m1, m2]
                if [*h1, *h2] <= *b"23" && [h1, h2, m1, m2].into_iter().all(u8::is_ascii_digit) =>
            {
                BrokenDownTime::parse("%:z", suffix).ok()?.offset()?
            }
            _ => return None,
        };

        Some(Self {
            local: value[..end].parse().ok()?,
            // Jiff reads :60 as :59. Keep the original second for legacy reads.
            leap_second: &whole.as_bytes()[17..19] == b"60",
            offset,
        })
    }

    fn utc(self) -> Option<DateTime> {
        if self.leap_second {
            return None;
        }
        self.local
            .checked_sub(self.offset.duration_since(Offset::UTC))
            .ok()
    }
}

/// Builds a single-operation request with its parameter bindings.
///
/// Parameters are bound structurally to complete value positions, never spliced
/// into the command text: that is what keeps a value that came from a model or
/// a user from being read as syntax.
pub fn request_with(command: impl Into<String>, parameters: Map<String, Json>) -> Request {
    Request {
        parameters: Some(parameters),
        ..Request::single(command)
    }
}

/// Builds the parameter map for a single binding.
pub fn param(name: &str, value: impl Into<Json>) -> Map<String, Json> {
    Map::from_iter([(name.to_string(), value.into())])
}

/// Whether every operation in the response committed or answered.
///
/// `outcome_unknown` is not success and not failure; see the module docs.
pub fn succeeded(response: &Response) -> bool {
    response.status == TopLevelStatus::Succeeded
}

/// The error a response reports, whichever level it sits at.
pub fn error_of(response: &Response) -> Option<&ErrorObject> {
    response
        .error
        .as_ref()
        .or_else(|| response.results.iter().find_map(|r| r.error.as_ref()))
}

/// The error a failed response reports, as a [`KipError`] keeping its code,
/// or an internal error saying `fallback` when the response names none.
pub(crate) fn response_error(response: &Response, fallback: &str) -> KipError {
    error_of(response)
        .map(|error| {
            KipError::new(
                anda_kip::KipErrorCode::from_name(&error.code)
                    .unwrap_or(anda_kip::KipErrorCode::InternalError),
                error.message.clone(),
            )
        })
        .unwrap_or_else(|| KipError::internal_error(fallback))
}

/// The first operation's result value, or `None` when the request did not
/// succeed.
///
/// A succeeded operation that produced no value (`no_effect`) also answers
/// `None`: there is nothing to read either way, and the caller that wants to
/// tell them apart asks [`succeeded`].
pub fn ok_result(response: &Response) -> Option<&Json> {
    if !succeeded(response) {
        return None;
    }
    response.results.first().and_then(|r| r.result.as_ref())
}

/// Renders whatever a response failed with as one log-friendly line.
pub fn error_message(response: &Response) -> String {
    match error_of(response) {
        Some(error) => format!("{}: {}", error.code, error.message),
        None => format!("{:?}", response.status),
    }
}

/// How many elements a committed mutation changed under the given op.
///
/// A KML result is `{handles, changes: [...]}` carrying one §36.1 Change
/// Envelope entry per element; there is no scalar "updated" count, and a sweep
/// that reported the whole `changes` length would count the elements a single
/// statement touched incidentally.
///
/// The ops are `create`, `update`, `lifecycle`, `retention`, `merge`, `purge`
/// and `payload_purge`. Since KIP 2.0 collapsed the lifecycle statements into
/// one `TRANSITION` (§52.5), every move reports `lifecycle` and the state it
/// moved to lives in `state.to` — so a caller counting retractions or archives
/// wants [`transitioned`] rather than this.
pub fn changed(response: &Response, op: &str) -> u64 {
    count_changes(response, |change| {
        change.get("op").and_then(Json::as_str) == Some(op)
    })
}

/// How many elements a committed mutation moved *to* the given lifecycle state.
///
/// `TRANSITION` is one statement for nine states, so the op alone no longer
/// says what happened: an archive and a retraction are both `lifecycle`, and
/// counting the op would let a sweep report the wrong work. §36.1 puts the
/// move in `state.from` / `state.to`, and this reads the half that says what
/// the element became.
#[cfg_attr(not(feature = "wiki"), allow(dead_code))]
pub fn transitioned(response: &Response, to: &str) -> u64 {
    count_changes(response, |change| {
        change.get("op").and_then(Json::as_str) == Some("lifecycle")
            && change
                .get("state")
                .and_then(|state| state.get("to"))
                .and_then(Json::as_str)
                == Some(to)
    })
}

/// The Change Envelope entries of the first result that match.
fn count_changes(response: &Response, matches: impl Fn(&Json) -> bool) -> u64 {
    let Some(result) = ok_result(response) else {
        return 0;
    };
    result
        .get("changes")
        .and_then(Json::as_array)
        .map(|changes| changes.iter().filter(|change| matches(change)).count() as u64)
        .unwrap_or(0)
}

/// Runs a whole request envelope on a read-only path.
///
/// [`anda_kip::execute_readonly`] gates a single command; a batch needs the
/// same gate applied to every operation before any of them runs. The rejection
/// is on what each command parses to, never on the `language` label an
/// operation declares, so no envelope field can talk a write past this
/// boundary.
pub async fn execute_readonly_request(executor: &impl Executor, request: &Request) -> Response {
    match request.parse_operations() {
        Ok(commands) => {
            if commands.iter().any(|command| command.is_mutation()) {
                return Response::from(KipError::readonly_violation(
                    "this endpoint executes KQL and META only; KML mutations must go through the \
                     state-capable path",
                ))
                .with_request_id(request.request_id.clone());
            }
        }
        Err(err) => return Response::from(err).with_request_id(request.request_id.clone()),
    }

    execute_request(executor, request).await
}

/// The most elements one gated mutation clause may select.
///
/// Matches `MAX_MAINTENANCE_SELECTION` in the Worker's `src/kip.ts`: the two
/// engines have to agree on a bound both deployment contracts quote.
pub const MAX_GATED_SELECTION: u64 = 20;

/// The most messages one formation pass mints as Evidence.
///
/// The newest ones, because a formation pass writes about what was just said
/// and an envelope carrying an unbounded transcript is a request nobody
/// bounded. The older turns are still in the prompt; what a message past this
/// line loses is the *verbatim* record, so a claim resting on one has to be
/// written the long way — which the contract says, so the model is not left
/// guessing why `:msg17` does not resolve.
///
/// Matches `MAX_INGESTED_MESSAGES` in the Worker's `src/kip.ts`.
pub const MAX_INGESTED_MESSAGES: usize = 16;

/// When one captured message was observed: its own millisecond timestamp when
/// the caller sent one, else the batch's observation time. This is what an
/// Assertion citing the message writes as `at` (Spec §13.2).
pub(crate) fn message_observed_at(message: &Message, batch_observed_at: &str) -> String {
    message
        .timestamp
        .and_then(rfc3339_datetime)
        .unwrap_or_else(|| batch_observed_at.to_string())
}

/// The captured messages' Evidence keys and observation times, for the model
/// to set `at` on the claims it forms from them.
pub(crate) fn observation_manifest(ingest: &IngestContext) -> String {
    ingest
        .evidence
        .iter()
        .map(|entry| {
            format!(
                ":{} {} observed_at {}",
                entry.key,
                entry.evidence_class,
                entry.observed_at.as_deref().unwrap_or("unknown")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a message's role makes it, as an Evidence class (Formation §7).
///
/// A transcript is not one observation. Who said a thing is part of what was
/// observed, and flattening four turns into one payload would leave a later
/// reader unable to tell the user's words from the assistant's — which is the
/// distinction an attributed claim rests on.
fn evidence_class(role: &str) -> &'static str {
    match role {
        "user" => "user_statement",
        "assistant" => "agent_statement",
        "tool" => "tool_result",
        _ => "message",
    }
}

/// The observation a formation pass was called on, ready for the engine to
/// mint (Spec §71.1, Formation §7).
///
/// The point is fidelity, and it is worth stating plainly: a model that retypes
/// an observation into `CREATE EVIDENCE ... {payload: "…"}` truncates it,
/// normalizes its whitespace, fixes its spelling, or paraphrases it — and the
/// record then says the source said something they did not (§88.12). So the
/// payload the runtime received is the payload that is stored, and the model
/// only ever writes `:msg1`.
///
/// `client_key` is what makes this safe to attach to every request in a
/// multi-turn pass and to a retry of the whole conversation: the first mint
/// wins and the rest resolve to it (§52.1). `origin` is what its stability
/// rests on.
///
/// `source_actor` has to name something a reader can follow — an element id or
/// a canonical identity — and `context.counterparty` is a Concept *key*, so it
/// is the caller's job to resolve one and `None` is an ordinary answer. Here
/// the caller is [`FormationAgent::process_one`](crate::agents::FormationAgent),
/// which upserts the counterparty's Person before the pass and therefore always
/// has one; the Worker leaves that write to the model, so its first
/// conversation with someone mints Evidence without a source. Attribution does
/// not depend on it either way: who said the thing is `asserted_by` on the
/// Assertion.
pub fn observation_ingest(
    messages: &[Message],
    observed_at: &str,
    origin: &str,
    source_actor: Option<&str>,
) -> Option<IngestContext> {
    let start = messages.len().saturating_sub(MAX_INGESTED_MESSAGES);
    let recent = &messages[start..];
    if recent.is_empty() {
        return None;
    }
    // Numbered from the start of the kept window, so `:msg1` is the oldest
    // message the model can cite and the numbering matches the order it reads
    // them in.
    let evidence = recent
        .iter()
        .enumerate()
        .map(|(index, message)| IngestEvidence {
            key: format!("msg{}", index + 1),
            evidence_class: evidence_class(&message.role).to_string(),
            payload: serde_json::to_value(message).ok(),
            observed_at: Some(message_observed_at(message, observed_at)),
            client_key: Some(format!("{origin}:{}", start + index + 1)),
            source_actor: source_actor
                .filter(|_| message.role == "user")
                .map(ElementReference::by_id),
            ..Default::default()
        })
        .collect();
    Some(IngestContext {
        evidence,
        extensions: None,
    })
}

/// Attach captured source bytes when a request opens a write transaction.
/// Read-only requests must not carry ingest: KIP refuses observations that
/// have no transaction to mint into. A request with several writes is several
/// transactions, and each entry's `client_key` is what makes them resolve one
/// Evidence rather than mint a copy each (Spec §71.1); [`observation_ingest`]
/// always sets it. A model may neither replace the ingest
/// block nor shadow a captured source handle at either parameter level.
pub fn attach_observation(
    request: &mut Request,
    observation: &IngestContext,
) -> Result<(), String> {
    if request.ingest.is_some() {
        return Err("formation ingest is supplied by the host".into());
    }
    let claimed = |parameters: Option<&Map<String, Json>>| {
        parameters.is_some_and(|parameters| {
            observation
                .evidence
                .iter()
                .any(|entry| parameters.contains_key(&entry.key))
        })
    };
    if claimed(request.parameters.as_ref())
        || request
            .operations
            .iter()
            .any(|operation| claimed(operation.parameters.as_ref()))
    {
        return Err("captured observation bindings cannot be replaced".into());
    }
    if request.operations.iter().any(|operation| {
        operation
            .parse()
            .is_ok_and(|command| command.opens_write_transaction())
    }) {
        request.ingest = Some(observation.clone());
    }
    Ok(())
}

/// Request parameters the host binds on every gated model write.
///
/// `strength_policy` is this deployment's pin, so a `MnemonicState` base is
/// written with the policy the engine computes `effective_strength` under
/// (Profile §6.1); `now` is this write's host time, the anchor of a base
/// written by it. The model cites them as `:strength_policy` and `:now`.
pub const HOST_STRENGTH_POLICY: &str = "strength_policy";

/// The host's time for this write; see [`HOST_STRENGTH_POLICY`].
pub const HOST_NOW: &str = "now";

/// Binds the host's strength-policy pin and write time into a request that
/// writes. The pin is host-owned: a request binding its own is refused rather
/// than overridden, so what a base is pinned to is never a model's choice. A
/// model-bound `now` is its own business and is left as written.
pub fn attach_host_bindings(request: &mut Request, now_ms: u64) -> Result<(), String> {
    if !request.operations.iter().any(|operation| {
        operation
            .parse()
            .is_ok_and(|command| command.opens_write_transaction())
    }) {
        return Ok(());
    }
    let binds = |parameters: Option<&Map<String, Json>>, name: &str| {
        parameters.is_some_and(|parameters| parameters.contains_key(name))
    };
    if binds(request.parameters.as_ref(), HOST_STRENGTH_POLICY)
        || request
            .operations
            .iter()
            .any(|operation| binds(operation.parameters.as_ref(), HOST_STRENGTH_POLICY))
    {
        return Err("`:strength_policy` is bound by the host; cite it without binding it".into());
    }
    let parameters = request.parameters.get_or_insert_with(Map::new);
    parameters.insert(
        HOST_STRENGTH_POLICY.to_string(),
        crate::cognitive::STRENGTH_POLICY.clone(),
    );
    parameters
        .entry(HOST_NOW.to_string())
        .or_insert_with(|| Json::from(timestamp(now_ms)));
    Ok(())
}

/// Binds a Memory Interface pass's host parameters (its context set, its
/// MemoryScope and, for a misrecording, the original source) into a request
/// that writes. Host-owned like the strength pin: a request binding one of
/// these names itself is refused rather than overridden.
pub fn attach_intent_bindings(
    request: &mut Request,
    bindings: Map<String, Json>,
) -> Result<(), String> {
    if !request.operations.iter().any(|operation| {
        operation
            .parse()
            .is_ok_and(|command| command.opens_write_transaction())
    }) {
        return Ok(());
    }
    let binds = |parameters: Option<&Map<String, Json>>| {
        parameters
            .is_some_and(|parameters| bindings.keys().any(|name| parameters.contains_key(name)))
    };
    if binds(request.parameters.as_ref())
        || request
            .operations
            .iter()
            .any(|operation| binds(operation.parameters.as_ref()))
    {
        return Err(
            "`:contexts`, `:scope_task` and `:orig` are bound by the host; cite them without \
             binding them"
                .into(),
        );
    }
    request
        .parameters
        .get_or_insert_with(Map::new)
        .extend(bindings);
    Ok(())
}

/// Why a `MnemonicState` write leaves strength unknown, if it does.
///
/// `effective_strength` is computed from the base, its anchor and a pinned
/// policy together, and reads `null` when any one is missing (Profile §6.1,
/// Spec §59.1). A base written without the other two is therefore a memory
/// whose strength nobody can ever read, so a write that sets
/// `memory_strength` has to set `last_metabolized_at` and `strength_policy`
/// with it. Checked on member names, which the grammar fixes.
fn incomplete_strength(
    command: &Command,
    parameters: Option<&Map<String, Json>>,
) -> Option<String> {
    use anda_kip::{FacetAssignment, SymbolRef, UpdateAction};
    let Command::Kml(statement) = command else {
        return None;
    };
    let mut assignments: Vec<&FacetAssignment> = Vec::new();
    for clause in &statement.clauses {
        match clause {
            MutationClause::CreateConcept(c) => assignments.extend(&c.set_facets),
            MutationClause::UpsertConcept(c) => assignments.extend(&c.set_facets),
            MutationClause::Update(c) => {
                assignments.extend(c.actions.iter().filter_map(|action| match action {
                    UpdateAction::SetFacet(facet) => Some(facet),
                    _ => None,
                }))
            }
            _ => {}
        }
    }
    for assignment in assignments {
        let name = match &assignment.facet {
            SymbolRef::Name(name) => Some(name.as_str()),
            SymbolRef::Param(name) => parameters.and_then(|p| p.get(name)).and_then(Json::as_str),
        };
        if name.is_none_or(|name| name.rsplit('/').next() != Some("MnemonicState")) {
            continue;
        }
        let writes = |member: &str| assignment.values.iter().any(|(name, _)| name == member);
        if writes("memory_strength")
            && !(writes("last_metabolized_at") && writes("strength_policy"))
        {
            return Some(
                "a MnemonicState memory_strength is a base: write it with its anchor and the \
                 host's policy pin, `last_metabolized_at: :now, strength_policy: \
                 :strength_policy`, or leave strength unwritten (Profile §6.1)"
                    .to_string(),
            );
        }
    }
    None
}

/// Why a claim taken from a captured message is undated, if one is.
///
/// A claim's `asserted_at` is its start key for temporal succession, and it
/// defaults to the transaction time (Spec §13.2, §25.4). A message processed
/// late would then be read as said now, and could end a value that is newer
/// than it — the case Memory Interface §5.1 warns about. So an Assertion
/// citing one of this request's captured messages has to carry `at` (or a
/// written `valid.from`). The Brain's own inferences are exempt: they are made
/// when they are written. A parameter may carry either value, and is given the
/// benefit of the doubt, as `anda_kip`'s own `KIP_2103` check does.
fn undated_citation(command: &Command, captured: &[&str]) -> Option<String> {
    use anda_kip::{BoundValue, MutationValue, SymbolRef};
    let Command::Kml(statement) = command else {
        return None;
    };
    for clause in &statement.clauses {
        let MutationClause::CreateAssertion(record) = clause else {
            continue;
        };
        let cites_capture = record.set_structural.iter().flatten().any(|edge| {
            matches!(&edge.field, SymbolRef::Name(field) if field == "evidence")
                && matches!(&edge.value, MutationValue::Param(name) if captured.contains(&name.as_str()))
        });
        if !cites_capture {
            continue;
        }
        let fields = record.set_fields.as_deref().unwrap_or_default();
        let field = |name: &str| {
            fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value)
        };
        if matches!(field("mode"), Some(MutationValue::Value(KipValue::String(mode))) if mode == "inferred")
        {
            continue;
        }
        let dated = field("asserted_at")
            .is_some_and(|value| !matches!(value, MutationValue::Value(KipValue::Null)));
        let starts = field("valid_time").is_some_and(|value| match value {
            MutationValue::Value(KipValue::Object(members)) => members
                .get("from")
                .is_some_and(|from| !matches!(from, KipValue::Null)),
            MutationValue::Value(_) => false,
            MutationValue::Object(members) => members.iter().any(|(key, from)| {
                key == "from" && !matches!(from, BoundValue::Value(KipValue::Null))
            }),
            _ => true,
        });
        if !dated && !starts {
            return Some(
                "an ASSERT citing a captured message needs `at:` — that message's observed_at \
                 from the Captured Evidence list — or a `valid: {from: …}`; without it the \
                 claim starts at this write's time and a late-processed message would end a \
                 newer value (Spec §13.2, §25.4)"
                    .to_string(),
            );
        }
    }
    None
}

/// Runs a whole request envelope on the cognition-only path.
///
/// Formation encodes what it observed: Concepts, the Propositions relating
/// them, the Evidence they rest on, the Assertions that take a stance, and the
/// corrections that revise one — which in KIP 2.0 is a *new* Assertion plus
/// supersession, so `TRANSITION ... TO "superseded" BY` and `TO "corrected" BY`
/// belong here even though they change a claim's standing.
///
/// The six lifecycle statements collapsed into one `TRANSITION` in KIP 2.0
/// (§52.5), so the split that used to fall between verbs now falls inside one:
/// the cognitive moves (`retracted`, `superseded`, `corrected`, and an
/// Activity's own `running` / `completed` / `failed` / `cancelled`) stay,
/// `archived` and `tombstoned` do not. A state written as a `:parameter` is
/// resolved against the request's bindings first, and refused when it cannot
/// be — otherwise the collapse would have handed Formation a binding-shaped
/// way to tombstone.
///
/// What is refused is refused deliberately. `UPDATE`, `SET RETENTION`, `PURGE`,
/// `PURGE PAYLOAD` and `MERGE CONCEPT` — and the removal half of
/// `TRANSITION` — act on
/// memory in bulk from a selection, and a pass whose entire input is an
/// untrusted conversation is the last thing that should hold them. The
/// reference policy assumes an authority model this deployment cannot express
/// — every agent runs as the Space's system Principal, and the KIP request
/// envelope carries no Principal of its own, so Governance grants cannot
/// separate the agents — and this gate is where that separation lives instead.
/// [`execute_maintenance_request`] is the wider half of it.
///
/// Reads are untouched: Formation has to ground before it writes (reference
/// policy §11), so KQL and META pass through.
///
/// Like [`execute_readonly_request`], the decision is on what each command
/// parses to, never on a label the caller attached to it.
pub async fn execute_cognition_request(executor: &impl Executor, request: &Request) -> Response {
    let commands = match request.parse_operations() {
        Ok(commands) => commands,
        Err(err) => return Response::from(err).with_request_id(request.request_id.clone()),
    };
    if let Some(refusal) = define_batch_refusal(&commands) {
        return Response::from(KipError::not_authorized(refusal))
            .with_request_id(request.request_id.clone());
    }
    let captured: Vec<&str> = request
        .ingest
        .iter()
        .flat_map(|ingest| ingest.evidence.iter().map(|entry| entry.key.as_str()))
        .collect();
    for (index, command) in commands.iter().enumerate() {
        if let Some(refusal) = undated_citation(command, &captured) {
            return Response::from(KipError::constraint_violation(refusal))
                .with_request_id(request.request_id.clone());
        }
        let parameters = operation_parameters(request, index);
        if let Some(refusal) = unsupported_cognitive_write(command, parameters.as_ref()) {
            return Response::from(KipError::unsupported_capability(refusal))
                .with_request_id(request.request_id.clone());
        }
        if let Some(refusal) = incomplete_strength(command, parameters.as_ref()) {
            return Response::from(KipError::constraint_violation(refusal))
                .with_request_id(request.request_id.clone());
        }
        if let Some(refusal) = cognition_refusal(command, parameters.as_ref()) {
            return Response::from(KipError::not_authorized(refusal))
                .with_request_id(request.request_id.clone());
        }
    }

    execute_request(executor, request).await
}

/// The parameters one operation actually executes with. KIP operation-level
/// bindings shadow request-level bindings with the same key; every preflight
/// gate must inspect that same environment or a checked value can differ from
/// the value the engine later applies.
fn operation_parameters(request: &Request, index: usize) -> Option<Map<String, Json>> {
    let request_parameters = request.parameters.as_ref();
    let operation_parameters = request
        .operations
        .get(index)
        .and_then(|operation| operation.parameters.as_ref());
    match (request_parameters, operation_parameters) {
        (None, None) => None,
        (outer, inner) => {
            let mut merged = outer.cloned().unwrap_or_default();
            merged.extend(inner.cloned().unwrap_or_default());
            Some(merged)
        }
    }
}

/// Facets a model plan cannot write: validated learning records and protected
/// runtime state belong to host bindings, and `GradingState` is a computed view
/// of the Skill's `current_evaluation`.
const PROTECTED_FACETS: [&str; 7] = [
    "TrialRecord",
    "EvaluationRecord",
    "OutcomeRecord",
    "AttemptRecord",
    "GradingState",
    "WatchState",
    "LeaseState",
];

/// Structural fields a model plan cannot write: the Skill's learning pointers
/// move only with the host's trial and verdict transactions, and the lineage
/// fields are computed from Activity provenance.
const PROTECTED_STRUCTURAL: [&str; 6] = [
    "current_trial",
    "current_evaluation",
    "derived_from",
    "compiled_from",
    "compiled_by",
    "consolidated_to",
];

/// Model plans do not own protected runtime or validated learning records.
/// Inspect AST facet and structural positions only: quoted source text
/// containing these words is ordinary Evidence. Parameterized names must
/// resolve before execution.
fn unsupported_cognitive_write(
    command: &Command,
    parameters: Option<&Map<String, Json>>,
) -> Option<String> {
    use anda_kip::{SymbolRef, UpdateAction};
    let Command::Kml(statement) = command else {
        return None;
    };
    let mut facets = Vec::new();
    let mut structural = Vec::new();
    for clause in &statement.clauses {
        match clause {
            MutationClause::CreateConcept(c) => {
                facets.extend(c.set_facets.iter().map(|f| &f.facet));
                structural.extend(c.set_structural.iter().flatten().map(|e| &e.field));
            }
            MutationClause::UpsertConcept(c) => {
                facets.extend(c.set_facets.iter().map(|f| &f.facet));
                facets.extend(c.unset_facets.iter().map(|f| &f.facet));
                structural.extend(c.set_structural.iter().flatten().map(|e| &e.field));
                structural.extend(c.unset_structural.iter().flatten().map(|e| &e.field));
            }
            MutationClause::CreateEvidence(c)
            | MutationClause::CreateAssertion(c)
            | MutationClause::CreateActivity(c) => {
                facets.extend(c.set_facets.iter().map(|f| &f.facet));
                structural.extend(c.set_structural.iter().flatten().map(|e| &e.field));
            }
            MutationClause::Transition(t) => {
                structural.extend(t.set_structural.iter().flatten().map(|e| &e.field));
            }
            MutationClause::Update(c) => {
                for action in &c.actions {
                    match action {
                        UpdateAction::SetFacet(f) => facets.push(&f.facet),
                        UpdateAction::UnsetFacet(f) => facets.push(&f.facet),
                        UpdateAction::SetStructural(edges) => {
                            structural.extend(edges.iter().map(|e| &e.field))
                        }
                        UpdateAction::UnsetStructural(edges) => {
                            structural.extend(edges.iter().map(|e| &e.field))
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    let resolve = |symbol: &SymbolRef| match symbol {
        SymbolRef::Name(name) => Some(name.clone()),
        SymbolRef::Param(name) => parameters
            .and_then(|p| p.get(name))
            .and_then(Json::as_str)
            .map(str::to_string),
    };
    let local = |name: &str| name.rsplit('/').next().unwrap_or(name).to_string();
    for facet in facets {
        let Some(name) = resolve(facet) else {
            return Some("model facet names must resolve before execution".into());
        };
        if PROTECTED_FACETS.contains(&local(&name).as_str()) {
            return Some(format!(
                "{name} requires a configured host learning/runtime binding or is computed by \
                 Nexus; it cannot be authored by a model plan"
            ));
        }
    }
    for field in structural {
        let Some(name) = resolve(field) else {
            return Some("model structural field names must resolve before execution".into());
        };
        if PROTECTED_STRUCTURAL.contains(&local(&name).as_str()) {
            return Some(format!(
                "{name} is moved by host learning transactions or computed from Activity \
                 provenance; it cannot be authored by a model plan"
            ));
        }
    }
    None
}

/// Why the cognition-only path refuses this command, if it does.
fn cognition_refusal(command: &Command, parameters: Option<&Map<String, Json>>) -> Option<String> {
    let Command::Kml(statement) = command else {
        return None;
    };
    for clause in &statement.clauses {
        let verb = match clause {
            // `TRANSITION` is the only one of these that selects, and the
            // shared bound below is what keeps an unbounded
            // `TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {} }` from
            // withdrawing every claim in the Space in one statement.
            MutationClause::CreateConcept(_)
            | MutationClause::UpsertConcept(_)
            | MutationClause::EnsureProposition(_)
            | MutationClause::CreateEvidence(_)
            | MutationClause::CreateAssertion(_)
            | MutationClause::CreateActivity(_) => {
                if let Some(refusal) = unbounded_selection(clause, parameters) {
                    return Some(refusal);
                }
                continue;
            }

            MutationClause::Transition(transition) => {
                match transition_state_of(transition, parameters) {
                    Some(state) if COGNITIVE_TRANSITIONS.contains(&state) => {
                        if let Some(refusal) = unbounded_selection(clause, parameters) {
                            return Some(refusal);
                        }
                        continue;
                    }
                    Some(state) => {
                        return Some(format!(
                            "formation cannot TRANSITION memory to \"{state}\"; it corrects \
                             cognition (retracted / superseded / corrected) and runs its own \
                             Activities. Removing memory belongs to maintenance"
                        ));
                    }
                    // A `:parameter` nothing in this request binds to a
                    // literal. Refused rather than guessed: the engine would
                    // resolve it later, and "later" is past this gate.
                    None => {
                        return Some(
                            "formation must write the TRANSITION state as a literal this \
                             request can be read against; a state bound elsewhere cannot be \
                             checked before it runs"
                                .to_string(),
                        );
                    }
                }
            }

            MutationClause::Define(define) => {
                if let Some(refusal) = define_refusal(define) {
                    return Some(refusal);
                }
                continue;
            }

            MutationClause::Update(_) => "UPDATE",
            MutationClause::SetRetention(_) => "SET RETENTION",
            MutationClause::Purge(_) => "PURGE",
            MutationClause::PurgePayload(_) => "PURGE PAYLOAD",
            MutationClause::MergeConcept(_) => "MERGE CONCEPT",
        };

        return Some(format!(
            "formation cannot issue {verb}; it writes cognition (CREATE / UPSERT / ENSURE / \
             ASSERT) and corrects it (TRANSITION to retracted / superseded / corrected). \
             Administering memory in bulk belongs to maintenance"
        ));
    }
    None
}

/// Why a request's `DEFINE`s cannot run as sent, if they cannot.
///
/// A draft symbol commits on its own and must exist before the `MUTATE` that
/// uses it (Spec §20.16), so a request that defines is made of `DEFINE`s only
/// — the host runs them independently, so one symbol that already exists does
/// not stop the rest — and carries at most
/// [`MAX_DEFINES_PER_REQUEST`](crate::vocabulary::MAX_DEFINES_PER_REQUEST).
fn define_batch_refusal(commands: &[Command]) -> Option<String> {
    let defines = commands
        .iter()
        .filter(|command| command.is_mutation() && !command.opens_write_transaction())
        .count();
    if defines == 0 {
        return None;
    }
    if defines != commands.len() {
        return Some(
            "send DEFINE in its own request, before the MUTATE that uses the symbol; a draft \
             symbol commits on its own and resolves from the next request"
                .to_string(),
        );
    }
    let max = crate::vocabulary::MAX_DEFINES_PER_REQUEST;
    (defines > max).then(|| {
        format!("one request may define at most {max} symbols; reuse existing symbols first")
    })
}

/// Why Formation may not send this `DEFINE`, if it may not.
///
/// The engine checks the body against §20.16 and the Schema Environment; the
/// host adds what makes a draft reviewable. The name and body are literals, so
/// what is defined is what the request says rather than what a binding
/// resolves to later, and names keep this brain's shapes — `drug`, `Drug` and
/// `medical device` would otherwise be three symbols meaning one thing, and a
/// draft symbol is never removed.
fn define_refusal(define: &anda_kip::DefineCommand) -> Option<String> {
    let anda_kip::SymbolRef::Name(name) = &define.name else {
        return Some("write the DEFINE name as a literal, not a parameter".to_string());
    };
    if !crate::vocabulary::is_symbol_name(define.kind, name) {
        return Some(format!(
            "`{name}` is not a symbol name this brain drafts: Concept types are UpperCamelCase \
             letters and digits, predicates are snake_case, at most {} characters, and never a \
             Core element kind",
            crate::vocabulary::MAX_SYMBOL_CHARS
        ));
    }
    if define
        .definition
        .values()
        .any(|value| !matches!(value, anda_kip::BoundValue::Value(_)))
    {
        return Some(
            "write the DEFINE body as literals; a bound body cannot be reviewed before it runs"
                .to_string(),
        );
    }
    None
}

/// The lifecycle states the cognition-only path may name (§52.5).
///
/// Everything §52.5 registers except `archived` and `tombstoned`: correcting a
/// claim is cognition, and removing memory is custody.
const COGNITIVE_TRANSITIONS: &[&str] = &[
    transition_state::RETRACTED,
    transition_state::SUPERSEDED,
    transition_state::CORRECTED,
    transition_state::RUNNING,
    transition_state::COMPLETED,
    transition_state::FAILED,
    transition_state::CANCELLED,
];

/// The state a `TRANSITION` names, resolving a `:parameter` against the
/// request's own bindings.
///
/// `None` means the gate cannot know — an unbound parameter, or one bound to
/// something that is not a string — which every caller here treats as a
/// refusal rather than as permission.
fn transition_state_of<'a>(
    transition: &'a anda_kip::Transition,
    parameters: Option<&'a Map<String, Json>>,
) -> Option<&'a str> {
    match &transition.to {
        Scalar::Literal(KipValue::String(state)) => Some(state.as_str()),
        Scalar::Param(name) => parameters?.get(name)?.as_str(),
        _ => None,
    }
}

/// Runs a whole request envelope on the maintenance path.
///
/// Maintenance administers memory, so it keeps the verbs Formation does not:
/// `UPDATE`, `SET RETENTION`, `TRANSITION` — including to `archived` and
/// `tombstoned` — and `MERGE CONCEPT` are
/// the custodial work the reference policy asks it for. Three things it still
/// does not get, and each for its own reason:
///
/// - **`PURGE` and `PURGE PAYLOAD`.** Erasure is irreversible, and a model
///   reading a snapshot of its own graph is not where "this should stop having
///   existed" gets decided. This deployment already erases — the right-to-be-
///   forgotten path in [`crate::space`] issues `PURGE` itself, deterministically,
///   from a request a person made. `PURGE PAYLOAD` is refused on the same
///   grounds rather than lesser ones: it leaves the Evidence record, its digest
///   and its citations standing and destroys the bytes underneath them, so an
///   Assertion keeps citing an observation whose content is gone.
/// - **`legal_hold`, in either direction.** A hold blocks erasure for everyone
///   (Spec §60.3), so a plan that could place one could make its own cognition
///   undeletable, and one that could lift one could unblock an erasure somebody
///   placed a hold to stop. Setting a retention class and an `expires_at` is
///   ordinary lifecycle judgement and stays; the hold is not.
/// - **An unbounded selection.** `TRANSITION ?e TO "archived" WHERE { ?e
///   CONCEPT {} }` and `UPDATE ?e SET ATTRIBUTES {…} WHERE { ?e CONCEPT {} }`
///   are one hazard wearing two verbs, so the bound is on the selection rather
///   than on a verb by name.
///
/// `MERGE CONCEPT` takes no `LIMIT` and needs none: both engines resolve each
/// operand to exactly one Concept and refuse a pattern that binds several,
/// which is a better answer than anything this gate could give.
///
/// Matches `assertMaintenanceOperations` in the Worker's `src/kip.ts`. The two
/// deployments run different engines and the same policy, and a verb one of
/// them refuses is not a verb the other may quietly keep.
pub async fn execute_maintenance_request(executor: &impl Executor, request: &Request) -> Response {
    let commands = match request.parse_operations() {
        Ok(commands) => commands,
        Err(err) => return Response::from(err).with_request_id(request.request_id.clone()),
    };
    for (index, command) in commands.iter().enumerate() {
        let parameters = operation_parameters(request, index);
        if let Some(refusal) = unsupported_cognitive_write(command, parameters.as_ref()) {
            return Response::from(KipError::unsupported_capability(refusal))
                .with_request_id(request.request_id.clone());
        }
        if let Some(refusal) = incomplete_strength(command, parameters.as_ref()) {
            return Response::from(KipError::constraint_violation(refusal))
                .with_request_id(request.request_id.clone());
        }
        if let Some(refusal) = maintenance_refusal(command, parameters.as_ref()) {
            return Response::from(KipError::not_authorized(refusal))
                .with_request_id(request.request_id.clone());
        }
    }

    execute_request(executor, request).await
}

/// Why the maintenance path refuses this command, if it does.
fn maintenance_refusal(
    command: &Command,
    parameters: Option<&Map<String, Json>>,
) -> Option<String> {
    let Command::Kml(statement) = command else {
        return None;
    };
    for clause in &statement.clauses {
        match clause {
            MutationClause::Define(_) => {
                return Some(
                    "maintenance reviews draft vocabulary and never defines it: record a \
                     near-synonym as an Insight about the existing symbol, and put a symbol \
                     worth keeping in the report as a proposed promotion {kind, from, to}"
                        .to_string(),
                );
            }
            MutationClause::Purge(_) | MutationClause::PurgePayload(_) => {
                return Some(
                    "maintenance cannot issue PURGE or PURGE PAYLOAD; erasure is irreversible \
                     and this deployment runs it deterministically from a person's forget \
                     request, never from a maintenance plan. TRANSITION to archived or \
                     tombstoned is yours"
                        .to_string(),
                );
            }

            // Checked on the member name, which the grammar fixes, so a
            // parameterised value cannot smuggle it past: `{legal_hold: :x}`
            // is refused on the name alone, before anything is evaluated.
            MutationClause::SetRetention(retention)
                if retention
                    .values
                    .iter()
                    .any(|(name, _)| name == "legal_hold") =>
            {
                return Some(
                    "maintenance cannot place or lift a legal hold; set a retention class and \
                     an expires_at, and leave the hold to a person"
                        .to_string(),
                );
            }

            _ => {}
        }

        if let Some(refusal) = unbounded_selection(clause, parameters) {
            return Some(refusal);
        }
    }
    None
}

/// Why this clause's selection is too wide, if it is.
///
/// `None` for a clause that names its target outright: the bound exists to
/// stop a *pattern* from reaching further than the writer meant, and
/// `TRANSITION "C-7" TO "archived"` reaches exactly one element by
/// construction.
fn unbounded_selection(
    clause: &MutationClause,
    parameters: Option<&Map<String, Json>>,
) -> Option<String> {
    let (verb, where_clauses, limit) = match clause {
        MutationClause::Update(c) => ("UPDATE", &c.where_clauses, &c.limit),
        MutationClause::Transition(c) => ("TRANSITION", &c.where_clauses, &c.limit),
        MutationClause::SetRetention(c) => ("SET RETENTION", &c.where_clauses, &c.limit),
        MutationClause::Purge(c) => ("PURGE", &c.where_clauses, &c.limit),
        MutationClause::PurgePayload(c) => ("PURGE PAYLOAD", &c.where_clauses, &c.limit),
        _ => return None,
    };
    if where_clauses
        .as_ref()
        .is_none_or(|clauses| clauses.is_empty())
    {
        return None;
    }
    match selection_limit(limit.as_ref(), parameters) {
        Some(limit) if limit <= MAX_GATED_SELECTION => None,
        _ => Some(format!(
            "a {verb} that selects with WHERE must carry LIMIT {MAX_GATED_SELECTION} or less"
        )),
    }
}

/// The positive integer a `LIMIT` slot holds, resolving a `:parameter` against
/// the request's own bindings.
///
/// Refusing every parameterised limit would push a model toward splicing the
/// number into the command text, which is the habit binding exists to break.
fn selection_limit(limit: Option<&Scalar>, parameters: Option<&Map<String, Json>>) -> Option<u64> {
    match limit? {
        Scalar::Literal(KipValue::Number(number)) => number.as_u64(),
        Scalar::Param(name) => parameters?.get(name)?.as_u64(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anda_kip::{KipError, OperationResult};
    use serde_json::json;

    #[test]
    fn a_single_command_request_carries_one_operation() {
        let req = request("DESCRIBE PRIMER");
        assert_eq!(req.kip, "2.0");
        assert_eq!(req.operations.len(), 1);
        assert_eq!(
            req.operations[0].command.as_deref(),
            Some("DESCRIBE PRIMER")
        );
        req.validate().unwrap();
    }

    #[test]
    fn parameters_ride_the_envelope_rather_than_the_command_text() {
        let req = request_with("FIND(?x) WHERE { ?x {type: :t} }", param("t", "Person"));
        assert_eq!(req.parameters.as_ref().unwrap()["t"], Json::from("Person"));
        req.validate().unwrap();
    }

    #[test]
    fn an_operation_level_failure_is_not_read_as_an_empty_success() {
        let response = Response::from_results(vec![
            OperationResult::ok(json!(1)),
            OperationResult::failed(KipError::not_found_or_not_visible("nothing here")),
        ]);
        assert!(response.error.is_none(), "the failure is per-operation");
        assert!(!succeeded(&response));
        assert_eq!(ok_result(&response), None);
        assert_eq!(error_of(&response).unwrap().code, "NotFoundOrNotVisible");
    }

    #[test]
    fn an_unknown_outcome_does_not_read_as_success() {
        // Spec §80.3: the write may have committed, so this is not a failure
        // either — a settlement pass answers it by retrying an idempotent
        // write, never by treating the memory as unwritten.
        let response = Response::outcome_unknown(KipError::outcome_unknown("connection dropped"));
        assert!(!succeeded(&response));
        assert_eq!(response.status, TopLevelStatus::OutcomeUnknown);
    }

    /// The one command in a request, parsed.
    fn parsed(command: &str) -> Command {
        request(command)
            .parse_operations()
            .expect("the command parses")
            .remove(0)
    }

    #[test]
    fn the_cognition_gate_admits_what_formation_encodes() {
        for command in [
            // Reads: Formation grounds before it writes (§11).
            r#"FIND(?c.id) WHERE { ?c CONCEPT {type: "Person"} } LIMIT 5"#,
            "LIST PREDICATES LIMIT 100",
            "DESCRIBE PRIMER",
            // The everyday write, and its desugaring.
            r#"MUTATE { ASSERT (:alice, "prefers", :dark) { by: :alice, mode: "stated" } }"#,
            r#"MUTATE {
  CREATE EVIDENCE ?e { SET FIELDS { evidence_class: "user_statement", payload: :p } }
  UPSERT CONCEPT ?a { MATCH {type: "Person", key: :k} SET FIELDS {name: :n} }
  ENSURE PROPOSITION ?p2 (?a, "prefers", :dark)
  CREATE ASSERTION ?as { SET FIELDS { proposition: ?p2, asserted_by: ?a, stance: "support" } }
  CREATE ACTIVITY ?act { SET FIELDS { activity_class: "extraction", status: "completed" } }
}"#,
            // Correction preserves history, so its verbs are cognition too.
            r#"MUTATE { ASSERT (:alice, "prefers", :light) { by: :alice, mode: "stated" } SUPERSEDING :old }"#,
            r#"TRANSITION :old TO "corrected" BY :new"#,
            r#"TRANSITION :act TO "completed""#,
            // A bounded retraction of one's own claim.
            r#"TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {asserted_by: :alice} } LIMIT 5"#,
            r#"TRANSITION :a TO "retracted""#,
        ] {
            assert_eq!(
                cognition_refusal(&parsed(command), None),
                None,
                "should be admitted: {command}"
            );
        }
    }

    #[test]
    fn the_cognition_gate_refuses_administering_memory_in_bulk() {
        // Every one of these is reachable from a conversation the Brain did
        // not write, and every one of them acts on memory the conversation
        // never mentioned.
        for (command, verb) in [
            (
                r#"UPDATE ?c SET FACET "MnemonicState" {salience: 0.1} WHERE { ?c CONCEPT {} } LIMIT 5"#,
                "UPDATE",
            ),
            (
                r#"SET RETENTION ?c { retention_class: "standard" } WHERE { ?c CONCEPT {} } LIMIT 5"#,
                "SET RETENTION",
            ),
            (
                r#"TRANSITION ?c TO "archived" WHERE { ?c CONCEPT {} } LIMIT 5"#,
                "archived",
            ),
            (
                r#"TRANSITION ?c TO "tombstoned" WHERE { ?c CONCEPT {} } LIMIT 5"#,
                "tombstoned",
            ),
            (
                r#"PURGE ?c WHERE { ?c CONCEPT {} } LIMIT 5 CONFIRM "PURGE""#,
                "PURGE",
            ),
            (
                r#"PURGE PAYLOAD ?e WHERE { ?e EVIDENCE {} } LIMIT 5 CONFIRM "PURGE""#,
                "PURGE PAYLOAD",
            ),
            (r#"MERGE CONCEPT :dup INTO :canonical"#, "MERGE CONCEPT"),
        ] {
            let refusal = cognition_refusal(&parsed(command), None)
                .unwrap_or_else(|| panic!("should be refused: {command}"));
            assert!(refusal.contains(verb), "{refusal}");
        }
    }

    #[test]
    fn a_selecting_retraction_must_say_how_much_it_may_withdraw() {
        // Allowed by verb, refused by blast radius: without a bound this
        // withdraws every claim in the Space in one statement.
        let unbounded = r#"TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {} }"#;
        assert!(cognition_refusal(&parsed(unbounded), None).is_some());

        let over_budget = r#"TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {} } LIMIT 500"#;
        assert!(cognition_refusal(&parsed(over_budget), None).is_some());

        // A parameterised limit resolves against the envelope's own bindings,
        // so a model is not pushed into splicing the number into the text.
        let bound = r#"TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {} } LIMIT :n"#;
        assert_eq!(
            cognition_refusal(&parsed(bound), Some(&param("n", 5))),
            None
        );
        assert!(cognition_refusal(&parsed(bound), Some(&param("n", 5000))).is_some());
        // An unbound `:n` is not a bound at all.
        assert!(cognition_refusal(&parsed(bound), None).is_some());
    }

    #[test]
    fn a_transition_state_the_gate_cannot_read_is_refused() {
        // Six statements collapsed into one, so the split now falls inside the
        // statement. A state the gate cannot resolve to a literal is refused
        // rather than guessed: the engine would resolve it later, and "later"
        // is past this gate — which would have handed an untrusted conversation
        // a binding-shaped way to tombstone.
        let parameterised = r#"TRANSITION :a TO :state"#;
        assert_eq!(
            cognition_refusal(&parsed(parameterised), Some(&param("state", "retracted"))),
            None,
            "a bound cognitive state resolves and passes"
        );
        let refusal =
            cognition_refusal(&parsed(parameterised), Some(&param("state", "tombstoned")))
                .expect("a bound removal state is refused on the state it names");
        assert!(refusal.contains("tombstoned"), "{refusal}");

        for parameters in [
            None,
            Some(param("other", "retracted")),
            Some(param("state", 7)),
        ] {
            let refusal = cognition_refusal(&parsed(parameterised), parameters.as_ref())
                .expect("an unreadable state is refused");
            assert!(refusal.contains("literal"), "{refusal}");
        }
    }

    #[test]
    fn the_cognition_gate_reads_the_command_not_the_label() {
        // The refusal is on what the text parses to. A `MUTATE` that opens
        // with a legal clause and closes with `PURGE` is the purge it is.
        let smuggled = r#"MUTATE {
  CREATE ACTIVITY ?a { SET FIELDS { activity_class: "extraction", status: "completed" } }
  PURGE :victim CONFIRM "PURGE"
}"#;
        let refusal = cognition_refusal(&parsed(smuggled), None).expect("refused");
        assert!(refusal.contains("PURGE"), "{refusal}");
    }

    #[test]
    fn the_maintenance_gate_admits_the_custodial_verbs() {
        for command in [
            // Reads, and the vocabulary review §15 asks for.
            r#"FIND(?c.id) WHERE { ?c CONCEPT {} } LIMIT 20"#,
            "LIST PREDICATES LIMIT 500",
            // The custodial work the reference policy asks maintenance for.
            r#"UPDATE ?c SET FACET "MnemonicState" {salience: 0.9} WHERE { ?c CONCEPT {} } LIMIT 20"#,
            r#"TRANSITION ?c TO "archived" WHERE { ?c CONCEPT {} } LIMIT 20"#,
            r#"TRANSITION ?c TO "tombstoned" WHERE { ?c CONCEPT {} } LIMIT 20"#,
            // Verbatim from the deployment contract: `STRUCTURAL` over a Core
            // reference field is how §16 and §26 find what a corrected
            // observation was resting under.
            r#"TRANSITION ?a TO "archived" WHERE { STRUCTURAL (?a, "evidence", :e) } LIMIT 20"#,
            // Retention without a hold: §20's judgement, which is a model's.
            r#"SET RETENTION ?e { retention_class: "standard", expires_at: "2027-01-01T00:00:00.000Z" } WHERE { ?e EVIDENCE {} } LIMIT 20"#,
            // Naming the target outright reaches one element by construction,
            // so there is nothing for a bound to do.
            r#"TRANSITION "C-7" TO "archived""#,
            // No LIMIT slot, and none needed: both engines refuse an operand
            // that binds more than one Concept.
            r#"MERGE CONCEPT ?dup INTO ?canonical WHERE { ?dup CONCEPT {key: "alice-2"} ?canonical CONCEPT {key: "alice"} }"#,
        ] {
            assert_eq!(
                maintenance_refusal(&parsed(command), None),
                None,
                "should be admitted: {command}"
            );
        }
    }

    #[test]
    fn the_maintenance_gate_refuses_erasure_and_holds() {
        for (command, expected) in [
            // Erasure is a person's decision, run deterministically.
            (
                r#"PURGE ?c WHERE { ?c CONCEPT {} } LIMIT 5 CONFIRM "PURGE""#,
                "PURGE",
            ),
            (r#"PURGE :victim CONFIRM "PURGE""#, "PURGE"),
            (r#"PURGE PAYLOAD :e CONFIRM "PURGE""#, "PURGE PAYLOAD"),
            // Placing a hold and lifting one are the same gate: `SET
            // RETENTION` replaces the block rather than patching it.
            (
                r#"SET RETENTION :e { retention_class: "standard", legal_hold: true }"#,
                "legal hold",
            ),
            (
                r#"SET RETENTION :e { retention_class: "standard", legal_hold: false }"#,
                "legal hold",
            ),
            // The member name is fixed by the grammar, so a parameterised
            // value never reaches evaluation.
            (
                r#"SET RETENTION :e { legal_hold: :whatever }"#,
                "legal hold",
            ),
            // One hazard, several verbs: the bound is on the selection.
            (
                r#"TRANSITION ?c TO "archived" WHERE { ?c CONCEPT {} }"#,
                "LIMIT 20 or less",
            ),
            (
                r#"UPDATE ?c SET FACET "MnemonicState" {salience: 0.9} WHERE { ?c CONCEPT {} } LIMIT 500"#,
                "LIMIT 20 or less",
            ),
        ] {
            let refusal = maintenance_refusal(&parsed(command), None)
                .unwrap_or_else(|| panic!("should be refused: {command}"));
            assert!(refusal.contains(expected), "{command}: {refusal}");
        }
    }

    /// The two gates are one policy read from two sides, so a verb the wider
    /// one holds back must not be reachable through the narrower one either.
    #[test]
    fn nothing_formation_may_write_is_something_maintenance_may_not() {
        for command in [
            r#"MUTATE { ASSERT (:alice, "prefers", :dark) { by: :alice, mode: "stated" } }"#,
            r#"TRANSITION :old TO "corrected" BY :new"#,
            r#"TRANSITION ?a TO "retracted" WHERE { ?a ASSERTION {asserted_by: :alice} } LIMIT 5"#,
        ] {
            let command = parsed(command);
            assert_eq!(cognition_refusal(&command, None), None);
            assert_eq!(maintenance_refusal(&command, None), None);
        }
    }

    fn said(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![text.to_string().into()],
            ..Default::default()
        }
    }

    #[test]
    fn source_timestamps_are_canonicalized_or_refused() {
        assert_eq!(
            source_timestamp("2026-09-22T08:00:00+08:00").unwrap(),
            "2026-09-22T00:00:00.000Z"
        );
        assert_eq!(
            source_timestamp("2026-09-22T00:00:00.5Z").unwrap(),
            "2026-09-22T00:00:00.500Z"
        );
        assert_eq!(
            source_timestamp("9999-12-31T22:59:59.999-01:00").unwrap(),
            "9999-12-31T23:59:59.999Z"
        );
        for invalid in [
            "yesterday",
            "2026-09-22T00:00:00.000123Z",
            "2026-09-22T00:00:00Z[UTC]",
            "2026-09-22T00:00Z",
            "2026-09-22T00:00:00+25:00",
            "2016-12-31T23:59:60Z",
        ] {
            assert!(source_timestamp(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn stored_observation_timestamps_truncate_or_use_receipt_time() {
        assert_eq!(
            observation_timestamp(Some(" 1970-01-01 01:00:00.123456789123+01:00 "), 42),
            "1970-01-01T00:00:00.123Z"
        );
        assert_eq!(
            observation_timestamp(Some("2016-12-31T23:59:60Z"), 42),
            "1970-01-01T00:00:00.042Z"
        );
    }

    #[test]
    fn stored_instants_convert_to_unix_milliseconds() {
        assert_eq!(time_ms("1969-12-31T23:59:59.999999999Z"), None);
        assert_eq!(time_ms("1969-12-31T23:59:60.123Z"), Some(123));
        assert_eq!(time_ms("1970-01-01t01:00:00.123456789123+01:00"), Some(123));
        assert_eq!(
            time_ms("9999-12-31T23:59:59.999-01:00"),
            Some(253_402_304_399_999)
        );
    }

    #[test]
    fn a_message_timestamp_is_its_own_observation_time() {
        let mut timed = said("user", "I moved to Berlin.");
        timed.timestamp = Some(1_758_499_200_000);
        let ingest = observation_ingest(
            &[said("user", "hi"), timed],
            "2026-09-24T00:00:00.000Z",
            "o",
            None,
        )
        .unwrap();
        let times: Vec<_> = ingest
            .evidence
            .iter()
            .map(|e| e.observed_at.as_deref().unwrap())
            .collect();
        assert_eq!(
            times,
            ["2026-09-24T00:00:00.000Z", "2025-09-22T00:00:00.000Z"]
        );
        let manifest = observation_manifest(&ingest);
        assert!(manifest.contains(":msg2 user_statement observed_at 2025-09-22T00:00:00.000Z"));
    }

    #[test]
    fn each_message_is_its_own_observation() {
        let messages = [
            said("user", "I always prefer dark mode."),
            said("assistant", "Noted."),
        ];
        let ingest = observation_ingest(
            &messages,
            "2026-08-20T00:00:00.000Z",
            "formation:chat-42",
            None,
        )
        .expect("two messages produce two entries");

        let keys: Vec<_> = ingest.evidence.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["msg1", "msg2"]);
        // The class comes from who was speaking, which is a fact about the
        // observation rather than a judgement about it.
        let classes: Vec<_> = ingest
            .evidence
            .iter()
            .map(|e| e.evidence_class.as_str())
            .collect();
        assert_eq!(classes, ["user_statement", "agent_statement"]);
        // Whole messages, not extracted text: who said a thing is part of what
        // was observed.
        assert_eq!(
            ingest.evidence[0].payload,
            Some(serde_json::to_value(&messages[0]).unwrap())
        );
        let keys: Vec<_> = ingest
            .evidence
            .iter()
            .map(|e| e.client_key.as_deref().unwrap())
            .collect();
        assert_eq!(keys, ["formation:chat-42:1", "formation:chat-42:2"]);

        assert!(observation_ingest(&[], "2026-08-20T00:00:00.000Z", "x", None).is_none());
    }

    #[test]
    fn only_the_newest_messages_are_minted() {
        // A long transcript still produces a bounded envelope, and the window
        // keeps the end of the conversation — which is what a formation pass
        // writes about.
        let messages: Vec<_> = (0..MAX_INGESTED_MESSAGES + 4)
            .map(|n| said("user", &format!("turn {n}")))
            .collect();
        let ingest = observation_ingest(&messages, "2026-08-20T00:00:00.000Z", "o", None).unwrap();

        assert_eq!(ingest.evidence.len(), MAX_INGESTED_MESSAGES);
        assert_eq!(ingest.evidence[0].key, "msg1");
        assert_eq!(
            ingest.evidence[0].payload,
            Some(serde_json::to_value(said("user", "turn 4")).unwrap())
        );
    }

    #[test]
    fn a_source_is_named_only_for_what_the_counterparty_said() {
        let messages = [said("user", "hi"), said("assistant", "hello")];
        let ingest =
            observation_ingest(&messages, "2026-08-20T00:00:00.000Z", "o", Some("C-7")).unwrap();

        // The assistant's turn is not the counterparty's, and an Evidence
        // source that pointed at them anyway would say they said it.
        assert_eq!(
            ingest.evidence[0].source_actor,
            Some(ElementReference::by_id("C-7"))
        );
        assert_eq!(ingest.evidence[1].source_actor, None);
    }

    #[test]
    fn a_model_cannot_shadow_captured_source_handles() {
        let observation =
            observation_ingest(&[said("user", "hi")], "2026-08-20T00:00:00.000Z", "o", None)
                .unwrap();

        let mut plain = request("MUTATE { CREATE ACTIVITY ?a { SET FIELDS {} } }");
        attach_observation(&mut plain, &observation).unwrap();
        assert!(plain.ingest.is_some(), "the ordinary case attaches");

        // §74 merges request- and operation-level parameters into one
        // environment, so either level claiming `msg1` would make `:msg1`
        // ambiguous and the engine would refuse the whole request. A model
        // writing Evidence the long way gets to.
        let mut claimed = request_with("MUTATE { CREATE ACTIVITY ?a { SET FIELDS {} } }", {
            param("msg1", "mine")
        });
        assert!(attach_observation(&mut claimed, &observation).is_err());
        assert!(claimed.ingest.is_none());

        let mut per_operation = request("MUTATE { CREATE ACTIVITY ?a { SET FIELDS {} } }");
        per_operation.operations[0].parameters = Some(param("msg1", "mine"));
        assert!(attach_observation(&mut per_operation, &observation).is_err());
        assert!(per_operation.ingest.is_none());
    }

    #[test]
    fn observation_ingest_only_attaches_to_requests_containing_a_parsed_mutation() {
        let observation =
            observation_ingest(&[said("user", "hi")], "2026-08-20T00:00:00.000Z", "o", None)
                .unwrap();
        for command in [
            "DESCRIBE PRIMER",
            r#"FIND(?e) WHERE { ?e EVIDENCE {id: "E-1"} } LIMIT 1"#,
        ] {
            let mut read = request(command);
            attach_observation(&mut read, &observation).unwrap();
            assert!(read.ingest.is_none());
            read.validate().unwrap();
        }

        let mut mixed = request("DESCRIBE PRIMER");
        mixed
            .operations
            .extend(request("CREATE ACTIVITY ?a { SET FIELDS {} }").operations);
        mixed.execution = Some(anda_kip::Execution::new(anda_kip::ExecutionMode::Sequence));
        attach_observation(&mut mixed, &observation).unwrap();
        assert!(mixed.ingest.is_some());
        mixed.validate().unwrap();

        // Two writes are two transactions, and both mint the observation. Its
        // client_key makes the second resolve the first one's Evidence; KIP
        // refuses the batch without it rather than recording one message twice
        // (Spec §71.1).
        let mut writes = request("CREATE ACTIVITY ?a { SET FIELDS {} }");
        writes
            .operations
            .extend(request("CREATE ACTIVITY ?b { SET FIELDS {} }").operations);
        writes.execution = Some(anda_kip::Execution::new(anda_kip::ExecutionMode::Sequence));
        attach_observation(&mut writes, &observation).unwrap();
        writes
            .validate()
            .expect("every captured message carries a client_key");
        for entry in &mut writes.ingest.as_mut().unwrap().evidence {
            entry.client_key = None;
        }
        assert_eq!(
            writes.validate().unwrap_err().code,
            anda_kip::KipErrorCode::InvalidRequestEnvelope
        );

        let mut invalid = request("not a KIP command");
        attach_observation(&mut invalid, &observation).unwrap();
        assert!(
            invalid.parse_operations().is_err(),
            "syntax errors stay visible"
        );
    }

    #[test]
    fn formation_defines_literal_symbols_in_their_own_request() {
        let admitted = [
            r#"DEFINE PREDICATE "mentors" {description: "The subject mentors the object.", subject: {concept_types: ["Person"]}}"#,
            r#"DEFINE CONCEPT TYPE "Instrument" {description: "A musical instrument."}"#,
        ];
        for command in admitted {
            assert_eq!(cognition_refusal(&parsed(command), None), None, "{command}");
        }
        let alone: Vec<Command> = admitted.iter().map(|c| parsed(c)).collect();
        assert_eq!(define_batch_refusal(&alone), None);
        // A DEFINE commits on its own, before the MUTATE that uses it.
        let mixed = vec![
            parsed(admitted[0]),
            parsed(r#"MUTATE { ASSERT (:a, "mentors", :b) { by: :a, mode: "stated" } }"#),
        ];
        assert!(define_batch_refusal(&mixed).is_some());
        let many = vec![parsed(admitted[1]); crate::vocabulary::MAX_DEFINES_PER_REQUEST + 1];
        assert!(define_batch_refusal(&many).is_some());
        assert_eq!(define_batch_refusal(&[parsed("DESCRIBE PRIMER")]), None);

        for command in [
            // Names keep this brain's shapes; the body and name are literals.
            r#"DEFINE CONCEPT TYPE "instrument" {description: "x"}"#,
            r#"DEFINE PREDICATE "Mentors" {description: "x"}"#,
            r#"DEFINE PREDICATE :name {description: "x"}"#,
            r#"DEFINE PREDICATE "mentors" {description: :description}"#,
        ] {
            assert!(
                cognition_refusal(&parsed(command), None).is_some(),
                "should be refused: {command}"
            );
        }
        // Maintenance reviews drafts; it never defines one.
        assert!(maintenance_refusal(&parsed(admitted[0]), None).is_some());
        // A DEFINE mints nothing, so it carries no observation and no bindings.
        let observation =
            observation_ingest(&[said("user", "hi")], "2026-08-20T00:00:00.000Z", "o", None)
                .unwrap();
        let mut define = request(admitted[1]);
        attach_observation(&mut define, &observation).unwrap();
        attach_host_bindings(&mut define, 1_000).unwrap();
        assert!(define.ingest.is_none() && define.parameters.is_none());
    }

    #[test]
    fn a_claim_from_a_captured_message_carries_its_time() {
        let captured = ["msg1", "msg2"];
        // Undated: the claim would start at the write's time.
        for command in [
            r#"ASSERT (:a, "prefers", :b) { by: :a, mode: "stated", evidence: :msg1 }"#,
            r#"MUTATE { ASSERT (:a, "prefers", :b) { by: :a, mode: "stated", evidence: [:x, :msg2] } }"#,
            r#"CREATE ASSERTION ?c { SET FIELDS { proposition: :p, asserted_by: :a, stance: "support" } SET STRUCTURAL { ("evidence", :msg1) } }"#,
        ] {
            assert!(
                undated_citation(&parsed(command), &captured).is_some(),
                "should be refused: {command}"
            );
        }
        for command in [
            // Dated by `at`, or by a written start.
            r#"ASSERT (:a, "prefers", :b) { by: :a, mode: "stated", evidence: :msg1, at: "2026-09-22T00:00:00.000Z" }"#,
            r#"ASSERT (:a, "prefers", :b) { by: :a, mode: "stated", evidence: :msg1, at: :observed_at }"#,
            r#"ASSERT (:a, "works_for", :b) { by: :a, mode: "stated", evidence: :msg1, valid: {from: "2026-01-01T00:00:00.000Z"} }"#,
            // The Brain's own inference is made when it is written.
            r#"ASSERT (:a, "prefers", :b) { by: :self, mode: "inferred", evidence: :msg1 }"#,
            // Evidence this request did not capture is not a message's time.
            r#"ASSERT (:a, "prefers", :b) { by: :a, mode: "stated", evidence: :e }"#,
            r#"ASSERT (:a, "prefers", :b) { by: :a, mode: "stated" }"#,
        ] {
            assert_eq!(
                undated_citation(&parsed(command), &captured),
                None,
                "should be admitted: {command}"
            );
        }
    }

    #[test]
    fn a_strength_base_is_written_with_its_anchor_and_policy() {
        for command in [
            r#"CREATE CONCEPT ?c { TYPE "Event" SET FACET "MnemonicState" { memory_strength: 0.7 } }"#,
            r#"UPSERT CONCEPT ?c { MATCH {type: "Person", key: "a"} SET FACET "MnemonicState" { memory_strength: 0.7, last_metabolized_at: :now } }"#,
            r#"UPDATE :c SET FACET "MnemonicState" { memory_strength: 0.9, strength_policy: :strength_policy }"#,
        ] {
            assert!(
                incomplete_strength(&parsed(command), None).is_some(),
                "should be refused: {command}"
            );
        }
        for command in [
            r#"CREATE CONCEPT ?c { TYPE "Event" SET FACET "MnemonicState" { memory_strength: 0.7, last_metabolized_at: :now, strength_policy: :strength_policy } }"#,
            // Salience and utility are not strength.
            r#"UPDATE :c SET FACET "MnemonicState" { salience: 0.4 }"#,
            r#"UPDATE :c SET FACET "MnemonicState" { utility: 0.4 }"#,
        ] {
            assert_eq!(
                incomplete_strength(&parsed(command), None),
                None,
                "should be admitted: {command}"
            );
        }
    }

    #[test]
    fn the_host_binds_its_strength_pin_and_write_time_on_writes_only() {
        let mut read = request("DESCRIBE PRIMER");
        attach_host_bindings(&mut read, 1_000).unwrap();
        assert!(read.parameters.is_none());

        let mut write = request(r#"UPDATE :c SET FACET "MnemonicState" { salience: 0.4 }"#);
        attach_host_bindings(&mut write, 1_780_000_000_000).unwrap();
        let parameters = write.parameters.as_ref().unwrap();
        assert_eq!(
            parameters[HOST_STRENGTH_POLICY],
            json!({
                "artifact_ref": "kip:strength-half-life-30d",
                "content_digest": "sha256:a50a89b83f937c97cabf0f8371cccfd326f4fdd438b6d7b9ead507d77927b227",
            })
        );
        assert_eq!(parameters[HOST_NOW], json!(timestamp(1_780_000_000_000)));
        write.validate().unwrap();

        // A model's own `now` is left alone; its own pin is refused.
        let mut own_now = request_with(
            r#"UPDATE :c SET FACET "MnemonicState" { salience: 0.4 }"#,
            param(HOST_NOW, "2026-01-01T00:00:00.000Z"),
        );
        attach_host_bindings(&mut own_now, 1_000).unwrap();
        assert_eq!(
            own_now.parameters.as_ref().unwrap()[HOST_NOW],
            "2026-01-01T00:00:00.000Z"
        );
        let mut own_pin = request_with(
            r#"UPDATE :c SET FACET "MnemonicState" { salience: 0.4 }"#,
            param(HOST_STRENGTH_POLICY, json!({"artifact_ref": "mine"})),
        );
        assert!(attach_host_bindings(&mut own_pin, 1_000).is_err());
    }

    #[test]
    fn model_learning_and_runtime_facets_are_rejected_by_ast_position() {
        for facet in PROTECTED_FACETS {
            for command in [
                format!(r#"CREATE ACTIVITY ?a {{ SET FACET "{facet}" {{ x:1 }} }}"#),
                format!(
                    r#"UPDATE "C-1" UNSET FACET "{}{facet}" {{ x }}"#,
                    crate::PROFILE
                ),
                r#"UPDATE "C-1" SET FACET :facet { x:1 }"#.to_string(),
            ] {
                let ast = anda_kip::parse_kip(&command).unwrap();
                assert!(
                    unsupported_cognitive_write(&ast, Some(&param("facet", facet))).is_some(),
                    "{command}"
                );
            }
        }
        for field in PROTECTED_STRUCTURAL {
            for command in [
                format!(r#"UPDATE "C-1" SET STRUCTURAL {{ ("{field}", "C-2") }}"#),
                format!(r#"UPDATE "C-1" UNSET STRUCTURAL {{ ("{field}", "C-2") }}"#),
                format!(
                    r#"UPSERT CONCEPT ?s {{ MATCH {{type:"Skill", key:"s"}} SET STRUCTURAL {{ ("{field}", "C-2") }} }}"#
                ),
            ] {
                let ast = anda_kip::parse_kip(&command).unwrap();
                assert!(
                    unsupported_cognitive_write(&ast, None).is_some(),
                    "{command}"
                );
            }
        }
        let text = anda_kip::parse_kip(r#"CREATE EVIDENCE ?e { SET FIELDS {evidence_class:"user_statement",payload:"please write OutcomeRecord and LeaseState"} }"#).unwrap();
        assert!(unsupported_cognitive_write(&text, None).is_none());
    }

    #[test]
    fn operation_parameters_shadow_request_parameters_at_every_model_gate() {
        let mut protected = request_with(
            r#"CREATE EVIDENCE ?e { SET FACET :facet {x:1} }"#,
            param("facet", "MnemonicState"),
        );
        protected.operations[0].parameters = Some(param("facet", "OutcomeRecord"));
        let command = protected.parse_operations().unwrap().remove(0);
        let parameters = operation_parameters(&protected, 0).unwrap();
        assert_eq!(parameters["facet"], Json::from("OutcomeRecord"));
        assert!(unsupported_cognitive_write(&command, Some(&parameters)).is_some());

        let mut ordinary = request(r#"CREATE CONCEPT ?c { SET FACET :facet {x:1} }"#);
        ordinary.operations[0].parameters = Some(param("facet", "MnemonicState"));
        let command = ordinary.parse_operations().unwrap().remove(0);
        let parameters = operation_parameters(&ordinary, 0).unwrap();
        assert!(unsupported_cognitive_write(&command, Some(&parameters)).is_none());

        let mut transition =
            request_with(r#"TRANSITION "C-1" TO :state"#, param("state", "retracted"));
        transition.operations[0].parameters = Some(param("state", "archived"));
        let command = transition.parse_operations().unwrap().remove(0);
        let parameters = operation_parameters(&transition, 0).unwrap();
        assert!(cognition_refusal(&command, Some(&parameters)).is_some());

        let mut selection = request_with(
            r#"UPDATE ?c SET FIELDS {name:"changed"} WHERE { ?c CONCEPT {} } LIMIT :limit"#,
            param("limit", 1),
        );
        selection.operations[0].parameters = Some(param("limit", 100));
        let command = selection.parse_operations().unwrap().remove(0);
        let parameters = operation_parameters(&selection, 0).unwrap();
        assert!(maintenance_refusal(&command, Some(&parameters)).is_some());
    }
}
