//! Explicit budget mode: the model selects authorized items; the host owns
//! reads, priority, current procedure checks and both serialization boundaries.
//! Each planning pass is a fresh request with the admitted memory snapshot.
//! Opaque provider history and a generic runner's automatic tool loop never
//! carry unbudgeted material into another model invocation.
use super::*;
use crate::recall_budget::{self as budget, Channel, Coverage, MemoryItem, Priority, RecallBudget};
use anda_core::{ContentPart, ToolInput, Usage};
use anda_kip::{Command, KipValue, MetaCommand, Request, Scalar};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

mod compact;

const MAX_CALLS: usize = 16;
const MAX_CALLS_PER_TURN: usize = 4;
const MAX_ROWS: usize = 32;
const MAX_BYTES: usize = 1024 * 1024;
const SELECT: &str = "select_recall_items";
/// The host's instructions for one budgeted planning pass.
const BUDGET_MODE: &str = "You are selecting an authorized memory packet for another agent. \
    Read only through the listed tools, then call select_recall_items with existing memory item \
    IDs. Initial query_search IDs are question-matched candidates, not proof of relevance. Select \
    items that answer the question; use further reads when these candidates and the profile do \
    not cover it. Compact views explicitly name omitted fields; project the original attributes \
    or facet through KQL for details. The KIP tool supports only KQL and SEARCH in this mode (up \
    to 4 operations, LIMIT at most 32); other META commands are unavailable and the current \
    Primer is already supplied. Wiki search uses at most 8 hits without neighbor expansion. At \
    most 4 tool calls per pass and 16 per Recall are allowed. You cannot add content or choose \
    priorities. No free-form answer, grades or completeness claims will be delivered. Preserve \
    native uncertainty, conflicts and warnings. Unchecked channels are unknown, not absent. Each \
    pass receives the entire currently admitted snapshot; previous provider history is \
    intentionally not replayed.";
/// Question-matched Concepts the host reads before any model pass.
const DISCOVERY_HITS: usize = 8;
/// What memory holds about one discovered Concept: the claims it takes part in, in
/// either direction, newest first, each with its final belief. A Concept record only
/// names an entity; the facts about it are Propositions and Assertions, which a
/// concept search never returns and which a pass that ran out of planning budget
/// never gets to read.
const CLAIMS: &str = r#"FIND(?p.id, ?p.subject, ?subject.name, ?predicate, ?p.object, ?object.name,
     ?belief.status, ?belief.explanation.excluded,
     ?a.id, ?a.stance, ?a.mode, ?a.asserted_at, ?a.asserted_by, ?actor.name)
WHERE {
  ?p (:entity, ?predicate, ?object)
  UNION { ?p (?subject, ?predicate, :entity) }
  ?a ASSERTION {proposition: ?p, asserted_by: ?actor}
  ?belief BELIEF (?p)
}
WITH EPISTEMIC {purpose: "answer_user", risk: "low", policy: "kip:memory-default"}
ORDER BY ?a.asserted_at DESC
LIMIT 6"#;
const ALL_CHANNELS: [Channel; 7] = [
    Channel::Primer,
    Channel::Notes,
    Channel::Counterparty,
    Channel::History,
    Channel::Kip,
    Channel::Wiki,
    Channel::Procedures,
];

