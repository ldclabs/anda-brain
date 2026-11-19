//! Durable discovery and bounded attention work. The directory is a hint, not
//! cognitive truth or execution authority. One live host owns each storage shard.
use anda_cognitive_nexus::attention::{RuntimePins, RuntimeScope};
use anda_core::BoxError;
use serde::{Deserialize, Serialize};

mod catalog;
mod expiry;
pub mod semantic;
mod service;
#[cfg(test)]
mod tests;
pub(crate) use catalog::Directory;
pub use service::AttentionRuntime;

pub const FORMAT: &str = "anda-brain:attention-v1";
const DIRECTORY_FORMAT: &str = "anda-brain:attention-directory-v1";
const MAX_COUNTER: u64 = anda_kip::MAX_SAFE_INTEGER;

/// Trusted host configuration, installed before sharing/loading an AppState.
/// Structured Watch scanning uses no model. Separately installed semantic/action
/// bindings carry their own callback and delivery budgets.
#[derive(Clone, Debug)]
pub struct AttentionPolicy {
    pub enabled: bool,
    pub tick_ms: u64,
    pub spaces_per_tick: usize,
    pub watches_per_space: usize,
    pub changes_per_watch: usize,
    pub wakes_per_space: usize,
    pub wall_time_ms: u64,
    pub reconcile_ms: u64,
    pub blocked_retry_ms: u64,
}
impl Default for AttentionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            tick_ms: 5_000,
            spaces_per_tick: 20,
            watches_per_space: 20,
            changes_per_watch: crate::settlement::watch::CHANGES_PAGE_LIMIT,
            wakes_per_space: 200,
            wall_time_ms: 10_000,
            // A safety net for a missed hint, not the scheduler: due Watches
            // and dirty generations already set `next_check_ms`. Every
            // reconcile opens a cold Space, which an idle eviction then
            // closes again.
            reconcile_ms: 15 * 60_000,
            blocked_retry_ms: 60_000,
        }
    }
}
impl AttentionPolicy {
    pub fn validate(&self) -> Result<(), BoxError> {
        if !(1..=60_000).contains(&self.tick_ms)
            || !(1..=20).contains(&self.spaces_per_tick)
            || !(1..=20).contains(&self.watches_per_space)
            || !(1..=200).contains(&self.changes_per_watch)
            || !(1..=200).contains(&self.wakes_per_space)
            || !(1..=30_000).contains(&self.wall_time_ms)
            || !(self.tick_ms..=3_600_000).contains(&self.reconcile_ms)
            || !(self.tick_ms..=3_600_000).contains(&self.blocked_retry_ms)
        {
            return Err("invalid bounded attention policy".into());
        }
        Ok(())
    }
}

/// The versioned registration shape. The external Brain ID is explicitly mapped
/// to the native KIP space by the containing directory entry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub format: String,
    pub scope: RuntimeScope,
    pub pins: RuntimePins,
    pub version: u64,
    pub shard: u32,
    pub enabled: bool,
    pub dirty_generation: u64,
    pub reconciled_generation: u64,
    pub next_check_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionReport {
    #[serde(default)]
    pub actions_processed: usize,
    #[serde(default)]
    pub action_states: std::collections::BTreeMap<String, usize>,
    pub watches_scanned: usize,
    pub advanced: usize,
    pub fired: usize,
    pub expired: usize,
    pub blocked: usize,
    pub blocked_wakes: usize,
    pub blocked_reasons: std::collections::BTreeMap<String, usize>,
    pub conflicted: usize,
    pub wakes_scanned: usize,
    pub pending: usize,
    pub resumed: usize,
    pub rechecks_due: usize,
    pub scan_complete: bool,
    pub budget_exhausted: bool,
    pub error: Option<String>,
}
impl AttentionReport {
    fn defer(&mut self, reason: &str, wake: bool) {
        self.blocked += 1;
        self.blocked_wakes += usize::from(wake);
        *self.blocked_reasons.entry(reason.into()).or_default() += 1;
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AttentionTick {
    pub visited: usize,
    pub loaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub fired: usize,
    pub budget_exhausted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecheckSource {
    Defer,
    SkillReview,
    DependencyInvalidation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recheck {
    pub key: String,
    pub source: RecheckSource,
    pub due_at_ms: u64,
    pub notified: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    pub generation: u64,
    pub runnable_after: String,
    pub all_after: String,
    pub wake_cursor: Option<String>,
    pub wake_pending: Vec<String>,
    pub wake_page_cursor: Option<String>,
    pub wake_page_complete: bool,
    pub runnable_done: bool,
    pub all_done: bool,
    pub wakes_done: bool,
    pub next_due_ms: Option<u64>,
    pub running_until_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttentionStatus {
    pub registration: Registration,
    pub last_scan_ms: u64,
    pub last_report: AttentionReport,
    pub rechecks: Vec<Recheck>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub format: String,
    pub slot: u64,
    pub registration: Registration,
    pub native_scope: RuntimeScope,
    pub checkpoint: Checkpoint,
    pub last_scan_ms: u64,
    pub failures: u32,
    pub last_report: AttentionReport,
    pub rechecks: Vec<Recheck>,
}
impl Entry {
    fn validate(&self, id: &str, shard: u32) -> Result<(), BoxError> {
        let r = &self.registration;
        valid_id(id)?;
        if self.format != DIRECTORY_FORMAT
            || r.format != FORMAT
            || r.scope.space_id != id
            || r.shard != shard
            || r.scope.space_instance != self.native_scope.space_instance
            || r.scope.space_instance.is_empty()
            || r.scope.space_instance.len() > 256
            || r.version == 0
            || r.version > MAX_COUNTER
            || r.dirty_generation == 0
            || r.dirty_generation > MAX_COUNTER
            || r.reconciled_generation > r.dirty_generation
            || self.rechecks.len() > 128
            || self.checkpoint.wake_pending.len() > 200
            || self.slot == 0
            || self.slot > catalog::MAX_SLOTS
        {
            return Err("invalid attention directory identity/version".into());
        }
        Ok(())
    }
    fn status(&self) -> AttentionStatus {
        AttentionStatus {
            registration: self.registration.clone(),
            last_scan_ms: self.last_scan_ms,
            last_report: self.last_report.clone(),
            rechecks: self.rechecks.clone(),
        }
    }
}

fn valid_id(id: &str) -> Result<(), BoxError> {
    if id.is_empty()
        || id.len() > 128
        || id.trim() != id
        || id.contains(['/', '\\'])
        || id == "."
        || id == ".."
        || id.starts_with("__brain_runtime__")
    {
        return Err("invalid attention Space ID".into());
    }
    Ok(())
}
fn next(n: u64) -> Result<u64, BoxError> {
    n.checked_add(1)
        .filter(|n| *n <= MAX_COUNTER)
        .ok_or_else(|| "attention counter exhausted".into())
}
fn time_ms(value: &str) -> Result<u64, BoxError> {
    Ok(crate::kip::unix_time_ms(
        anda_cognitive_nexus::time::parse(value)?,
    )?)
}
fn min_due(old: &mut Option<u64>, new: u64) {
    *old = Some(old.map_or(new, |v| v.min(new)));
}
