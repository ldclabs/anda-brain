//! Immutable agent prompt configuration owned by a host instance.
//!
//! Defaults are compiled in. A trusted launcher may replace a deployment's
//! section A before creating an AppState; the vendored KIP reference prefix
//! always comes from the compiled asset. No process-wide mutable layer exists.

use anda_core::BoxError;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// Compiled-in default policies and deployment contracts, not complete system prompts.
///
/// Each is one mode's policy and this deployment's contract — the KIP language
/// itself is not in here. `anda_kip` ships the syntax card and the Cognitive
/// Memory Profile alongside the protocol they describe. The system prompt assembler
/// adds the full syntax, Profile, applicable role cards and host constraints before
/// every completion, including budgeted Recall and instance-specific contracts.
/// KIP 1.x taught the language from a copy pasted into each of these files;
/// three hand-maintained copies of a protocol drift, and the one that drifts
/// silently is the one a model then writes against.
pub const FORMATION_DEFAULT: &str = include_str!("../../assets/BrainFormation.md");
pub const RECALL_DEFAULT: &str = include_str!("../../assets/BrainRecall.md");
pub const MAINTENANCE_DEFAULT: &str = include_str!("../../assets/BrainMaintenance.md");

/// The shared language and ontology, included in every agent's system context.
/// Normative specification details remain available through `kip_reference`.
pub fn language_reference() -> &'static str {
    static REFERENCE: OnceLock<String> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}",
            anda_kip::KIP_SYNTAX,
            anda_kip::COGNITIVE_MEMORY_PROFILE,
            anda_kip::KIP_VOCABULARY_CARD
        )
    })
}

/// Complete syntax, ontology, applicable role cards and host constraints.
/// Kept separate from the deployment policy for existing Rust integrations.
pub fn mode_reference(target: PromptTarget) -> String {
    compiled_mode_reference(target).to_owned()
}

fn compiled_mode_reference(target: PromptTarget) -> &'static str {
    // Only immutable protocol/host text is shared. Deployment overrides remain
    // instance-owned, and live Primer/notes/time are appended by each caller.
    static REFERENCES: [OnceLock<String>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    REFERENCES[slot(target)].get_or_init(|| {
        let cards = match target {
            PromptTarget::Formation => anda_kip::KIP_FORMATION_CARD.to_string(),
            PromptTarget::Recall => anda_kip::KIP_RECALL_CARD.to_string(),
            PromptTarget::Maintenance => format!(
                "{}\n\n{}\n\n{}",
                anda_kip::KIP_RECALL_CARD,
                anda_kip::KIP_FORMATION_CARD,
                anda_kip::KIP_MAINTENANCE_CARD
            ),
        };
        format!(
            "{}\n\n---\n\n{}\n\n---\n\n{}\n\n{}",
            language_reference(),
            cards,
            crate::cognitive::CAPABILITIES,
            crate::kip_reference::INSTRUCTIONS
        )
    })
}

/// The static system prefix used by all agent completion paths and experiment
/// identities. Full syntax is unconditional; a deployment override only replaces
/// section A of `deployment_policy`. Append live context and mode limits after it.
pub(crate) fn system_prompt(target: PromptTarget, deployment_policy: &str) -> String {
    format!(
        "{}\n\n---\n\n{}",
        compiled_mode_reference(target),
        deployment_policy
    )
}

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
/// A selection-only pass spends its input on the native candidates. The upstream
/// Recall policy and all protocol cards stay in both variants. Only the built-in
/// section A for writing a prose answer is replaced; custom host policy is verbatim.
const PACKET_SELECTION: &str = "Select existing memory_items that answer the query. Use the \
    question's named subject, or use context.counterparty for the current user. Keep actor, \
    time, stance, exclusions and final BELIEF together. Include both links when an answer \
    needs a chain, and retain relevant uncertainty and conflicting claims. Use only \
    select_recall_items with selected_ids, or return that JSON object. Select an empty list \
    when nothing helps. The host retains required constraints and warnings, and checks \
    procedure standing. The packet grants no execution authority.";