#[derive(Default)]
struct Material {
    next_id: usize,
    failure: Option<&'static str>,
    items: Vec<MemoryItem>,
    coverage: Coverage,
    critical_missing: bool,
    skills: BTreeSet<String>,
    /// Slot -> exact Skill reference, supplied by the actual procedure tool.
    procedures: BTreeMap<String, String>,
}
fn mark(coverage: &mut Coverage, channel: Channel, partial: bool) {
    if !coverage.queried.contains(&channel) {
        coverage.queried.push(channel);
    }
    if partial && !coverage.partial.contains(&channel) {
        coverage.partial.push(channel);
    }
}
fn omit(coverage: &mut Coverage, channel: Channel) {
    mark(coverage, channel, true);
    if !coverage.omitted.contains(&channel) {
        coverage.omitted.push(channel);
    }
}
/// A coverage as delivered: every channel nothing queried is unchecked.
fn with_unchecked(mut coverage: Coverage) -> Coverage {
    coverage.unchecked = ALL_CHANNELS
        .into_iter()
        .filter(|c| !coverage.queried.contains(c))
        .collect();
    coverage
}
impl Material {
    fn mark(&mut self, channel: Channel, partial: bool) {
        mark(&mut self.coverage, channel, partial);
    }
    fn omit(&mut self, channel: Channel) {
        omit(&mut self.coverage, channel);
    }
    fn add(
        &mut self,
        channel: Channel,
        priority: Priority,
        mut content: Json,
    ) -> Result<Option<String>, BoxError> {
        self.mark(channel, false);
        if content.is_null()
            || content.as_array().is_some_and(Vec::is_empty)
            || content.as_object().is_some_and(|v| v.is_empty())
        {
            return Ok(None);
        }
        if matches!(channel, Channel::Kip | Channel::Counterparty)
            && compact::memory(&mut content, priority <= Priority::Warning)
        {
            self.mark(channel, true);
        }
        if self.items.len() >= budget::MAX_ITEMS || serde_json::to_vec(&content)?.len() > MAX_BYTES
        {
            self.omit(channel);
            self.critical_missing |= matches!(priority, Priority::Required | Priority::Warning);
            return Ok(None);
        }
        // Lexical packet ordering must preserve admission/search rank past m009.
        let id = format!("m{:03}", self.next_id);
        self.next_id += 1;
        self.items.push(MemoryItem {
            id: id.clone(),
            channel,
            priority,
            content,
        });
        Ok(Some(id))
    }
    fn coverage(&self) -> Coverage {
        with_unchecked(self.coverage.clone())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    selected_ids: Vec<String>,
}

fn selector() -> FunctionDefinition {
    serde_json::from_value(json!({
        "name": SELECT,
        "description": "Finish Recall by selecting existing memory item IDs. The host retains \
            all required constraints and warnings regardless of this selection. Do not generate \
            new content or invent IDs.",
        "strict": true,
        "parameters": {
            "type": "object",
            "properties": {"selected_ids": {"type": "array", "items": {"type": "string"}}},
            "required": ["selected_ids"],
            "additionalProperties": false,
        },
    }))
    .unwrap()
}

impl RecallAgent {
    pub(super) async fn run_budgeted(
        &self,
        ctx: AgentCtx,
        prompt: String,
        parsed: Option<RecallInput>,
        limits: RecallBudget,
    ) -> Result<AgentOutput, BoxError> {
        limits.validate()?;
        // A clone with its own extension map, even when a host reuses ctx.
        let mut ctx = ctx;
        ctx.base = ctx.base.with_caller(*ctx.caller());
        let now = unix_ms();
        let query = parsed
            .as_ref()
            .map(|i| i.query.clone())
            .unwrap_or_else(|| prompt.clone());
        if query.trim().is_empty() {
            return Err("recall query must not be empty".into());
        }
        if prompt.len() > MAX_BYTES {
            return Err("Recall input exceeds bounded byte limit".into());
        }
        // Persist the limits this run actually resolved, rather than the
        // caller's possibly looser request. Space reads this canonical input
        // back when it builds the structured budget receipt.
        let persisted_prompt = serde_json::to_string(&RecallInput {
            query: query.clone(),
            context: parsed.as_ref().and_then(|input| input.context.clone()),
            budget: Some(limits.clone()),
        })?;
        let mut conversation = Conversation {
            user: *ctx.caller(),
            messages: vec![json!(Message {
                role: "user".into(),
                content: vec![persisted_prompt.into()],
                timestamp: Some(now),
                ..Default::default()
            })],
            status: ConversationStatus::Working,
            period: now / 3_600_000,
            created_at: now,
            updated_at: now,
            label: Some("recall".into()),
            ..Default::default()
        };
        conversation._id = self
            .conversations
            .add_conversation(ConversationRef::from(&conversation))
            .await?;
        let mut usage = Usage::default();
        let mut tool_usage = HashMap::new();
        let result = self
            .plan_budgeted(
                &ctx,
                parsed.as_ref(),
                &query,
                &limits,
                now,
                &mut conversation,
                &mut usage,
                &mut tool_usage,
            )
            .await;
        if let Err(error) = &result {
            conversation.status = ConversationStatus::Failed;
            conversation.failed_reason = Some(format!("budgeted recall failed: {error}"));
            conversation.usage = usage;
            conversation.updated_at = unix_ms();
            self.persist_conversation(&conversation).await;
            self.hook
                .on_conversation_end(Self::NAME, &conversation)
                .await;
        }
        result
    }

