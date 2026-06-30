//! Longitudinal memory evaluation harness.
//!
//! The harness intentionally drives Anda Brain through the same deep interface
//! used by callers: formation, recall, maintenance, and read-only KIP probes.
//! This keeps evals implementation-agnostic while still producing attribution
//! that points back to Formation, Recall, or Maintenance behavior.

use anda_core::{AgentOutput, BoxError, ContentPart, Json, Message, Usage};
use anda_engine::rfc3339_datetime_now;
use anda_kip::{Request, Response};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::{Instant, sleep};

use crate::{
    agents::SELF_USER_ID,
    payload::StringOr,
    space::Space,
    types::{FormationInput, InputContext, MaintenanceInput, MaintenanceScope, RecallInput},
};

const DEFAULT_WAIT_TIMEOUT_MS: u64 = 180_000;
const DEFAULT_POLL_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalScenario {
    pub id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Visible only to scenario authors and simulated users. The Brain never
    /// receives this directly; checkpoints and rubrics decide what matters.
    #[serde(default)]
    pub hidden_profile: Json,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context: Option<InputContext>,

    #[serde(default)]
    pub timeline: Vec<EvalTurn>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalTurn {
    pub turn: u64,

    #[serde(rename = "type")]
    pub turn_type: EvalTurnType,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<InputContext>,

    /// Convenience field for one-message turns in hand-written scenarios.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation: Option<EvalRubric>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<MaintenanceInput>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalTurnType {
    Normal,
    CheckpointOrganic,
    CheckpointSynthetic,
    Maintenance,
}

impl EvalTurnType {
    fn is_checkpoint(self) -> bool {
        matches!(
            self,
            EvalTurnType::CheckpointOrganic | EvalTurnType::CheckpointSynthetic
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EvalRubric {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoring_rubric: Option<String>,

    /// Terms that should appear in the final answer. These are deliberately
    /// simple and deterministic; LLM-as-judge can be layered on top later.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_answer_terms: Vec<String>,

    /// Terms whose presence usually means the answer is stale or overconfident.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_answer_terms: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_memories: Vec<ExpectedMemory>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectedMemory {
    pub id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(default)]
    pub mode: MemoryExpectationMode,

    #[serde(default = "default_expectation_weight")]
    pub weight: f64,

    /// Read-only KIP probe used to inspect whether the graph has the expected
    /// memory state before Recall answers the checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<Request>,

    /// Terms expected in the final answer when this memory is relevant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answer_terms: Vec<String>,

    /// Terms expected in recall tool traces if grounding succeeded. Defaults to
    /// answer_terms when omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace_terms: Vec<String>,
}

fn default_expectation_weight() -> f64 {
    1.0
}

impl Default for ExpectedMemory {
    fn default() -> Self {
        Self {
            id: String::new(),
            description: None,
            mode: MemoryExpectationMode::default(),
            weight: default_expectation_weight(),
            probe: None,
            answer_terms: Vec::new(),
            trace_terms: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryExpectationMode {
    /// The memory should be present and usable.
    #[default]
    ShouldExist,
    /// The memory should not be active anymore, usually because it was
    /// superseded, forgotten, or cleaned up.
    ShouldNotExist,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(default = "default_wait_timeout_ms")]
    pub wait_timeout_ms: u64,

    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,

    /// Run maintenance after every N normal turns. `None` means only explicit
    /// maintenance turns run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_every_n_turns: Option<usize>,

    #[serde(default)]
    pub maintenance_scope: MaintenanceScope,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_checkpoint_latency_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_checkpoint_total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalValidationReport {
    pub passed: bool,
    pub planned_runs: usize,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenarios: Vec<EvalScenarioPlan>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<EvalProfilePlan>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<EvalValidationIssue>,
}

impl EvalValidationReport {
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == EvalValidationSeverity::Error)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalScenarioPlan {
    pub id: String,
    pub normal_turns: usize,
    pub checkpoint_turns: usize,
    pub maintenance_turns: usize,
    pub expected_memories: usize,
    pub probes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalProfilePlan {
    pub id: String,
    pub wait_timeout_ms: u64,
    pub poll_interval_ms: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_every_n_turns: Option<usize>,

    pub maintenance_scope: MaintenanceScope,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_checkpoint_latency_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_checkpoint_total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalValidationIssue {
    pub severity: EvalValidationSeverity,
    pub path: String,
    pub message: String,
}

fn default_wait_timeout_ms() -> u64 {
    DEFAULT_WAIT_TIMEOUT_MS
}

fn default_poll_interval_ms() -> u64 {
    DEFAULT_POLL_INTERVAL_MS
}

impl Default for EvalProfile {
    fn default() -> Self {
        Self {
            id: None,
            wait_timeout_ms: DEFAULT_WAIT_TIMEOUT_MS,
            poll_interval_ms: DEFAULT_POLL_INTERVAL_MS,
            maintenance_every_n_turns: None,
            maintenance_scope: MaintenanceScope::Daydream,
            max_checkpoint_latency_ms: None,
            max_checkpoint_total_tokens: None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalAgentResult {
    pub content: String,
    pub usage: Usage,
    pub conversation: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_reason: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl From<AgentOutput> for EvalAgentResult {
    fn from(output: AgentOutput) -> Self {
        Self {
            content: output.content,
            usage: output.usage,
            conversation: output.conversation,
            failed_reason: output.failed_reason,
            model: output.model,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RecallTrace {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolTrace>,
}

impl RecallTrace {
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut tools: Vec<ToolTrace> = Vec::new();

        for message in messages {
            for part in &message.content {
                match part {
                    ContentPart::ToolCall {
                        name,
                        args,
                        call_id,
                    } => tools.push(ToolTrace {
                        name: name.clone(),
                        args: args.clone(),
                        call_id: call_id.clone(),
                        output: None,
                        is_error: None,
                    }),
                    ContentPart::ToolOutput {
                        name,
                        output,
                        is_error,
                        call_id,
                        ..
                    } => {
                        if let Some(existing) = tools.iter_mut().rev().find(|trace| {
                            trace.output.is_none()
                                && trace.name == *name
                                && (call_id.is_none() || trace.call_id == *call_id)
                        }) {
                            existing.output = Some(output.clone());
                            existing.is_error = *is_error;
                        } else {
                            tools.push(ToolTrace {
                                name: name.clone(),
                                args: Json::Null,
                                call_id: call_id.clone(),
                                output: Some(output.clone()),
                                is_error: *is_error,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        Self { tools }
    }

    pub fn contains_any_term(&self, terms: &[String]) -> bool {
        if terms.is_empty() {
            return false;
        }

        let haystack = serde_json::to_string(self)
            .unwrap_or_default()
            .to_lowercase();
        terms
            .iter()
            .any(|term| !term.trim().is_empty() && haystack.contains(&term.to_lowercase()))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolTrace {
    pub name: String,
    pub args: Json,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Json>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalReport {
    pub scenario_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub score: EvalScore,
    pub attribution: AttributionSummary,
    pub usage: Usage,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<EvalGateReport>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<EvalTurnReport>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalSuiteReport {
    pub suite_id: String,
    pub score: EvalScore,
    pub attribution: AttributionSummary,
    pub usage: Usage,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<EvalGateReport>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reports: Vec<EvalReport>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalExperimentReport {
    pub experiment_id: String,
    pub score: EvalScore,
    pub attribution: AttributionSummary,
    pub usage: Usage,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_suite_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<EvalGateReport>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparisons: Vec<EvalSuiteComparison>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suites: Vec<EvalSuiteReport>,
}

impl EvalExperimentReport {
    pub fn from_suites(experiment_id: String, suites: Vec<EvalSuiteReport>) -> Self {
        let mut usage = Usage::default();
        let mut attribution = AttributionSummary::default();
        for suite in &suites {
            usage.accumulate(&suite.usage);
            attribution.accumulate(&suite.attribution);
        }

        let score = aggregate_suite_scores(&suites);
        let comparisons = compare_suites(&suites);
        let best_suite_id = comparisons
            .first()
            .map(|comparison| comparison.suite_id.clone());
        Self {
            experiment_id,
            score,
            attribution,
            usage,
            best_suite_id,
            gate: None,
            comparisons,
            suites,
        }
    }
}

impl EvalSuiteReport {
    pub fn from_reports(suite_id: String, reports: Vec<EvalReport>) -> Self {
        let mut usage = Usage::default();
        let mut attribution = AttributionSummary::default();
        for report in &reports {
            usage.accumulate(&report.usage);
            attribution.accumulate(&report.attribution);
        }

        let score = aggregate_report_scores(&reports);
        Self {
            suite_id,
            score,
            attribution,
            usage,
            gate: None,
            reports,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalTurnReport {
    pub turn: u64,
    pub turn_type: EvalTurnTypeReport,
    pub latency_ms: u64,
    pub usage: Usage,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<EvalScore>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<MemoryProbeReport>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recall_trace: Option<RecallTrace>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<EvalFinding>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvalTurnTypeReport {
    #[default]
    Normal,
    Checkpoint,
    Maintenance,
    AutoMaintenance,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct MemoryProbeReport {
    pub expectation_id: String,
    pub mode: MemoryExpectationMode,
    pub hit_count: usize,
    pub satisfied: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<Response>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalScore {
    pub total: f64,
    pub memory_utility: f64,
    pub evolution_quality: f64,
    pub uncertainty_calibration: f64,
    pub forgetting_quality: f64,
    pub graph_health: f64,
    pub latency_penalty: f64,
    pub token_cost_penalty: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalSuiteComparison {
    pub suite_id: String,
    pub rank: usize,
    pub score: EvalScore,
    pub delta_from_best_total: f64,
    pub total_findings: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AttributionSummary {
    pub formation_miss: u64,
    pub bad_consolidation: u64,
    pub bad_grounding: u64,
    pub bad_synthesis: u64,
    pub overconfidence: u64,
    pub graph_probe_error: u64,
    pub latency_cost: u64,
    pub token_cost: u64,
}

impl AttributionSummary {
    pub fn accumulate(&mut self, other: &Self) {
        self.formation_miss = self.formation_miss.saturating_add(other.formation_miss);
        self.bad_consolidation = self
            .bad_consolidation
            .saturating_add(other.bad_consolidation);
        self.bad_grounding = self.bad_grounding.saturating_add(other.bad_grounding);
        self.bad_synthesis = self.bad_synthesis.saturating_add(other.bad_synthesis);
        self.overconfidence = self.overconfidence.saturating_add(other.overconfidence);
        self.graph_probe_error = self
            .graph_probe_error
            .saturating_add(other.graph_probe_error);
        self.latency_cost = self.latency_cost.saturating_add(other.latency_cost);
        self.token_cost = self.token_cost.saturating_add(other.token_cost);
    }

    pub fn total_findings(&self) -> u64 {
        self.formation_miss
            .saturating_add(self.bad_consolidation)
            .saturating_add(self.bad_grounding)
            .saturating_add(self.bad_synthesis)
            .saturating_add(self.overconfidence)
            .saturating_add(self.graph_probe_error)
            .saturating_add(self.latency_cost)
            .saturating_add(self.token_cost)
    }

    fn add_finding(&mut self, kind: EvalFindingKind) {
        match kind {
            EvalFindingKind::FormationMiss => self.formation_miss += 1,
            EvalFindingKind::BadConsolidation => self.bad_consolidation += 1,
            EvalFindingKind::BadGrounding => self.bad_grounding += 1,
            EvalFindingKind::BadSynthesis => self.bad_synthesis += 1,
            EvalFindingKind::Overconfidence => self.overconfidence += 1,
            EvalFindingKind::GraphProbeError => self.graph_probe_error += 1,
            EvalFindingKind::LatencyCost => self.latency_cost += 1,
            EvalFindingKind::TokenCost => self.token_cost += 1,
        }
    }

    fn add_turn(&mut self, turn: &EvalTurnReport) {
        for finding in &turn.findings {
            self.add_finding(finding.kind);
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalGate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_total_score: Option<f64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_findings: Option<u64>,
}

impl EvalGate {
    pub fn is_configured(&self) -> bool {
        self.min_total_score.is_some() || self.max_total_findings.is_some()
    }

    pub fn evaluate(&self, score: &EvalScore, attribution: &AttributionSummary) -> EvalGateReport {
        let mut failures = Vec::new();
        if let Some(min_total_score) = self.min_total_score
            && score.total < min_total_score
        {
            failures.push(format!(
                "total score {:.4} is below required minimum {:.4}",
                score.total, min_total_score
            ));
        }

        if let Some(max_total_findings) = self.max_total_findings {
            let total_findings = attribution.total_findings();
            if total_findings > max_total_findings {
                failures.push(format!(
                    "total findings {total_findings} exceeds maximum {max_total_findings}"
                ));
            }
        }

        EvalGateReport {
            criteria: self.clone(),
            passed: failures.is_empty(),
            failures,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EvalGateReport {
    pub criteria: EvalGate,
    pub passed: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
}

pub fn validate_eval_plan(
    scenarios: &[EvalScenario],
    profiles: &[EvalProfile],
) -> EvalValidationReport {
    let mut report = EvalValidationReport {
        planned_runs: scenarios.len().saturating_mul(profiles.len()),
        ..Default::default()
    };
    let mut scenario_ids = BTreeSet::new();
    let mut profile_ids = BTreeSet::new();

    if scenarios.is_empty() {
        push_validation_issue(
            &mut report,
            EvalValidationSeverity::Error,
            "scenarios",
            "at least one scenario is required",
        );
    }

    if profiles.is_empty() {
        push_validation_issue(
            &mut report,
            EvalValidationSeverity::Error,
            "profiles",
            "at least one profile is required",
        );
    }

    for (index, scenario) in scenarios.iter().enumerate() {
        let path = format!("scenarios[{index}]");
        let id = scenario.id.trim();
        if id.is_empty() {
            push_validation_issue(
                &mut report,
                EvalValidationSeverity::Error,
                &path,
                "scenario id must not be empty",
            );
        } else if !scenario_ids.insert(id.to_string()) {
            push_validation_issue(
                &mut report,
                EvalValidationSeverity::Error,
                &path,
                format!("duplicate scenario id `{id}`"),
            );
        }

        report
            .scenarios
            .push(validate_scenario_plan(scenario, index, &mut report.issues));
    }

    for (index, profile) in profiles.iter().enumerate() {
        let path = format!("profiles[{index}]");
        let id = profile.id.as_deref().unwrap_or("default").trim();
        if id.is_empty() {
            push_validation_issue(
                &mut report,
                EvalValidationSeverity::Error,
                &path,
                "profile id must not be empty",
            );
        } else if !profile_ids.insert(id.to_string()) {
            push_validation_issue(
                &mut report,
                EvalValidationSeverity::Error,
                &path,
                format!("duplicate profile id `{id}`"),
            );
        }

        report
            .profiles
            .push(validate_profile_plan(profile, index, &mut report.issues));
    }

    report.passed = !report.has_errors();
    report
}

fn validate_scenario_plan(
    scenario: &EvalScenario,
    scenario_index: usize,
    issues: &mut Vec<EvalValidationIssue>,
) -> EvalScenarioPlan {
    let mut plan = EvalScenarioPlan {
        id: scenario.id.clone(),
        ..Default::default()
    };
    let mut seen_turns = BTreeSet::new();
    let mut previous_turn = None;

    if scenario.timeline.is_empty() {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Warning,
            path: format!("scenarios[{scenario_index}].timeline"),
            message: "scenario has no turns".to_string(),
        });
    }

    for (turn_index, turn) in scenario.timeline.iter().enumerate() {
        let path = format!("scenarios[{scenario_index}].timeline[{turn_index}]");
        if !seen_turns.insert(turn.turn) {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Error,
                path: path.clone(),
                message: format!("duplicate turn number {}", turn.turn),
            });
        }
        if let Some(previous) = previous_turn
            && turn.turn < previous
        {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Warning,
                path: path.clone(),
                message: format!(
                    "turn number {} is lower than previous turn {previous}",
                    turn.turn
                ),
            });
        }
        previous_turn = Some(turn.turn);

        match turn.turn_type {
            EvalTurnType::Normal => {
                plan.normal_turns += 1;
                if !turn_has_input_messages(turn) {
                    issues.push(EvalValidationIssue {
                        severity: EvalValidationSeverity::Error,
                        path,
                        message: "normal turn must include `user` text or `messages`".to_string(),
                    });
                }
            }
            EvalTurnType::Maintenance => {
                plan.maintenance_turns += 1;
            }
            kind if kind.is_checkpoint() => {
                plan.checkpoint_turns += 1;
                validate_checkpoint_turn(turn, &path, issues, &mut plan);
            }
            _ => {}
        }
    }

    if plan.checkpoint_turns == 0 {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Warning,
            path: format!("scenarios[{scenario_index}].timeline"),
            message: "scenario has no checkpoint turns, so aggregate score will be zero"
                .to_string(),
        });
    }

    plan
}

fn validate_checkpoint_turn(
    turn: &EvalTurn,
    path: &str,
    issues: &mut Vec<EvalValidationIssue>,
    plan: &mut EvalScenarioPlan,
) {
    if turn
        .query
        .as_ref()
        .map(|query| query.trim().is_empty())
        .unwrap_or(true)
    {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: path.to_string(),
            message: "checkpoint turn must include a non-empty `query`".to_string(),
        });
    }

    let Some(rubric) = &turn.evaluation else {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Warning,
            path: format!("{path}.evaluation"),
            message: "checkpoint has no evaluation rubric".to_string(),
        });
        return;
    };

    if rubric.required_answer_terms.is_empty()
        && rubric.forbidden_answer_terms.is_empty()
        && rubric.expected_memories.is_empty()
    {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Warning,
            path: format!("{path}.evaluation"),
            message: "checkpoint rubric has no answer terms or memory expectations".to_string(),
        });
    }

    validate_term_overlap(path, rubric, issues);

    let mut expectation_ids = BTreeSet::new();
    for (expectation_index, expectation) in rubric.expected_memories.iter().enumerate() {
        let expectation_path = format!("{path}.evaluation.expected_memories[{expectation_index}]");
        plan.expected_memories += 1;
        if expectation.id.trim().is_empty() {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Error,
                path: expectation_path.clone(),
                message: "expected memory id must not be empty".to_string(),
            });
        } else if !expectation_ids.insert(expectation.id.trim().to_string()) {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Error,
                path: expectation_path.clone(),
                message: format!("duplicate expected memory id `{}`", expectation.id),
            });
        }

        if !expectation.weight.is_finite() || expectation.weight <= 0.0 {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Error,
                path: format!("{expectation_path}.weight"),
                message: "expected memory weight must be a positive finite number".to_string(),
            });
        }

        match &expectation.probe {
            Some(probe) => {
                plan.probes += 1;
                if !probe.readonly {
                    issues.push(EvalValidationIssue {
                        severity: EvalValidationSeverity::Error,
                        path: format!("{expectation_path}.probe.readonly"),
                        message: "memory probe must set `readonly` to true".to_string(),
                    });
                }
                if probe.command.trim().is_empty() && probe.commands.is_empty() {
                    issues.push(EvalValidationIssue {
                        severity: EvalValidationSeverity::Warning,
                        path: format!("{expectation_path}.probe"),
                        message: "memory probe has neither `command` nor `commands`".to_string(),
                    });
                }
            }
            None if expectation.mode == MemoryExpectationMode::ShouldNotExist => {
                issues.push(EvalValidationIssue {
                    severity: EvalValidationSeverity::Warning,
                    path: expectation_path.clone(),
                    message: "`should_not_exist` expectations are strongest with a probe"
                        .to_string(),
                });
            }
            None => {}
        }

        if expectation.mode == MemoryExpectationMode::ShouldExist
            && expectation.probe.is_none()
            && expectation.answer_terms.is_empty()
            && expectation.trace_terms.is_empty()
        {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Warning,
                path: expectation_path,
                message: "`should_exist` expectation has no probe, answer terms, or trace terms"
                    .to_string(),
            });
        }
    }
}

fn validate_term_overlap(path: &str, rubric: &EvalRubric, issues: &mut Vec<EvalValidationIssue>) {
    let required: BTreeSet<String> = rubric
        .required_answer_terms
        .iter()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();

    for forbidden in &rubric.forbidden_answer_terms {
        let forbidden = forbidden.trim().to_lowercase();
        if !forbidden.is_empty() && required.contains(&forbidden) {
            issues.push(EvalValidationIssue {
                severity: EvalValidationSeverity::Warning,
                path: format!("{path}.evaluation"),
                message: format!("answer term `{forbidden}` is both required and forbidden"),
            });
        }
    }
}

fn validate_profile_plan(
    profile: &EvalProfile,
    profile_index: usize,
    issues: &mut Vec<EvalValidationIssue>,
) -> EvalProfilePlan {
    let path = format!("profiles[{profile_index}]");
    let id = profile.id.clone().unwrap_or_else(|| "default".to_string());

    if profile.wait_timeout_ms == 0 {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: format!("{path}.wait_timeout_ms"),
            message: "`wait_timeout_ms` must be greater than zero".to_string(),
        });
    }

    if profile.poll_interval_ms == 0 {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: format!("{path}.poll_interval_ms"),
            message: "`poll_interval_ms` must be greater than zero".to_string(),
        });
    } else if profile.wait_timeout_ms > 0 && profile.poll_interval_ms > profile.wait_timeout_ms {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Warning,
            path: format!("{path}.poll_interval_ms"),
            message: "`poll_interval_ms` is greater than `wait_timeout_ms`".to_string(),
        });
    }

    if profile.maintenance_every_n_turns == Some(0) {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: format!("{path}.maintenance_every_n_turns"),
            message: "`maintenance_every_n_turns` must be greater than zero when set".to_string(),
        });
    }

    if profile.max_checkpoint_latency_ms == Some(0) {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: format!("{path}.max_checkpoint_latency_ms"),
            message: "`max_checkpoint_latency_ms` must be greater than zero when set".to_string(),
        });
    }

    if profile.max_checkpoint_total_tokens == Some(0) {
        issues.push(EvalValidationIssue {
            severity: EvalValidationSeverity::Error,
            path: format!("{path}.max_checkpoint_total_tokens"),
            message: "`max_checkpoint_total_tokens` must be greater than zero when set".to_string(),
        });
    }

    EvalProfilePlan {
        id,
        wait_timeout_ms: profile.wait_timeout_ms,
        poll_interval_ms: profile.poll_interval_ms,
        maintenance_every_n_turns: profile.maintenance_every_n_turns,
        maintenance_scope: profile.maintenance_scope,
        max_checkpoint_latency_ms: profile.max_checkpoint_latency_ms,
        max_checkpoint_total_tokens: profile.max_checkpoint_total_tokens,
    }
}

fn push_validation_issue(
    report: &mut EvalValidationReport,
    severity: EvalValidationSeverity,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    report.issues.push(EvalValidationIssue {
        severity,
        path: path.into(),
        message: message.into(),
    });
}

fn turn_has_input_messages(turn: &EvalTurn) -> bool {
    turn.user
        .as_ref()
        .map(|user| !user.trim().is_empty())
        .unwrap_or(false)
        || !turn.messages.is_empty()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalFinding {
    pub kind: EvalFindingKind,
    pub message: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expectation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvalFindingKind {
    FormationMiss,
    BadConsolidation,
    BadGrounding,
    BadSynthesis,
    Overconfidence,
    GraphProbeError,
    LatencyCost,
    TokenCost,
}

#[async_trait::async_trait]
pub trait EvalDriver: Send + Sync {
    async fn remember(&self, input: FormationInput) -> Result<EvalAgentResult, BoxError>;
    async fn recall(&self, input: RecallInput) -> Result<EvalAgentResult, BoxError>;
    async fn maintain(&self, input: MaintenanceInput) -> Result<EvalAgentResult, BoxError>;
    async fn execute_kip_readonly(&self, request: Request) -> Result<Response, BoxError>;

    async fn wait_for_formation(
        &self,
        _conversation: u64,
        _timeout: Duration,
        _poll_interval: Duration,
    ) -> Result<(), BoxError> {
        Ok(())
    }

    async fn wait_for_maintenance(
        &self,
        _conversation: u64,
        _timeout: Duration,
        _poll_interval: Duration,
    ) -> Result<(), BoxError> {
        Ok(())
    }

    async fn recall_trace(&self, _conversation: u64) -> Result<Option<RecallTrace>, BoxError> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl EvalDriver for Space {
    async fn remember(&self, input: FormationInput) -> Result<EvalAgentResult, BoxError> {
        self.ingest(SELF_USER_ID, StringOr::Value(input))
            .await
            .map(EvalAgentResult::from)
    }

    async fn recall(&self, input: RecallInput) -> Result<EvalAgentResult, BoxError> {
        self.query(SELF_USER_ID, StringOr::Value(input))
            .await
            .map(EvalAgentResult::from)
    }

    async fn maintain(&self, input: MaintenanceInput) -> Result<EvalAgentResult, BoxError> {
        self.maintenance(SELF_USER_ID, input)
            .await
            .map(EvalAgentResult::from)
    }

    async fn execute_kip_readonly(&self, request: Request) -> Result<Response, BoxError> {
        self.execute_kip_readonly(request).await
    }

    async fn wait_for_formation(
        &self,
        conversation: u64,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<(), BoxError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.formation_status();
            if !status.formation_processing && status.formation_processed_id >= conversation {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "formation conversation {conversation} did not complete within {} ms",
                    timeout.as_millis()
                )
                .into());
            }
            sleep(poll_interval).await;
        }
    }

    async fn wait_for_maintenance(
        &self,
        conversation: u64,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<(), BoxError> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = self.formation_status();
            if !status.maintenance_processing {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "maintenance conversation {conversation} did not complete within {} ms",
                    timeout.as_millis()
                )
                .into());
            }
            sleep(poll_interval).await;
        }
    }

    async fn recall_trace(&self, conversation: u64) -> Result<Option<RecallTrace>, BoxError> {
        let conversation = self
            .get_conversation(Some("recall".to_string()), conversation)
            .await?;
        let messages: Vec<Message> = conversation
            .messages
            .into_iter()
            .filter_map(|message| serde_json::from_value::<Message>(message).ok())
            .collect();
        Ok(Some(RecallTrace::from_messages(&messages)))
    }
}

#[async_trait::async_trait]
impl EvalDriver for Arc<Space> {
    async fn remember(&self, input: FormationInput) -> Result<EvalAgentResult, BoxError> {
        self.as_ref().remember(input).await
    }

    async fn recall(&self, input: RecallInput) -> Result<EvalAgentResult, BoxError> {
        self.as_ref().recall(input).await
    }

    async fn maintain(&self, input: MaintenanceInput) -> Result<EvalAgentResult, BoxError> {
        self.as_ref().maintain(input).await
    }

    async fn execute_kip_readonly(&self, request: Request) -> Result<Response, BoxError> {
        self.as_ref().execute_kip_readonly(request).await
    }

    async fn wait_for_formation(
        &self,
        conversation: u64,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<(), BoxError> {
        self.as_ref()
            .wait_for_formation(conversation, timeout, poll_interval)
            .await
    }

    async fn wait_for_maintenance(
        &self,
        conversation: u64,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<(), BoxError> {
        self.as_ref()
            .wait_for_maintenance(conversation, timeout, poll_interval)
            .await
    }

    async fn recall_trace(&self, conversation: u64) -> Result<Option<RecallTrace>, BoxError> {
        self.as_ref().recall_trace(conversation).await
    }
}

pub async fn run_scenario<D>(
    driver: &D,
    scenario: &EvalScenario,
    profile: &EvalProfile,
) -> Result<EvalReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let mut report = EvalReport {
        scenario_id: scenario.id.clone(),
        description: scenario.description.clone(),
        ..Default::default()
    };
    let mut normal_turns_since_maintenance = 0usize;
    let timeout = Duration::from_millis(profile.wait_timeout_ms);
    let poll_interval = Duration::from_millis(profile.poll_interval_ms);

    for turn in &scenario.timeline {
        match turn.turn_type {
            EvalTurnType::Normal => {
                let turn_report =
                    run_normal_turn(driver, scenario, turn, timeout, poll_interval).await?;
                normal_turns_since_maintenance += 1;
                report.usage.accumulate(&turn_report.usage);
                report.turns.push(turn_report);

                if let Some(every) = profile.maintenance_every_n_turns
                    && every > 0
                    && normal_turns_since_maintenance >= every
                {
                    let turn_report =
                        run_auto_maintenance(driver, profile, turn, timeout, poll_interval).await?;
                    normal_turns_since_maintenance = 0;
                    report.usage.accumulate(&turn_report.usage);
                    report.turns.push(turn_report);
                }
            }
            EvalTurnType::Maintenance => {
                let turn_report =
                    run_maintenance_turn(driver, turn, timeout, poll_interval).await?;
                report.usage.accumulate(&turn_report.usage);
                report.turns.push(turn_report);
                normal_turns_since_maintenance = 0;
            }
            kind if kind.is_checkpoint() => {
                let turn_report = run_checkpoint_turn(driver, scenario, turn, profile).await?;
                report.usage.accumulate(&turn_report.usage);
                report.attribution.add_turn(&turn_report);
                report.turns.push(turn_report);
            }
            _ => {}
        }
    }

    report.score = aggregate_scores(&report.turns);
    Ok(report)
}

async fn run_normal_turn<D>(
    driver: &D,
    scenario: &EvalScenario,
    turn: &EvalTurn,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<EvalTurnReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let messages = turn_messages(turn)?;
    let input = FormationInput {
        messages,
        context: turn_context(scenario, turn),
        timestamp: Some(turn_timestamp(turn)),
    };

    let started = Instant::now();
    let output = driver.remember(input).await?;
    if let Some(conversation) = output.conversation {
        driver
            .wait_for_formation(conversation, timeout, poll_interval)
            .await?;
    }

    Ok(EvalTurnReport {
        turn: turn.turn,
        turn_type: EvalTurnTypeReport::Normal,
        latency_ms: started.elapsed().as_millis() as u64,
        usage: output.usage,
        conversation: output.conversation,
        findings: agent_failure_finding(output.failed_reason),
        ..Default::default()
    })
}

async fn run_maintenance_turn<D>(
    driver: &D,
    turn: &EvalTurn,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<EvalTurnReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let input = turn
        .maintenance
        .clone()
        .unwrap_or_else(|| MaintenanceInput {
            timestamp: turn.timestamp.clone(),
            ..Default::default()
        });

    run_maintenance(
        driver,
        turn.turn,
        EvalTurnTypeReport::Maintenance,
        input,
        timeout,
        poll_interval,
    )
    .await
}

async fn run_auto_maintenance<D>(
    driver: &D,
    profile: &EvalProfile,
    turn: &EvalTurn,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<EvalTurnReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let input = MaintenanceInput {
        trigger: "threshold".to_string(),
        scope: profile.maintenance_scope,
        timestamp: turn.timestamp.clone(),
        ..Default::default()
    };

    run_maintenance(
        driver,
        turn.turn,
        EvalTurnTypeReport::AutoMaintenance,
        input,
        timeout,
        poll_interval,
    )
    .await
}

async fn run_maintenance<D>(
    driver: &D,
    turn: u64,
    turn_type: EvalTurnTypeReport,
    input: MaintenanceInput,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<EvalTurnReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let started = Instant::now();
    let output = driver.maintain(input).await?;
    if let Some(conversation) = output.conversation {
        driver
            .wait_for_maintenance(conversation, timeout, poll_interval)
            .await?;
    }

    Ok(EvalTurnReport {
        turn,
        turn_type,
        latency_ms: started.elapsed().as_millis() as u64,
        usage: output.usage,
        conversation: output.conversation,
        findings: agent_failure_finding(output.failed_reason),
        ..Default::default()
    })
}

async fn run_checkpoint_turn<D>(
    driver: &D,
    scenario: &EvalScenario,
    turn: &EvalTurn,
    profile: &EvalProfile,
) -> Result<EvalTurnReport, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let rubric = turn.evaluation.clone().unwrap_or_default();
    let probes = run_memory_probes(driver, &rubric).await?;
    let query = checkpoint_query(turn)?;
    let input = RecallInput {
        query,
        context: turn_context(scenario, turn),
    };

    let started = Instant::now();
    let output = driver.recall(input).await?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let trace = match output.conversation {
        Some(conversation) => driver.recall_trace(conversation).await?,
        None => None,
    };
    let mut turn_report = EvalTurnReport {
        turn: turn.turn,
        turn_type: EvalTurnTypeReport::Checkpoint,
        latency_ms,
        usage: output.usage.clone(),
        conversation: output.conversation,
        answer: Some(output.content.clone()),
        probes,
        recall_trace: trace,
        findings: agent_failure_finding(output.failed_reason),
        ..Default::default()
    };
    let score = score_checkpoint(&rubric, &turn_report, profile);
    turn_report.findings.extend(score.findings);
    turn_report.score = Some(score.score);
    Ok(turn_report)
}

async fn run_memory_probes<D>(
    driver: &D,
    rubric: &EvalRubric,
) -> Result<Vec<MemoryProbeReport>, BoxError>
where
    D: EvalDriver + ?Sized,
{
    let mut probes = Vec::new();
    for expectation in &rubric.expected_memories {
        let Some(request) = expectation.probe.clone() else {
            continue;
        };
        let response = driver.execute_kip_readonly(request).await?;
        let hit_count = response_hit_count(&response);
        let satisfied = match expectation.mode {
            MemoryExpectationMode::ShouldExist => {
                hit_count > 0 && !matches!(response, Response::Err { .. })
            }
            MemoryExpectationMode::ShouldNotExist => {
                hit_count == 0 && !matches!(response, Response::Err { .. })
            }
        };
        probes.push(MemoryProbeReport {
            expectation_id: expectation.id.clone(),
            mode: expectation.mode,
            hit_count,
            satisfied,
            response: Some(response),
        });
    }
    Ok(probes)
}

struct CheckpointScore {
    score: EvalScore,
    findings: Vec<EvalFinding>,
}

fn score_checkpoint(
    rubric: &EvalRubric,
    turn: &EvalTurnReport,
    profile: &EvalProfile,
) -> CheckpointScore {
    let answer = turn.answer.as_deref().unwrap_or_default();
    let mut findings = Vec::new();

    let required_answer_score = fraction_present(&rubric.required_answer_terms, answer);
    for term in missing_terms(&rubric.required_answer_terms, answer) {
        findings.push(EvalFinding {
            kind: EvalFindingKind::BadSynthesis,
            expectation_id: None,
            message: format!("answer is missing required term `{term}`"),
        });
    }

    let mut expected_present_weight = 0.0;
    let mut expected_present_score = 0.0;
    let mut forgetting_weight = 0.0;
    let mut forgetting_score = 0.0;
    let mut probe_errors = 0usize;
    let probe_by_id: BTreeMap<&str, &MemoryProbeReport> = turn
        .probes
        .iter()
        .map(|probe| (probe.expectation_id.as_str(), probe))
        .collect();

    for expectation in &rubric.expected_memories {
        let probe = probe_by_id.get(expectation.id.as_str()).copied();
        let probe_satisfied = probe.map(|p| p.satisfied).unwrap_or(true);
        let probe_error = probe
            .and_then(|p| p.response.as_ref())
            .is_some_and(|response| matches!(response, Response::Err { .. }));
        if probe_error {
            probe_errors += 1;
            findings.push(EvalFinding {
                kind: EvalFindingKind::GraphProbeError,
                expectation_id: Some(expectation.id.clone()),
                message: "read-only KIP probe returned an error".to_string(),
            });
        }

        match expectation.mode {
            MemoryExpectationMode::ShouldExist => {
                expected_present_weight += expectation.weight;
                if probe_satisfied {
                    expected_present_score += expectation.weight;
                } else {
                    findings.push(EvalFinding {
                        kind: EvalFindingKind::FormationMiss,
                        expectation_id: Some(expectation.id.clone()),
                        message: "expected memory was not present in the graph before recall"
                            .to_string(),
                    });
                }

                let expectation_terms = expectation_terms(expectation);
                let missing = missing_terms(&expectation.answer_terms, answer);
                if probe_satisfied && !missing.is_empty() {
                    let trace_has_evidence = turn
                        .recall_trace
                        .as_ref()
                        .is_some_and(|trace| trace.contains_any_term(&expectation_terms));
                    let kind = if trace_has_evidence {
                        EvalFindingKind::BadSynthesis
                    } else {
                        EvalFindingKind::BadGrounding
                    };
                    findings.push(EvalFinding {
                        kind,
                        expectation_id: Some(expectation.id.clone()),
                        message: format!(
                            "answer did not use expected memory terms: {}",
                            missing.join(", ")
                        ),
                    });
                }
            }
            MemoryExpectationMode::ShouldNotExist => {
                forgetting_weight += expectation.weight;
                if probe_satisfied {
                    forgetting_score += expectation.weight;
                } else {
                    findings.push(EvalFinding {
                        kind: EvalFindingKind::BadConsolidation,
                        expectation_id: Some(expectation.id.clone()),
                        message: "stale or superseded memory is still active".to_string(),
                    });
                }
            }
        }
    }

    let forbidden_present = present_terms(&rubric.forbidden_answer_terms, answer);
    for term in &forbidden_present {
        findings.push(EvalFinding {
            kind: EvalFindingKind::Overconfidence,
            expectation_id: None,
            message: format!("answer contains forbidden or stale term `{term}`"),
        });
    }
    let uncertainty_calibration = if rubric.forbidden_answer_terms.is_empty() {
        1.0
    } else {
        1.0 - forbidden_present.len() as f64 / rubric.forbidden_answer_terms.len() as f64
    };

    let expected_present_score = if expected_present_weight == 0.0 {
        1.0
    } else {
        expected_present_score / expected_present_weight
    };
    let memory_utility = if rubric.required_answer_terms.is_empty() {
        expected_present_score
    } else {
        (required_answer_score + expected_present_score) / 2.0
    };

    let forgetting_quality = if forgetting_weight == 0.0 {
        1.0
    } else {
        forgetting_score / forgetting_weight
    };
    let graph_health = if turn.probes.is_empty() {
        1.0
    } else {
        1.0 - probe_errors as f64 / turn.probes.len() as f64
    };
    let evolution_quality = (memory_utility + forgetting_quality) / 2.0;

    let latency_penalty = profile
        .max_checkpoint_latency_ms
        .map(|max| over_budget_ratio(turn.latency_ms, max))
        .unwrap_or_default();
    if latency_penalty > 0.0 {
        findings.push(EvalFinding {
            kind: EvalFindingKind::LatencyCost,
            expectation_id: None,
            message: format!("checkpoint latency {} ms exceeded budget", turn.latency_ms),
        });
    }

    let token_cost_penalty = profile
        .max_checkpoint_total_tokens
        .map(|max| over_budget_ratio(usage_total_tokens(&turn.usage), max))
        .unwrap_or_default();
    if token_cost_penalty > 0.0 {
        findings.push(EvalFinding {
            kind: EvalFindingKind::TokenCost,
            expectation_id: None,
            message: "checkpoint token usage exceeded budget".to_string(),
        });
    }

    let total = clamp01(
        memory_utility * 0.45
            + evolution_quality * 0.2
            + uncertainty_calibration * 0.15
            + forgetting_quality * 0.1
            + graph_health * 0.1
            - latency_penalty * 0.05
            - token_cost_penalty * 0.05,
    );

    CheckpointScore {
        score: EvalScore {
            total,
            memory_utility: clamp01(memory_utility),
            evolution_quality: clamp01(evolution_quality),
            uncertainty_calibration: clamp01(uncertainty_calibration),
            forgetting_quality: clamp01(forgetting_quality),
            graph_health: clamp01(graph_health),
            latency_penalty: clamp01(latency_penalty),
            token_cost_penalty: clamp01(token_cost_penalty),
        },
        findings,
    }
}

fn aggregate_scores(turns: &[EvalTurnReport]) -> EvalScore {
    let checkpoint_scores: Vec<&EvalScore> = turns
        .iter()
        .filter_map(|turn| turn.score.as_ref())
        .collect();
    if checkpoint_scores.is_empty() {
        return EvalScore::default();
    }

    let len = checkpoint_scores.len() as f64;
    EvalScore {
        total: checkpoint_scores.iter().map(|s| s.total).sum::<f64>() / len,
        memory_utility: checkpoint_scores
            .iter()
            .map(|s| s.memory_utility)
            .sum::<f64>()
            / len,
        evolution_quality: checkpoint_scores
            .iter()
            .map(|s| s.evolution_quality)
            .sum::<f64>()
            / len,
        uncertainty_calibration: checkpoint_scores
            .iter()
            .map(|s| s.uncertainty_calibration)
            .sum::<f64>()
            / len,
        forgetting_quality: checkpoint_scores
            .iter()
            .map(|s| s.forgetting_quality)
            .sum::<f64>()
            / len,
        graph_health: checkpoint_scores
            .iter()
            .map(|s| s.graph_health)
            .sum::<f64>()
            / len,
        latency_penalty: checkpoint_scores
            .iter()
            .map(|s| s.latency_penalty)
            .sum::<f64>()
            / len,
        token_cost_penalty: checkpoint_scores
            .iter()
            .map(|s| s.token_cost_penalty)
            .sum::<f64>()
            / len,
    }
}

fn aggregate_report_scores(reports: &[EvalReport]) -> EvalScore {
    if reports.is_empty() {
        return EvalScore::default();
    }

    let len = reports.len() as f64;
    EvalScore {
        total: reports.iter().map(|r| r.score.total).sum::<f64>() / len,
        memory_utility: reports.iter().map(|r| r.score.memory_utility).sum::<f64>() / len,
        evolution_quality: reports
            .iter()
            .map(|r| r.score.evolution_quality)
            .sum::<f64>()
            / len,
        uncertainty_calibration: reports
            .iter()
            .map(|r| r.score.uncertainty_calibration)
            .sum::<f64>()
            / len,
        forgetting_quality: reports
            .iter()
            .map(|r| r.score.forgetting_quality)
            .sum::<f64>()
            / len,
        graph_health: reports.iter().map(|r| r.score.graph_health).sum::<f64>() / len,
        latency_penalty: reports.iter().map(|r| r.score.latency_penalty).sum::<f64>() / len,
        token_cost_penalty: reports
            .iter()
            .map(|r| r.score.token_cost_penalty)
            .sum::<f64>()
            / len,
    }
}

fn aggregate_suite_scores(suites: &[EvalSuiteReport]) -> EvalScore {
    if suites.is_empty() {
        return EvalScore::default();
    }

    let len = suites.len() as f64;
    EvalScore {
        total: suites.iter().map(|s| s.score.total).sum::<f64>() / len,
        memory_utility: suites.iter().map(|s| s.score.memory_utility).sum::<f64>() / len,
        evolution_quality: suites
            .iter()
            .map(|s| s.score.evolution_quality)
            .sum::<f64>()
            / len,
        uncertainty_calibration: suites
            .iter()
            .map(|s| s.score.uncertainty_calibration)
            .sum::<f64>()
            / len,
        forgetting_quality: suites
            .iter()
            .map(|s| s.score.forgetting_quality)
            .sum::<f64>()
            / len,
        graph_health: suites.iter().map(|s| s.score.graph_health).sum::<f64>() / len,
        latency_penalty: suites.iter().map(|s| s.score.latency_penalty).sum::<f64>() / len,
        token_cost_penalty: suites
            .iter()
            .map(|s| s.score.token_cost_penalty)
            .sum::<f64>()
            / len,
    }
}

fn compare_suites(suites: &[EvalSuiteReport]) -> Vec<EvalSuiteComparison> {
    let Some(best_total) = suites
        .iter()
        .map(|suite| suite.score.total)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
    else {
        return Vec::new();
    };

    let mut comparisons: Vec<EvalSuiteComparison> = suites
        .iter()
        .map(|suite| EvalSuiteComparison {
            suite_id: suite.suite_id.clone(),
            rank: 0,
            score: suite.score.clone(),
            delta_from_best_total: suite.score.total - best_total,
            total_findings: suite.attribution.total_findings(),
            total_tokens: usage_total_tokens(&suite.usage),
        })
        .collect();

    comparisons.sort_by(|a, b| {
        b.score
            .total
            .partial_cmp(&a.score.total)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.total_findings.cmp(&b.total_findings))
            .then_with(|| a.total_tokens.cmp(&b.total_tokens))
            .then_with(|| a.suite_id.cmp(&b.suite_id))
    });

    for (index, comparison) in comparisons.iter_mut().enumerate() {
        comparison.rank = index + 1;
    }

    comparisons
}

fn turn_messages(turn: &EvalTurn) -> Result<Vec<Message>, BoxError> {
    if !turn.messages.is_empty() {
        return Ok(turn.messages.clone());
    }

    let Some(user) = &turn.user else {
        return Err(format!("turn {} has no messages or user text", turn.turn).into());
    };

    Ok(vec![Message {
        role: "user".to_string(),
        content: vec![user.clone().into()],
        ..Default::default()
    }])
}

fn checkpoint_query(turn: &EvalTurn) -> Result<String, BoxError> {
    if let Some(query) = &turn.query
        && !query.trim().is_empty()
    {
        return Ok(query.clone());
    }
    if let Some(user) = &turn.user
        && !user.trim().is_empty()
    {
        return Ok(user.clone());
    }
    if let Some(message) = turn.messages.first()
        && let Some(text) = message.text()
    {
        return Ok(text);
    }

    Err(format!("checkpoint turn {} has no query", turn.turn).into())
}

fn turn_context(scenario: &EvalScenario, turn: &EvalTurn) -> Option<InputContext> {
    turn.context
        .clone()
        .or_else(|| scenario.default_context.clone())
}

fn turn_timestamp(turn: &EvalTurn) -> String {
    turn.timestamp.clone().unwrap_or_else(rfc3339_datetime_now)
}

fn agent_failure_finding(reason: Option<String>) -> Vec<EvalFinding> {
    reason
        .map(|reason| {
            vec![EvalFinding {
                kind: EvalFindingKind::BadSynthesis,
                expectation_id: None,
                message: format!("agent execution failed: {reason}"),
            }]
        })
        .unwrap_or_default()
}

fn expectation_terms(expectation: &ExpectedMemory) -> Vec<String> {
    if expectation.trace_terms.is_empty() {
        expectation.answer_terms.clone()
    } else {
        expectation.trace_terms.clone()
    }
}

fn response_hit_count(response: &Response) -> usize {
    match response {
        Response::Ok { result, .. } => json_hit_count(result),
        Response::Err { result, .. } => result.as_ref().map(json_hit_count).unwrap_or_default(),
    }
}

fn json_hit_count(value: &Json) -> usize {
    match value {
        Json::Null => 0,
        Json::Bool(false) => 0,
        Json::Bool(true) => 1,
        Json::Number(number) => {
            if number.as_f64().unwrap_or_default() == 0.0 {
                0
            } else {
                1
            }
        }
        Json::String(text) => usize::from(!text.trim().is_empty()),
        Json::Array(items) => {
            if items.iter().all(looks_like_serialized_kip_response) {
                items.iter().map(json_hit_count).sum()
            } else {
                items.len()
            }
        }
        Json::Object(map) => {
            if map.is_empty() {
                0
            } else if let Some(result) = map.get("result") {
                json_hit_count(result)
            } else if map.contains_key("error") {
                0
            } else {
                1
            }
        }
    }
}

fn looks_like_serialized_kip_response(value: &Json) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.contains_key("result") || map.contains_key("error"))
}

fn fraction_present(terms: &[String], text: &str) -> f64 {
    if terms.is_empty() {
        return 1.0;
    }
    present_terms(terms, text).len() as f64 / terms.len() as f64
}

fn present_terms(terms: &[String], text: &str) -> Vec<String> {
    let text = text.to_lowercase();
    terms
        .iter()
        .filter(|term| {
            let term = term.trim();
            !term.is_empty() && text.contains(&term.to_lowercase())
        })
        .cloned()
        .collect()
}

fn missing_terms(terms: &[String], text: &str) -> Vec<String> {
    let text = text.to_lowercase();
    terms
        .iter()
        .filter(|term| {
            let term = term.trim();
            !term.is_empty() && !text.contains(&term.to_lowercase())
        })
        .cloned()
        .collect()
}

fn over_budget_ratio(actual: u64, max: u64) -> f64 {
    if max == 0 || actual <= max {
        return 0.0;
    }
    ((actual - max) as f64 / max as f64).min(1.0)
}

fn usage_total_tokens(usage: &Usage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cached_tokens)
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anda_core::ToolOutput;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeEvalDriver {
        recall_answer: String,
        trace: Option<RecallTrace>,
        probes: Mutex<BTreeMap<String, Response>>,
        remembered: Mutex<Vec<FormationInput>>,
    }

    #[async_trait::async_trait]
    impl EvalDriver for FakeEvalDriver {
        async fn remember(&self, input: FormationInput) -> Result<EvalAgentResult, BoxError> {
            self.remembered.lock().unwrap().push(input);
            Ok(EvalAgentResult {
                conversation: Some(1),
                ..Default::default()
            })
        }

        async fn recall(&self, _input: RecallInput) -> Result<EvalAgentResult, BoxError> {
            Ok(EvalAgentResult {
                content: self.recall_answer.clone(),
                conversation: Some(2),
                usage: Usage {
                    input_tokens: 60,
                    output_tokens: 40,
                    ..Default::default()
                },
                ..Default::default()
            })
        }

        async fn maintain(&self, _input: MaintenanceInput) -> Result<EvalAgentResult, BoxError> {
            Ok(EvalAgentResult {
                conversation: Some(3),
                ..Default::default()
            })
        }

        async fn execute_kip_readonly(&self, request: Request) -> Result<Response, BoxError> {
            let key = request.command.clone();
            Ok(self
                .probes
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Response::ok(Json::Array(Vec::new()))))
        }

        async fn recall_trace(&self, _conversation: u64) -> Result<Option<RecallTrace>, BoxError> {
            Ok(self.trace.clone())
        }
    }

    #[tokio::test]
    async fn run_scenario_attributes_grounding_failure() {
        let driver = FakeEvalDriver {
            recall_answer: "I do not know.".to_string(),
            ..Default::default()
        };
        driver.probes.lock().unwrap().insert(
            "find_style".to_string(),
            Response::ok(json!([{"name": "concise direct style"}])),
        );
        let scenario = EvalScenario {
            id: "style".to_string(),
            default_context: Some(InputContext {
                counterparty: Some("user_042".to_string()),
                ..Default::default()
            }),
            timeline: vec![
                EvalTurn {
                    turn: 1,
                    turn_type: EvalTurnType::Normal,
                    user: Some("I prefer concise, direct writing.".to_string()),
                    ..empty_turn()
                },
                EvalTurn {
                    turn: 50,
                    turn_type: EvalTurnType::CheckpointOrganic,
                    query: Some("Can you rewrite this to sound more like me?".to_string()),
                    evaluation: Some(EvalRubric {
                        required_answer_terms: vec!["concise".to_string()],
                        expected_memories: vec![ExpectedMemory {
                            id: "style_pref".to_string(),
                            probe: Some(Request {
                                command: "find_style".to_string(),
                                readonly: true,
                                ..Default::default()
                            }),
                            answer_terms: vec!["concise".to_string()],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..empty_turn()
                },
            ],
            ..empty_scenario()
        };

        let report = run_scenario(&driver, &scenario, &EvalProfile::default())
            .await
            .unwrap();

        assert_eq!(driver.remembered.lock().unwrap().len(), 1);
        assert_eq!(report.attribution.bad_grounding, 1);
        assert!(report.score.total < 1.0);
    }

    #[test]
    fn recall_trace_extracts_tool_calls_and_outputs() {
        let call = ContentPart::ToolCall {
            name: "execute_kip_readonly".to_string(),
            args: json!({"command": "FIND(?x) WHERE { ?x {type: \"Preference\"} }"}),
            call_id: Some("call_1".to_string()),
        };
        let output = ToolOutput::new(json!([{"name": "prefers concise"}]));
        let output = ContentPart::ToolOutput {
            name: "execute_kip_readonly".to_string(),
            output: json!(output.output),
            is_error: None,
            call_id: Some("call_1".to_string()),
            remote_id: None,
        };
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: vec![call, output],
            ..Default::default()
        }];

        let trace = RecallTrace::from_messages(&messages);

        assert_eq!(trace.tools.len(), 1);
        assert!(trace.contains_any_term(&["concise".to_string()]));
    }

    #[test]
    fn response_hit_count_handles_batch_responses() {
        let response = Response::ok(json!([
            {"result": [{"name": "a"}, {"name": "b"}]},
            {"result": []},
            {"error": {"code": "KIP_3002"}}
        ]));

        assert_eq!(response_hit_count(&response), 2);
    }

    #[test]
    fn suite_report_aggregates_scores_usage_and_attribution() {
        let reports = vec![
            EvalReport {
                scenario_id: "a".to_string(),
                score: EvalScore {
                    total: 0.5,
                    memory_utility: 0.4,
                    ..Default::default()
                },
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                attribution: AttributionSummary {
                    bad_grounding: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            EvalReport {
                scenario_id: "b".to_string(),
                score: EvalScore {
                    total: 1.0,
                    memory_utility: 0.8,
                    ..Default::default()
                },
                usage: Usage {
                    input_tokens: 20,
                    output_tokens: 7,
                    ..Default::default()
                },
                attribution: AttributionSummary {
                    overconfidence: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        let suite = EvalSuiteReport::from_reports("suite".to_string(), reports);

        assert_eq!(suite.reports.len(), 2);
        assert_eq!(suite.usage.input_tokens, 30);
        assert_eq!(suite.usage.output_tokens, 12);
        assert_eq!(suite.attribution.bad_grounding, 1);
        assert_eq!(suite.attribution.overconfidence, 2);
        assert_eq!(suite.score.total, 0.75);
        assert!((suite.score.memory_utility - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn experiment_report_aggregates_suite_reports() {
        let suites = vec![
            EvalSuiteReport {
                suite_id: "a".to_string(),
                score: EvalScore {
                    total: 0.25,
                    graph_health: 0.5,
                    ..Default::default()
                },
                usage: Usage {
                    input_tokens: 3,
                    ..Default::default()
                },
                attribution: AttributionSummary {
                    formation_miss: 1,
                    ..Default::default()
                },
                ..Default::default()
            },
            EvalSuiteReport {
                suite_id: "b".to_string(),
                score: EvalScore {
                    total: 0.75,
                    graph_health: 1.0,
                    ..Default::default()
                },
                usage: Usage {
                    input_tokens: 7,
                    ..Default::default()
                },
                attribution: AttributionSummary {
                    bad_synthesis: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        let experiment = EvalExperimentReport::from_suites("experiment".to_string(), suites);

        assert_eq!(experiment.suites.len(), 2);
        assert_eq!(experiment.usage.input_tokens, 10);
        assert_eq!(experiment.attribution.formation_miss, 1);
        assert_eq!(experiment.attribution.bad_synthesis, 2);
        assert_eq!(experiment.score.total, 0.5);
        assert_eq!(experiment.score.graph_health, 0.75);
        assert_eq!(experiment.best_suite_id.as_deref(), Some("b"));
        assert_eq!(experiment.comparisons.len(), 2);
        assert_eq!(experiment.comparisons[0].suite_id, "b");
        assert_eq!(experiment.comparisons[0].rank, 1);
        assert_eq!(experiment.comparisons[0].delta_from_best_total, 0.0);
        assert_eq!(experiment.comparisons[1].suite_id, "a");
        assert_eq!(experiment.comparisons[1].rank, 2);
        assert_eq!(experiment.comparisons[1].delta_from_best_total, -0.5);
        assert_eq!(experiment.comparisons[1].total_findings, 1);
        assert_eq!(experiment.comparisons[1].total_tokens, 3);
    }

    #[test]
    fn eval_gate_reports_score_and_finding_failures() {
        let gate = EvalGate {
            min_total_score: Some(0.9),
            max_total_findings: Some(1),
        };
        let report = gate.evaluate(
            &EvalScore {
                total: 0.8,
                ..Default::default()
            },
            &AttributionSummary {
                formation_miss: 1,
                bad_grounding: 1,
                ..Default::default()
            },
        );

        assert!(!report.passed);
        assert_eq!(report.criteria.min_total_score, Some(0.9));
        assert_eq!(report.criteria.max_total_findings, Some(1));
        assert_eq!(report.failures.len(), 2);
        assert!(report.failures[0].contains("below required minimum"));
        assert!(report.failures[1].contains("exceeds maximum"));
    }

    #[test]
    fn validate_eval_plan_reports_offline_input_errors() {
        let scenario = EvalScenario {
            id: "scenario".to_string(),
            timeline: vec![
                EvalTurn {
                    turn: 1,
                    turn_type: EvalTurnType::Normal,
                    ..empty_turn()
                },
                EvalTurn {
                    turn: 2,
                    turn_type: EvalTurnType::CheckpointSynthetic,
                    query: Some("What should I remember?".to_string()),
                    evaluation: Some(EvalRubric {
                        required_answer_terms: vec!["direct".to_string()],
                        forbidden_answer_terms: vec!["direct".to_string()],
                        expected_memories: vec![ExpectedMemory {
                            id: "pref".to_string(),
                            probe: Some(Request {
                                command: "SEARCH CONCEPT \"direct\" MODE \"semantic\" LIMIT 1"
                                    .to_string(),
                                readonly: false,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                    ..empty_turn()
                },
            ],
            ..empty_scenario()
        };
        let profile = EvalProfile {
            id: Some("bad".to_string()),
            maintenance_every_n_turns: Some(0),
            ..Default::default()
        };

        let report = validate_eval_plan(&[scenario], &[profile]);

        assert!(!report.passed);
        assert_eq!(report.planned_runs, 1);
        assert_eq!(report.scenarios[0].normal_turns, 1);
        assert_eq!(report.scenarios[0].checkpoint_turns, 1);
        assert_eq!(report.scenarios[0].expected_memories, 1);
        assert_eq!(report.scenarios[0].probes, 1);
        assert_eq!(report.profiles[0].id, "bad");
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error && issue.message.contains("normal turn")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error && issue.message.contains("readonly")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("maintenance_every_n_turns")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Warning
                && issue.message.contains("both required and forbidden")
        }));
    }

    #[test]
    fn validate_eval_plan_reports_duplicate_ids_and_warning_only_cases() {
        let no_checkpoint = EvalScenario {
            id: "duplicate".to_string(),
            timeline: vec![EvalTurn {
                turn: 1,
                turn_type: EvalTurnType::Normal,
                user: Some("Remember this setup note.".to_string()),
                ..empty_turn()
            }],
            ..empty_scenario()
        };
        let invalid_checkpoint = EvalScenario {
            id: "duplicate".to_string(),
            timeline: vec![
                EvalTurn {
                    turn: 2,
                    turn_type: EvalTurnType::CheckpointSynthetic,
                    query: Some(String::new()),
                    evaluation: Some(EvalRubric {
                        expected_memories: vec![
                            ExpectedMemory {
                                id: "memory".to_string(),
                                weight: 0.0,
                                ..Default::default()
                            },
                            ExpectedMemory {
                                id: "memory".to_string(),
                                ..Default::default()
                            },
                        ],
                        ..Default::default()
                    }),
                    ..empty_turn()
                },
                EvalTurn {
                    turn: 1,
                    turn_type: EvalTurnType::CheckpointSynthetic,
                    query: Some("Out of order?".to_string()),
                    evaluation: Some(EvalRubric::default()),
                    ..empty_turn()
                },
            ],
            ..empty_scenario()
        };
        let profile_a = EvalProfile {
            id: Some("same".to_string()),
            wait_timeout_ms: 0,
            poll_interval_ms: 0,
            ..Default::default()
        };
        let profile_b = EvalProfile {
            id: Some("same".to_string()),
            poll_interval_ms: 1_000,
            wait_timeout_ms: 10,
            max_checkpoint_latency_ms: Some(0),
            max_checkpoint_total_tokens: Some(0),
            ..Default::default()
        };

        let report = validate_eval_plan(
            &[no_checkpoint, invalid_checkpoint],
            &[profile_a, profile_b],
        );

        assert!(!report.passed);
        assert_eq!(report.planned_runs, 4);
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("duplicate scenario id")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("duplicate profile id")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("non-empty `query`")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("positive finite")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Error
                && issue.message.contains("duplicate expected memory id")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Warning
                && issue.message.contains("no checkpoint")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Warning
                && issue.message.contains("lower than previous")
        }));
        assert!(report.issues.iter().any(|issue| {
            issue.severity == EvalValidationSeverity::Warning
                && issue.message.contains("greater than `wait_timeout_ms`")
        }));
    }

    fn empty_turn() -> EvalTurn {
        EvalTurn {
            turn: 0,
            turn_type: EvalTurnType::Normal,
            timestamp: None,
            context: None,
            user: None,
            messages: Vec::new(),
            query: None,
            evaluation: None,
            maintenance: None,
        }
    }

    fn empty_scenario() -> EvalScenario {
        EvalScenario {
            id: String::new(),
            description: None,
            hidden_profile: Json::Null,
            default_context: None,
            timeline: Vec::new(),
        }
    }
}