pub(crate) fn budgeted_recall_prompt(policy: &str, selection_only: bool) -> String {
    let policy = if selection_only && policy == RECALL_DEFAULT {
        RECALL_DEFAULT
            .split_once("\n# A.")
            .expect("compiled Recall deployment boundary")
            .0
    } else {
        policy
    };
    let mode = if selection_only {
        PACKET_SELECTION
    } else {
        BUDGET_MODE
    };
    format!(
        "{}\n\n# Host budget mode\n{mode}",
        system_prompt(PromptTarget::Recall, policy)
    )
}

/// Which agent receives a deployment-specific configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptTarget {
    Formation,
    #[default]
    Recall,
    Maintenance,
}

impl PromptTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Formation => "formation",
            Self::Recall => "recall",
            Self::Maintenance => "maintenance",
        }
    }

    pub fn default_prompt(self) -> &'static str {
        match self {
            Self::Formation => FORMATION_DEFAULT,
            Self::Recall => RECALL_DEFAULT,
            Self::Maintenance => MAINTENANCE_DEFAULT,
        }
    }
}

/// Shared immutable compiled defaults. Retained as the default-prompt getter
/// for existing integrations; it can no longer observe a global override.
static DEFAULTS: [OnceLock<Arc<str>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];

fn slot(target: PromptTarget) -> usize {
    match target {
        PromptTarget::Formation => 0,
        PromptTarget::Recall => 1,
        PromptTarget::Maintenance => 2,
    }
}

pub fn active_prompt(target: PromptTarget) -> Arc<str> {
    DEFAULTS[slot(target)]
        .get_or_init(|| Arc::from(target.default_prompt()))
        .clone()
}

/// Trusted, immutable instance configuration. Only the deployment section can
/// be supplied by the host; protocol policy is always the compiled reference.
/// This is Rust configuration, never an agent tool or a public HTTP payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentPrompts([Arc<str>; 3]);

impl Default for AgentPrompts {
    fn default() -> Self {
        Self([
            active_prompt(PromptTarget::Formation),
            active_prompt(PromptTarget::Recall),
            active_prompt(PromptTarget::Maintenance),
        ])
    }
}

impl AgentPrompts {
    /// Replaces only the selected deployment's section A in this value.
    /// Other instances and the compiled default getter remain unchanged.
    pub fn with_deployment_section(
        mut self,
        target: PromptTarget,
        section: &str,
    ) -> Result<Self, BoxError> {
        if !section.starts_with("# A.") || section.len() > 128 * 1024 {
            return Err("deployment prompt must start with # A. and fit within 128 KiB".into());
        }
        let (reference, _) = target
            .default_prompt()
            .split_once("\n# A.")
            .ok_or("compiled prompt is missing its deployment boundary")?;
        self.0[slot(target)] = Arc::from(format!("{reference}\n{section}"));
        Ok(self)
    }