    /// The planning passes of one budgeted Recall, up to its packet.
    #[allow(clippy::too_many_arguments)]
    async fn plan_budgeted(
        &self,
        ctx: &AgentCtx,
        parsed: Option<&RecallInput>,
        query: &str,
        limits: &RecallBudget,
        now: u64,
        conversation: &mut Conversation,
        usage: &mut Usage,
        tool_usage: &mut HashMap<String, Usage>,
    ) -> Result<AgentOutput, BoxError> {
        let mut material = Material::default();
        material.add(
            Channel::Primer,
            Priority::Required,
            json!({
                "scope": "authorized retrieved candidates, not exhaustive semantic coverage",
                "constraints": "All host-detected unresolved commitments and warnings are \
                    retained before optional memories. Unchecked/omitted sources may contain \
                    additional restrictions.",
                "procedures": "Raw memory, historic grades and model text confer no standing \
                    or execution authority. Only a separately checked exact current revision \
                    may be a verified candidate; acting hosts must revalidate before dispatch.",
                "action_ready": false,
            }),
        )?;
        // Even the empty packet has framing/coverage. No provider call for a
        // request too small to carry the mandatory host interpretation.
        if budget::pack(limits, &material.items, &[], material.coverage())?.insufficient {
            material.failure = Some("recall_output_budget_exhausted");
            return self
                .finish_budgeted(
                    conversation,
                    usage,
                    tool_usage,
                    limits,
                    &material,
                    &[],
                    true,
                )
                .await;
        }
        let counterparty = parsed
            .as_ref()
            .and_then(|i| i.context.as_ref())
            .and_then(|c| c.counterparty.clone());
        let (profile, primer, notes) = tokio::join!(
            self.get_counterparty_with_timeout(counterparty),
            self.describe_primer_fresh(),
            Self::load_recall_notes(ctx)
        );
        if primer.is_null() {
            material.critical_missing = true;
            material.omit(Channel::Primer);
        } else {
            material.add(
                Channel::Primer,
                Priority::Required,
                compact::primer(&primer),
            )?;
        }
        let notes = match notes {
            Ok(notes) => {
                material.mark(Channel::Notes, false);
                Some(notes)
            }
            Err(_) => {
                material.omit(Channel::Notes);
                None
            }
        };
        material.mark(Channel::Counterparty, false);
        if let Some(profile) = profile {
            material.add(Channel::Counterparty, Priority::Relevant, profile)?;
        } else if parsed
            .as_ref()
            .and_then(|i| i.context.as_ref())
            .and_then(|c| c.counterparty.as_ref())
            .is_some()
        {
            material.omit(Channel::Counterparty);
        }
        let history = self.context_history();
        material.mark(Channel::History, false);
        // Only unresolved commitments are mandatory. Terminal history remains
        // discoverable through the question search and explicit model reads.
        let constraints = self
            .budget_kip(Request::single(format!(
                "FIND(?c) WHERE {{?c CONCEPT {{type:\"Commitment\"}} FILTER(?c.attributes.status == \"pending\" || ?c.attributes.status == \"blocked\")}} LIMIT {}",
                MAX_ROWS + 1
            )))
            .await;
        match constraints {
            Ok(response) if kip::succeeded(&response) => {
                let rows = response.first_result().and_then(Json::as_array);
                if let Some(rows) = rows {
                    material.mark(Channel::Kip, rows.len() > MAX_ROWS);
                    if rows.len() > MAX_ROWS {
                        material.critical_missing = true;
                    }
                    for row in rows.iter().take(MAX_ROWS) {
                        material.add(Channel::Kip, Priority::Required, row.clone())?;
                    }
                } else {
                    material.critical_missing = true;
                    material.omit(Channel::Kip);
                }
            }
            _ => {
                material.critical_missing = true;
                material.omit(Channel::Kip);
            }
        }
        if material.critical_missing
            || budget::pack(limits, &material.items, &[], material.coverage())?.insufficient
        {
            material.failure = Some(if material.critical_missing {
                "recall_required_read_incomplete"
            } else {
                "recall_output_budget_exhausted"
            });
            return self
                .finish_budgeted(
                    conversation,
                    usage,
                    tool_usage,
                    limits,
                    &material,
                    &[],
                    true,
                )
                .await;
        }
        // Ground every selection in the actual question, even when the model
        // can finish in one pass. Each hit is a separate optional item so a
        // small packet can keep some useful results without the whole window.
        let mut observations = vec![json!({"primer":primer})];
        let discovery = self
            .budget_kip(kip::request_with(
                "SEARCH CONCEPT :query LIMIT 16",
                kip::param("query", query),
            ))
            .await;
        material.mark(Channel::Kip, true);
        let mut discovered = Vec::new();
        match discovery {
            Ok(response) if kip::succeeded(&response) => {
                if let Some(hits) = response
                    .first_result()
                    .and_then(|value| value["hits"].as_array())
                {
                    let mut seen_claims = BTreeSet::new();
                    // A SleepTask is the Brain's own queued work, such as the review
                    // of a draft symbol whose description repeats the question's
                    // words. It is not memory about the question.
                    for hit in hits
                        .iter()
                        .filter(|hit| !is_sleep_task(&hit["element"]))
                        .take(DISCOVERY_HITS)
                    {
                        let element = &hit["element"];
                        discover_skills(element, &mut material.skills);
                        for claim in self.discovered_claims(element, &mut seen_claims).await {
                            if let Some(id) =
                                material.add(Channel::Kip, Priority::Relevant, claim)?
                            {
                                discovered.push(id);
                            }
                        }
                        if let Some(existing) = material.items.iter().find(|item| {
                            element["id"].is_string() && item.content["id"] == element["id"]
                        }) {
                            discovered.push(existing.id.clone());
                            continue;
                        }
                        let priority = if contains_program(element) {
                            Priority::UnprovenProcedure
                        } else {
                            Priority::Relevant
                        };
                        if let Some(id) = material.add(Channel::Kip, priority, element.clone())? {
                            discovered.push(id);
                        }
                    }
                } else {
                    material.omit(Channel::Kip);
                }
            }
            _ => {
                material.omit(Channel::Kip);
                material.add(
                    Channel::Kip,
                    Priority::Warning,
                    json!({"query_search":"unavailable","meaning":"not evidence of absence"}),
                )?;
            }
        }
        observations.push(json!({"query_search":{"item_ids":discovered,"partial":true}}));
        conversation.messages.push(json!(Message {
            role: "tool".into(),
            content: vec![ContentPart::ToolOutput {
                name: "recall_query_discovery".into(),
                output: json!({"query":query,"item_ids":discovered,"partial":true}),
                is_error: None,
                call_id: None,
                remote_id: None,
            }],
            ..Default::default()
        }));
        // Prefer the current question's candidates over replayed narrative
        // when optional items compete for output or planner-input space.
        if let Some(notes) = notes {
            material.add(Channel::Notes, Priority::Relevant, notes)?;
        }
        material.add(Channel::History, Priority::Relevant, json!(history))?;
        let names = self.tool_dependencies();
        let mut tools = ctx.tool_definitions(Some(&names));
        tools.push(selector());
        let instructions = format!(
            "{}\n\n# Host budget mode\n{BUDGET_MODE}",
            super::super::prompts::system_prompt(
                super::super::prompts::PromptTarget::Recall,
                &self.prompt
            )
        );
        let template = CompletionRequest {
            instructions,
            tools,
            tool_choice_required: false,
            // The host bounds the delivered packet below. Keep provider
            // output defaults: some backends reject max_output_tokens entirely.
            effort: Some(ModelEffort::Medium),
            ..Default::default()
        };
        // Resolve once without invoking the generic runner's automatic tools.
        let model = ctx
            .clone()
            .completion_iter(template.clone(), vec![])
            .model()
            .clone();
        let mut calls = 0;
        let mut selected = Vec::new();
        let mut failed = false;
        let mut finished = false;
        let mut context_spent = 0usize;
        let mut context_fallback = None;
        for _ in 0..self.max_model_turns().min(8) {
            let mut request = template.clone();
            // Optional items can be evicted, never a required item/warning,
            // lowest value first.
            let visible = material.items.clone();
            let coverage_before = material.coverage.clone();
            let mut order: Vec<usize> = (0..visible.len())
                .filter(|&i| !matches!(visible[i].priority, Priority::Required | Priority::Warning))
                .collect();
            order.sort_by(|&a, &b| {
                (visible[b].priority, &visible[b].id).cmp(&(visible[a].priority, &visible[a].id))
            });
            let room = (limits.context_tokens as usize).saturating_sub(context_spent);
            let mut probe = template.clone();
            // The request that keeps the `kept` optional items evicted last.
            let fit = |kept: usize| -> Result<Option<(usize, String)>, BoxError> {
                let evicted = &order[..order.len() - kept];
                let mut coverage = coverage_before.clone();
                for &i in evicted {
                    omit(&mut coverage, visible[i].channel);
                }
                let items: Vec<&MemoryItem> = visible
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !evicted.contains(i))
                    .map(|(_, item)| item)
                    .collect();
                probe.prompt = json!({
                    "query": query,
                    "context": parsed.and_then(|i| i.context.as_ref()),
                    "memory_items": items,
                    "observations": observations,
                    "coverage": with_unchecked(coverage),
                })
                .to_string();
                let encoded = normalized_request(&probe)?;
                let tokens = if encoded.len() <= 4 * MAX_BYTES {
                    budget::count(&encoded)?
                } else {
                    usize::MAX
                };
                Ok((tokens <= room).then(|| (tokens, std::mem::take(&mut probe.prompt))))
            };
            let evicted = match budget::longest_fitting(0, order.len(), fit)? {
                Some((kept, (tokens, prompt))) => {
                    request.prompt = prompt;
                    context_spent += tokens;
                    order.len() - kept
                }
                None => {
                    failed = true;
                    material.failure = Some("recall_context_budget_exhausted");
                    order.len()
                }
            };
            for &i in &order[..evicted] {
                material.omit(visible[i].channel);
                // Also remove it from the final candidate pool: selection
                // cannot reference a record the planner was denied here.
                material.items.retain(|item| item.id != visible[i].id);
            }
            if failed {
                context_fallback = Some((visible, coverage_before));
            }
            if failed || material.critical_missing {
                failed = true;
                break;
            }
            let Some(remaining) = recall_time_remaining(now) else {
                failed = true;
                material.failure = Some("recall_deadline_reached");
                break;
            };
            let cancel = ctx.cancellation_token();
            let response = tokio::select! {
                _=cancel.cancelled()=>None,
                result=timeout(remaining,model.completion(request))=>match result {
                    Ok(Ok(output)) => Some(output),
                    Ok(Err(error)) => {
                        log::warn!(target: "brain", model = model.model_name(),
                            error = error.to_string().chars().take(512).collect::<String>();
                            "budgeted Recall model request failed");
                        None
                    }
                    Err(_) => None,
                },
            };
            let Some(output) = response else {
                failed = true;
                material.failure = Some("recall_model_unavailable");
                break;
            };
            usage.accumulate(&output.usage);
            // Provider-native thought/history/artifacts never become inputs or
            // outputs of this mode. Persist only bounded public tool receipts.
            if output.failed_reason.is_some() {
                failed = true;
                material.failure = Some("recall_model_unavailable");
                break;
            }
            if output.tool_calls.is_empty() {
                if output.content.len() <= 16_384
                    && let Ok(choice) = serde_json::from_str::<Selection>(&output.content)
                {
                    selected = choice.selected_ids;
                    finished = true;
                } else {
                    failed = true;
                }
                break;
            }
            if output.tool_calls.len() > MAX_CALLS_PER_TURN
                || calls + output.tool_calls.len() > MAX_CALLS
            {
                failed = true;
                break;
            }
            for tool in output.tool_calls {
                calls += 1;
                if serde_json::to_vec(&tool.args)?.len() > 16_384 {
                    failed = true;
                    break;
                }
                if tool.name == SELECT {
                    if let Ok(choice) = serde_json::from_value::<Selection>(tool.args) {
                        selected = choice.selected_ids;
                        finished = true;
                    } else {
                        failed = true;
                    }
                    break;
                }
                if !names.contains(&tool.name) {
                    failed = true;
                    break;
                }
                if tool.name == crate::kip_reference::KipReferenceTool::NAME {
                    // Protocol text is planning context, never a retrieved memory
                    // item or evidence that a memory channel was covered. These
                    // observations pass through the cumulative token admission
                    // check before the next model call, and count as tool calls.
                    let response = self
                        .budget_tool_before_deadline(ctx, &tool.name, tool.args, now)
                        .await;
                    let (value, is_error) = match response {
                        Ok((value, measured, is_error)) => {
                            usage.accumulate(&measured);
                            tool_usage
                                .entry(tool.name.clone())
                                .or_default()
                                .accumulate(&measured);
                            (value, is_error)
                        }
                        Err(_) => (
                            json!({
                                "status": "unavailable",
                                "hint": "Use document=index, or section=index for exact \
                                         headings, with offset=0.",
                            }),
                            true,
                        ),
                    };
                    observations
                        .push(json!({"tool":tool.name,"reference":value,"is_error":is_error}));
                    conversation.messages.push(json!(Message {
                        role: "tool".into(),
                        content: vec![ContentPart::ToolOutput {
                            name: tool.name,
                            output: value,
                            is_error: is_error.then_some(true),
                            call_id: tool.call_id,
                            remote_id: None,
                        }],
                        ..Default::default()
                    }));
                    continue;
                }
                let channel = match tool.name.as_str() {
                    "execute_kip_readonly" => Channel::Kip,
                    "wiki_read" | "wiki_search" => Channel::Wiki,
                    "check_procedure_status" => Channel::Procedures,
                    _ => {
                        failed = true;
                        break;
                    }
                };
                let response = self
                    .budget_tool_before_deadline(ctx, &tool.name, tool.args.clone(), now)
                    .await;
                material.mark(channel, false);
                match response {
                    Ok((value, measured, tool_error)) => {
                        usage.accumulate(&measured);
                        tool_usage
                            .entry(tool.name.clone())
                            .or_default()
                            .accumulate(&measured);
                        if tool_error {
                            material.omit(channel);
                        }
                        let priority = if tool_error {
                            Priority::Warning
                        } else if channel == Channel::Procedures {
                            if value["recommendation_allowed"] == true {
                                Priority::VerifiedProcedure
                            } else {
                                Priority::Warning
                            }
                        } else if contains_program(&value) {
                            Priority::UnprovenProcedure
                        } else {
                            Priority::Relevant
                        };
                        discover_skills(&value, &mut material.skills);
                        if let Some(id) = material.add(channel, priority, value.clone())? {
                            if channel == Channel::Procedures
                                && let Some(skill) = tool.args["skill_ref"].as_str()
                            {
                                material.procedures.insert(id.clone(), skill.into());
                            }
                            observations
                                .push(json!({"tool":tool.name,"item_id":id,"partial":tool_error}));
                            conversation.messages.push(json!(Message {
                                role: "tool".into(),
                                content: vec![ContentPart::ToolOutput {
                                    name: tool.name.clone(),
                                    output: value,
                                    is_error: tool_error.then_some(true),
                                    call_id: tool.call_id.clone(),
                                    remote_id: None,
                                }],
                                ..Default::default()
                            }));
                        }
                    }
                    Err(_) => {
                        material.omit(channel);
                        material.add(
                            channel,
                            Priority::Warning,
                            json!({
                                "tool": tool.name,
                                "status": "unavailable",
                                "meaning": "not evidence of absence or complete coverage",
                            }),
                        )?;
                        observations.push(json!({"tool":tool.name,"status":"unavailable"}));
                    }
                }
                // A full result window is not an exhaustive search. A bounded
                // projection/lookup still cannot attest semantic coverage.
                if matches!(channel, Channel::Kip | Channel::Wiki) {
                    material.mark(channel, true);
                }
            }
            if failed || finished {
                break;
            }
        }
        // An exhausted planning-input budget can still deliver host-read
        // candidates. Keep every constraint/warning, mark partial coverage, and
        // never invent a summary or promote a procedure into executable standing.
        if material.failure == Some("recall_context_budget_exhausted") && !material.critical_missing
        {
            if let Some((items, coverage)) = context_fallback {
                material.items = items;
                material.coverage = coverage;
            }
            // Without model planning, return structured query candidates and
            // constraints, not opaque notes or prior conversational narratives.
            for channel in [Channel::Notes, Channel::History] {
                let before = material.items.len();
                material
                    .items
                    .retain(|item| item.channel != channel || item.priority <= Priority::Warning);
                if material.items.len() != before {
                    material.omit(channel);
                }
            }
            material.add(
                Channel::Primer,
                Priority::Warning,
                json!({
                    "status": "partial",
                    "reason": "recall_context_budget_exhausted",
                    "selection": "host-read candidates; model planning was incomplete",
                    "action_ready": false,
                }),
            )?;
            material.mark(Channel::Kip, true);
            selected = material.items.iter().map(|item| item.id.clone()).collect();
            material.failure = None;
            finished = true;
            failed = false;
        }
        failed |= !finished;
        if failed && material.failure.is_none() {
            material.failure = Some("recall_planner_incomplete");
        }
        selected.retain(|id| material.items.iter().any(|item| &item.id == id));
        if !failed {
            self.refresh_budget_procedures(ctx, &mut material, usage, tool_usage, now)
                .await?;
        }
        self.finish_budgeted(
            conversation,
            usage,
            tool_usage,
            limits,
            &material,
            &selected,
            failed,
        )
        .await
    }

    /// The claims a discovered Concept takes part in, as compact packet items. A failed
    /// or empty read adds nothing: the Concept record is still delivered, and the
    /// channel is already marked partial.
    async fn discovered_claims(&self, element: &Json, seen: &mut BTreeSet<String>) -> Vec<Json> {
        let Some(id) = element["id"].as_str() else {
            return Vec::new();
        };
        let response = self
            .budget_kip(kip::request_with(
                CLAIMS,
                kip::param("entity", json!({"id": id})),
            ))
            .await;
        let Ok(response) = response else {
            return Vec::new();
        };
        if !kip::succeeded(&response) {
            return Vec::new();
        }
        response
            .first_result()
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| claim_item(row, element))
            .filter(|claim| {
                claim["assertion"]
                    .as_str()
                    .is_some_and(|assertion| seen.insert(assertion.to_owned()))
            })
            .collect()
    }

    async fn budget_kip(&self, mut request: Request) -> Result<Response, BoxError> {
        let _guard = if let Some(control) = &self.product_control {
            let guard = control.gate.read().await;
            if !control.available() {
                return Err("memory_change_pending".into());
            }
            if control.epoch() > 0 {
                control.current_request(&request)?;
            }
            Some(guard)
        } else {
            None
        };
        self.clock.bind_read(&mut request)?;
        Ok(timeout(
            READONLY_KIP_TIMEOUT,
            kip::execute_readonly_request(self.memory.nexus().as_ref(), &request),
        )
        .await
        .map_err(|_| "bounded KIP read timed out")?)
    }

    async fn budget_tool(
        &self,
        ctx: &AgentCtx,
        name: &str,
        mut args: Json,
    ) -> Result<(Json, Usage, bool), BoxError> {
        if name == "execute_kip_readonly" {
            let args: KipArgs = serde_json::from_value(args)?;
            let mut request = args.into_readonly_request()?;
            if request.operations.len() > 4 {
                return Err("bounded Recall allows at most four KIP operations per read".into());
            }
            let parsed = request.parse_operations()?;
            let global = request.parameters.clone();
            for (op, mut cmd) in request.operations.iter_mut().zip(parsed) {
                let limit = match &mut cmd {
                    Command::Kql(query) => &mut query.limit,
                    Command::Meta(MetaCommand::Search(search)) => &mut search.limit,
                    Command::Meta(_) => {
                        return Err("bounded Recall supports KQL and SEARCH; broad metadata \
                                    expansion is unavailable"
                            .into());
                    }
                    Command::Kml(_) => return Err("Recall is read-only".into()),
                };
                *limit = Some(capped_limit(
                    limit.as_ref(),
                    op.parameters.as_ref(),
                    global.as_ref(),
                )?);
                op.command = None;
                op.ast = Some(cmd);
            }
            let result = self.budget_kip(request).await?;
            let error = !kip::succeeded(&result);
            return Ok((
                serde_json::to_value(result)?,
                Usage {
                    requests: 1,
                    ..Default::default()
                },
                error,
            ));
        }
        if name == "wiki_search" {
            args["top_k"] = json!(args["top_k"].as_u64().unwrap_or(8).min(8));
            args["expand"] = json!(0);
        }
        let output = timeout(
            READONLY_KIP_TIMEOUT,
            ctx.tool_call(ToolInput {
                name: name.into(),
                args,
                resources: vec![],
                meta: None,
            }),
        )
        .await
        .map_err(|_| "bounded Recall tool timed out")??;
        Ok((output.output, output.usage, output.is_error == Some(true)))
    }

    async fn budget_tool_before_deadline(
        &self,
        ctx: &AgentCtx,
        name: &str,
        args: Json,
        started_at: u64,
    ) -> Result<(Json, Usage, bool), BoxError> {
        let remaining = recall_time_remaining(started_at).ok_or("Recall read deadline reached")?;
        let cancel = ctx.cancellation_token();
        tokio::select! {
            _ = cancel.cancelled() => Err("Recall read cancelled".into()),
            result = timeout(remaining, self.budget_tool(ctx, name, args)) => {
                result.map_err(|_| "Recall read deadline reached")?
            }
        }
    }

    async fn refresh_budget_procedures(
        &self,
        ctx: &AgentCtx,
        material: &mut Material,
        usage: &mut Usage,
        tool_usage: &mut HashMap<String, Usage>,
        started_at: u64,
    ) -> Result<(), BoxError> {
        if !self
            .tool_dependencies()
            .iter()
            .any(|n| n == "check_procedure_status")
        {
            return Ok(());
        }
        let mut skills = material.skills.clone();
        skills.extend(material.procedures.values().cloned());
        if skills.len() > 8 {
            material.critical_missing = true;
            material.failure = Some("recall_procedure_window_incomplete");
            material.omit(Channel::Procedures);
            return Ok(());
        }
        // Drop all old status objects. No earlier true flag may survive refresh.
        material
            .items
            .retain(|i| !material.procedures.contains_key(&i.id));
        for skill in skills {
            let result = self
                .budget_tool_before_deadline(
                    ctx,
                    "check_procedure_status",
                    json!({"skill_ref":skill}),
                    started_at,
                )
                .await;
            let (content, priority) = match result {
                Ok((mut value, cost, false)) => {
                    usage.accumulate(&cost);
                    tool_usage
                        .entry("check_procedure_status".into())
                        .or_default()
                        .accumulate(&cost);
                    let valid = value["recommendation_allowed"] == true
                        && value["context_expires_at_ms"]
                            .as_u64()
                            .is_some_and(|t| t > unix_ms())
                        && value["review_due_at"].as_str().is_some_and(|t| {
                            t > crate::kip::timestamp(self.clock.now_ms()).as_str()
                        });
                    if valid {
                        (value, Priority::VerifiedProcedure)
                    } else {
                        value["recommendation_allowed"] = json!(false);
                        value["host_delivery_warning"] = json!(
                            "current applicability was not established or expired before delivery"
                        );
                        (value, Priority::Warning)
                    }
                }
                _ => (
                    json!({
                        "skill_ref": skill,
                        "recommendation_allowed": false,
                        "reason": "current procedure verification unavailable",
                    }),
                    Priority::Warning,
                ),
            };
            // The current check is mandatory even if the model omitted it.
            material.add(
                Channel::Procedures,
                if priority == Priority::VerifiedProcedure {
                    Priority::Required
                } else {
                    priority
                },
                content,
            )?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_budgeted(
        &self,
        conversation: &mut Conversation,
        usage: &Usage,
        tools_usage: &HashMap<String, Usage>,
        limits: &RecallBudget,
        material: &Material,
        selected: &[String],
        failed: bool,
    ) -> Result<AgentOutput, BoxError> {
        let mut items = material.items.clone();
        expire_procedure_checks(&mut items, self.clock.now_ms(), false);
        let utilities = self.utility_ranks(&items).await;
        let mut packet = if failed || material.critical_missing {
            budget::insufficient(limits, material.coverage())?
        } else {
            budget::pack_ranked(limits, &items, selected, material.coverage(), &utilities)?
        };
        // If any permit expires during serialization, downgrade every positive
        // check and serialize once more; the second packet contains no permits.
        if !failed
            && !material.critical_missing
            && expire_procedure_checks(&mut items, self.clock.now_ms(), false)
        {
            expire_procedure_checks(&mut items, self.clock.now_ms(), true);
            packet =
                budget::pack_ranked(limits, &items, selected, material.coverage(), &utilities)?;
        }
        conversation.status = if packet.insufficient {
            ConversationStatus::Failed
        } else {
            ConversationStatus::Completed
        };
        let failure = packet.insufficient.then(|| {
            material
                .failure
                .unwrap_or("recall_output_budget_exhausted")
                .to_string()
        });
        if let Some(reason) = &failure {
            packet = budget::with_failure_reason(limits, packet, reason)?;
        }
        conversation.failed_reason = failure.clone();
        conversation.usage = usage.clone();
        conversation.updated_at = unix_ms();
        conversation.messages.push(json!(Message {
            role: "assistant".into(),
            content: vec![packet.content.clone().into()],
            ..Default::default()
        }));
        self.persist_conversation(conversation).await;
        push_completed_history(&self.history, conversation, RECALL_HISTORY_LIMIT);
        self.hook
            .on_conversation_end(Self::NAME, conversation)
            .await;
        if let Some(receipts) = &self.receipts {
            let reference = receipts
                .issue_packet(
                    format!("conversation:{}", conversation._id),
                    packet.content.clone(),
                    limits.clone(),
                )
                .await?;
            receipts
                .bind_conversation(conversation._id, reference)
                .await?;
        }
        // This whitelist is the delivery seam, including direct agent calls.
        // No diagnostic conversation, reasoning, tool args, citations or
        // provider artifact can bypass the semantic memory packet budget.
        Ok(AgentOutput {
            content: packet.content,
            usage: usage.clone(),
            tools_usage: tools_usage.clone(),
            conversation: Some(conversation._id),
            failed_reason: failure,
            ..Default::default()
        })
    }
    async fn utility_ranks(&self, items: &[MemoryItem]) -> BTreeMap<String, f64> {
        let Some(utility) = self.utility.as_ref().and_then(|u| u.upgrade()) else {
            return BTreeMap::new();
        };
        let mut slots = BTreeMap::new();
        let mut pins = Vec::new();
        for item in items {
            let mut found = Vec::new();
            crate::assess::collect_entity_objects(&item.content, &mut |_, row| {
                if let Ok(p) = crate::recall_receipt::pin(&json!(row)) {
                    found.push(p);
                }
            });
            found.sort_by(|a, b| a.id.cmp(&b.id));
            found.dedup();
            if found.len() == 1 && found[0].id.starts_with("C-") {
                slots.insert(item.id.clone(), found[0].id.clone());
                pins.push(found.remove(0));
            }
        }
        // Optional ranking must not hold up or broaden required Recall reads.
        let ranks = tokio::time::timeout(Duration::from_millis(500), utility.rank(&pins))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        slots
            .into_iter()
            .filter_map(|(slot, id)| ranks.get(&id).map(|v| (slot, *v)))
            .collect()
    }
}

fn capped_limit(
    limit: Option<&Scalar>,
    local: Option<&serde_json::Map<String, Json>>,
    global: Option<&serde_json::Map<String, Json>>,
) -> Result<Scalar, BoxError> {
    let requested = match limit {
        None => MAX_ROWS as u64,
        Some(Scalar::Literal(KipValue::Number(n))) => n
            .as_u64()
            .ok_or("Recall LIMIT must be a nonnegative integer")?,
        Some(Scalar::Param(name)) => local
            .and_then(|p| p.get(name))
            .or_else(|| global.and_then(|p| p.get(name)))
            .and_then(Json::as_u64)
            .ok_or("Recall LIMIT parameter must resolve to a nonnegative integer")?,
        _ => return Err("Recall LIMIT must be a nonnegative integer".into()),
    };
    Ok(Scalar::Literal(KipValue::Number(
        requested.min(MAX_ROWS as u64).into(),
    )))
}

fn expire_procedure_checks(items: &mut [MemoryItem], business_now: u64, all: bool) -> bool {
    let mut changed = false;
    for item in items {
        if item.channel == Channel::Procedures
            && item.content["recommendation_allowed"] == true
            && (all
                || item.content["context_expires_at_ms"]
                    .as_u64()
                    .is_none_or(|t| t <= unix_ms())
                || item.content["review_due_at"]
                    .as_str()
                    .is_none_or(|t| t <= crate::kip::timestamp(business_now).as_str()))
        {
            item.content["recommendation_allowed"] = json!(false);
            item.content["host_delivery_warning"] =
                json!("procedure check expired before delivery; revalidation required");
            item.priority = Priority::Warning;
            changed = true;
        }
    }
    changed
}

fn is_sleep_task(element: &Json) -> bool {
    element["schema_ref"]
        .as_str()
        .is_some_and(|schema| schema.ends_with("/SleepTask"))
}

/// One `CLAIMS` row as a packet item. `entity` is the discovered Concept the row was
/// read for; it stands at whichever endpoint the row left unbound.
fn claim_item(row: &Json, entity: &Json) -> Option<Json> {
    let [
        proposition,
        subject,
        subject_name,
        predicate,
        object,
        object_name,
        belief,
        excluded,
        assertion,
        stance,
        mode,
        asserted_at,
        asserted_by,
        actor_name,
    ] = row.as_array()?.as_slice()
    else {
        return None;
    };
    // A Concept endpoint is delivered with its name; a Literal object is itself.
    let endpoint = |reference: &Json, name: &Json| -> Json {
        let Some(id) = reference.get("id") else {
            return reference.clone();
        };
        let name = if name.is_null() && *id == entity["id"] {
            &entity["name"]
        } else {
            name
        };
        json!({"id": id, "name": name})
    };
    let mut item = json!({
        "kind": "claim",
        "subject": endpoint(subject, subject_name),
        "predicate": predicate,
        "object": endpoint(object, object_name),
        "belief": belief,
        "stance": stance,
        "mode": mode,
        "asserted_at": asserted_at,
        "asserted_by": endpoint(asserted_by, actor_name),
        "proposition": proposition,
        "assertion": assertion,
    });
    // Why this Assertion does not count toward the belief, such as a value that a
    // later one succeeded: the status alone reads as "nothing is known".
    let reasons: Vec<&Json> = excluded
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["assertion_id"] == *assertion)
        .map(|entry| &entry["reason"])
        .collect();
    if !reasons.is_empty() {
        item["excluded"] = json!(reasons);
    }
    Some(item)
}