    pub fn prompt(&self, target: PromptTarget) -> Arc<str> {
        self.0[slot(target)].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_selection_keeps_the_protocol_policy_and_custom_deployment_contract() {
        let (reference, _) = RECALL_DEFAULT.split_once("\n# A.").unwrap();
        let custom = AgentPrompts::default()
            .with_deployment_section(
                PromptTarget::Recall,
                "# A. Host policy\nCUSTOM_SELECTION_RULE",
            )
            .unwrap();
        for selection_only in [false, true] {
            let prompt =
                budgeted_recall_prompt(&custom.prompt(PromptTarget::Recall), selection_only);
            assert!(prompt.contains(reference));
            assert!(prompt.contains("CUSTOM_SELECTION_RULE"));
            assert!(prompt.contains(anda_kip::KIP_SYNTAX));
            assert!(budgeted_recall_prompt(RECALL_DEFAULT, selection_only).contains(reference));
        }
    }

    #[test]
    fn all_modes_include_complete_versioned_syntax_and_only_their_role_cards() {
        for target in [
            PromptTarget::Recall,
            PromptTarget::Formation,
            PromptTarget::Maintenance,
        ] {
            let reference = mode_reference(target);
            assert_eq!(reference.matches(anda_kip::KIP_SYNTAX).count(), 1);
            assert_eq!(reference.matches(anda_kip::KIP_VOCABULARY_CARD).count(), 1);
            assert_eq!(
                reference
                    .matches(anda_kip::COGNITIVE_MEMORY_PROFILE)
                    .count(),
                1
            );
            assert!(reference.contains(crate::kip_reference::INSTRUCTIONS));
            assert!(reference.contains(crate::cognitive::CAPABILITIES));
            for (card, expected) in [
                (anda_kip::KIP_RECALL_CARD, target != PromptTarget::Formation),
                (anda_kip::KIP_FORMATION_CARD, target != PromptTarget::Recall),
                (
                    anda_kip::KIP_MAINTENANCE_CARD,
                    target == PromptTarget::Maintenance,
                ),
            ] {
                assert_eq!(reference.matches(card).count(), usize::from(expected));
            }
        }
    }

    #[test]
    fn every_system_prompt_preserves_language_and_policy_across_deployment_overrides() {
        for target in [
            PromptTarget::Formation,
            PromptTarget::Recall,
            PromptTarget::Maintenance,
        ] {
            let defaults = AgentPrompts::default();
            let changed = defaults
                .clone()
                .with_deployment_section(target, "# A. Test deployment\nCUSTOM_CONTRACT")
                .unwrap();
            for prompts in [&defaults, &changed] {
                let policy = prompts.prompt(target);
                let system = system_prompt(target, &policy);
                assert!(system.starts_with(language_reference()));
                assert_eq!(system.matches(anda_kip::KIP_SYNTAX).count(), 1);
                assert_eq!(
                    system.matches(anda_kip::COGNITIVE_MEMORY_PROFILE).count(),
                    1
                );
                assert!(system.ends_with(policy.as_ref()));
                let compiled_policy = target.default_prompt().split_once("\n# A.").unwrap().0;
                assert!(system.contains(compiled_policy));
                assert!(
                    system.find(crate::cognitive::CAPABILITIES).unwrap()
                        > system.find(anda_kip::KIP_SYNTAX).unwrap()
                );
            }
        }
    }

    #[test]
    fn executable_syntax_and_role_card_examples_parse_with_the_pinned_engine() {
        // `text` fences contain metasyntax. Only `kip` fences promise complete
        // executable commands; parameters and permissions are resolved at runtime.
        for document in [
            anda_kip::KIP_SYNTAX,
            anda_kip::KIP_RECALL_CARD,
            anda_kip::KIP_FORMATION_CARD,
            anda_kip::KIP_MAINTENANCE_CARD,
        ] {
            let mut examples = 0;
            for block in document.split("```kip\n").skip(1) {
                let (command, _) = block.split_once("```").unwrap();
                anda_kip::parse_kip(command)
                    .unwrap_or_else(|error| panic!("invalid KIP example: {error}\n{command}"));
                examples += 1;
            }
            assert!(examples > 0);
        }
    }

    #[test]
    fn instance_prompt_edits_preserve_compiled_reference_and_other_instances() {
        let baseline = AgentPrompts::default();
        let changed = baseline
            .clone()
            .with_deployment_section(PromptTarget::Recall, "# A. local\nINSTANCE_ONLY")
            .unwrap();
        let reference = RECALL_DEFAULT.split_once("\n# A.").unwrap().0;
        assert_eq!(
            changed
                .prompt(PromptTarget::Recall)
                .split_once("\n# A.")
                .unwrap()
                .0,
            reference
        );
        assert!(
            changed
                .prompt(PromptTarget::Recall)
                .ends_with("INSTANCE_ONLY")
        );
        assert_eq!(
            baseline.prompt(PromptTarget::Recall).as_ref(),
            RECALL_DEFAULT
        );
        assert_eq!(active_prompt(PromptTarget::Recall).as_ref(), RECALL_DEFAULT);
        assert_eq!(
            changed.prompt(PromptTarget::Formation),
            baseline.prompt(PromptTarget::Formation)
        );
        assert!(
            baseline
                .clone()
                .with_deployment_section(PromptTarget::Recall, "replace reference policy")
                .is_err()
        );
        assert!(
            baseline
                .with_deployment_section(
                    PromptTarget::Recall,
                    &format!("# A.{}", "x".repeat(128 * 1024))
                )
                .is_err()
        );
    }
}