fn contains_program(value: &Json) -> bool {
    let mut stack = vec![value];
    let mut nodes = 0;
    while let Some(value) = stack.pop() {
        nodes += 1;
        if nodes > 4096 {
            return true;
        }
        if value["schema_ref"] == profile!("Skill")
            || value["schema_ref"] == profile!("SkillRevision")
        {
            return true;
        }
        match value {
            Json::Object(map) => stack.extend(map.values()),
            Json::Array(items) => stack.extend(items),
            _ => {}
        }
    }
    false
}

/// Exact versioned serialization of every data-bearing field used by this
/// fresh-request mode. Provider-specific message templates/billing are not
/// inferred from it. Hidden history/documents cannot bypass this contract.
fn normalized_request(request: &CompletionRequest) -> Result<String, BoxError> {
    if !request.chat_history.is_empty()
        || !request.raw_history.is_empty()
        || !request.documents.is_empty()
        || !request.content.is_empty()
        || request.output_schema.is_some()
    {
        return Err("budgeted Recall does not accept additional request material".into());
    }
    Ok(
        json!({"format":"anda-brain-recall-input/1","instructions":request.instructions,
        "prompt":request.prompt,"tools":request.tools,"max_output_tokens":request.max_output_tokens,
        "tool_choice_required":request.tool_choice_required,"effort":request.effort})
        .to_string(),
    )
}
fn discover_skills(value: &Json, into: &mut BTreeSet<String>) {
    let mut stack = vec![value];
    let mut nodes = 0;
    while let Some(value) = stack.pop() {
        nodes += 1;
        if nodes > 4096 || into.len() > 8 {
            break;
        }
        if value["schema_ref"] == profile!("Skill")
            && let Some(id) = value["id"].as_str()
        {
            into.insert(id.into());
        }
        match value {
            Json::Object(map) => stack.extend(map.values()),
            Json::Array(items) => stack.extend(items),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
