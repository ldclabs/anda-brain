# Changelog

All notable changes to the Anda Brain project.

## [0.6.10] — 2026-06-07

### Changed
- **CI now validates the workspace on Linux, Windows, and macOS.** The GitHub Actions test job now uses an OS matrix, installs `protoc` per runner, and runs clippy plus workspace tests on all three platforms.
- **Dependencies updated for cross-platform runtime fixes.** `anda_core` 0.12.7 → 0.12.8 and `anda_engine` 0.12.30 → 0.12.32, picking up the latest platform-aware runtime support; transitive `bitflags` updated to 2.13.0.

## [0.6.9] — 2026-06-06

### Changed
- **KIP syntax guidance updated for RC7-compatible value handling.** Brain prompt assets now document JSON-compatible KIP values, unquoted identifier object keys, parameter placeholders in complete KIP value positions, `SEARCH` parameter forms, optional proposition handles, and the registered `belongs_to_class` predicate.
- **Brain Formation and Maintenance metadata discipline tightened.** Write templates now consistently include `created_at` alongside `source`, `author`, `confidence`, and `observed_at` where applicable.
- **Contradiction and decay workflows now update matched proposition IDs.** Formation and Maintenance examples first retrieve existing proposition IDs, then use `(id: :link_id)` updates to avoid accidentally creating missing historical links while marking facts superseded or decayed.
- **Brain Maintenance append patterns clarified.** Maintenance logs now use read-merge-write arrays instead of overwriting with a single-entry array, and confidence decay queries/updates are aligned with current KIP semantics.
- **Brain Recall ranking guidance aligned with current KIP ordering.** Contextual briefing now uses a single `ORDER BY` expression and instructs Recall to synthesize strongest-first ranking from returned evidence fields.
- **Dependencies updated.** `anda_cognitive_nexus` 0.7.19 → 0.7.20, `anda_core` 0.12.6 → 0.12.7, `anda_engine` 0.12.28 → 0.12.30, `anda_db` family patch releases, `anda_kip` 0.7.13 → 0.7.14, `anda_object_store` 0.3.3 → 0.3.4, plus minor `chrono` and `log` bumps.
- **Service startup and shutdown paths split into testable units.** CLI parsing, model configuration, object-store selection, CORS setup, router construction, and cancellation-driven service shutdown now have focused coverage without changing the public command-line surface.
- **Repository agent guidance added.** `AGENTS.md` now documents workspace layout, verification commands, Brain invariants, and API/doc synchronization expectations for future coding agents.

### Fixed
- **Space creation now persists metadata before returning.** Newly created spaces close the initialized database after saving metadata, ensuring owner and tier extensions are durable for subsequent opens. Idle eviction now closes spaces instead of only flushing them so resources are released consistently.
- **Formation and Maintenance history retention now records completed conversations only.** Shared history buffering ignores in-progress conversations and caps retained context deterministically, avoiding transient conversations leaking into later agent context.
- **Formation retries now clear stale failure reasons after success.** A conversation that previously failed but later completes now persists a null `failed_reason`, preventing old error text from lingering on successful runs.
- **BYOK updates now validate model configuration before persistence.** Invalid model settings fail before replacing stored BYOK configuration or mutating the runtime model registry.
- **External cancellation now participates in graceful shutdown.** Service shutdown can be driven by the cancellation token as well as OS signals, making runtime shutdown deterministic in tests and embedded callers.

## [0.6.8] — 2026-06-04

### Added
- **Test coverage for core Brain modules.** Added unit tests across 9 modules: Formation/Maintenance `ProcessingGuard` lifecycle, Recall KIP function definition and timeout, `AnyHost` matching, ED25519 public key parsing (trim, validation, comma-separated), `markdown_to_html` GFM tables and raw-HTML preservation, `StringOr` and `HeaderVals` X-Shard extractors, `SpaceEntry` initialization and `touch`, `ModelConfig` compact alias deserialization and engine conversion, compact ref serialization, double-encoded `InputContext` JSON strings, `MaintenanceScope` `FromStr`/`Display` roundtrip.

### Changed
- **Dependencies updated.** `anda_core` 0.12.4 → 0.12.6, `anda_engine` 0.12.24 → 0.12.28, plus minor bumps (bitflags, hyper, uuid, zerocopy, etc.).

## [0.6.7] — 2026-05-30

### Added
- **`PayloadFormat` struct** separating request `ContentType` detection from response serialization format. Request format now respects `Content-Type` header only; response format honors `Accept` header independently.
- **Conversation delta endpoint.** `GET /v1/{space_id}/conversations/{conversation_id}/delta` route for incremental conversation sync.
- **`daydream` maintenance scope.** New `MaintenanceScope::Daydream` variant for lightweight background processing.

### Fixed
- **KIP readonly auth scope corrected.** `execute_kip_readonly` was incorrectly requiring `Write` scope; changed to `Read` with `is_public` guard for space token verification.
- **`SpaceTier::allow_nodes` overflow prevention.** Replaced unchecked `pow(2, tier-1)` with `checked_pow` saturating to `MAX`.
- **`MaintenanceInput.timestamp` now optional.** Added `#[serde(default)]` so callers can omit the field.

### Changed
- **Default content type changed** from `Markdown(false)` to `Json` for both missing `Content-Type` and missing `Accept` headers.
- **API docs, SKILL.md, READMEs** updated with new endpoints, `daydream` scope, and anda-bot usage example.

## [0.6.6] — 2026-05-29

### Changed
- **Formation now defers to active Maintenance.** `FormationAgent::process` and the idle-path both early-return when `BrainHook::is_maintenance_processing()` is true, letting Maintenance finish before Formation resumes.
- **Shutdown path now explicitly flushes all open spaces.** Cancellation collects entries first, avoiding iterator-invalidation while holding the read lock.
- **Idle eviction guard tightened.** `try_remove_idle_space` checks `Arc::strong_count` on both the `SpaceEntry` (≤2) and `Space` (≤1) before evicting, preventing races where a request is mid-flight.
- **Space idle timeout tightened** from 20 minutes to 9 minutes for faster resource reclamation.

### Added
- **`is_maintenance_processing` hook.** New `BrainHook` trait method; `Hooks` implementation delegates to `space.maintenance.is_processing()`. Formation uses it to queue safely during Maintenance runs.
- **`TimedMemoryReadonly` read-only wrapper.** A `Tool` implementation wrapping `MemoryReadonly` with a 15-second `READONLY_KIP_TIMEOUT`; on timeout it returns a `KipErrorCode::ExecutionTimeout` response instead of hanging.
- **Recall read timeout.** `Space::kip_readonly` now wraps KIP execution in `tokio::time::timeout(15s)`, converting hangs into structured timeout errors.
- **Async `MaintenanceAgent::set_processed_at`.** Switched from synchronous extension write to `save_extension_from(...).await`, matching the engine's async persistence layer.

### Fixed
- **User init routed through Formation.** `get_or_init_user` now calls `space.formation.get_or_init_counterparty()` instead of `space.memory.get_or_init_caller()`, aligning user identity with the Formation pipeline.
- **`Space.formation` visibility.** Changed from private to `pub` so external callers can reach it without going through `memory`.
- **Maintenance history retention.** In-memory history buffer now keeps the latest 2 entries (was 3), reducing transient memory footprint during long maintenance runs.

## [0.6.5] — 2026-05-29

### Changed
- **Dropped "(大脑)" Chinese annotations from Brain identity.** All three KIP prompts (`BrainFormation`, `BrainMaintenance`, `BrainRecall`) now refer to "Brain" without the parenthetical Chinese label — the name is self-sufficient.
- **Default `memory_tier` changed from `episodic` to `short-term`** in Formation's event encoding template. New events start as short-term and graduate to episodic only after Maintenance validates them.

### Added
- **Flashbulb salience encoding in Formation.** Phase 2 now supports setting an initial `salience_score` (60–100) for emotionally charged moments (corrections, breakthroughs, strong commitments) so they resist decay from the start.
- **Reinforcement (spacing effect) in Formation.** Phase 3 ("Deduplicate & Reinforce") now strengthens re-confirmed facts — bump `evidence_count`, refresh `last_observed`, nudge `confidence` upward (cap 0.99). The counter-force to Maintenance's decay.
- **Associative encoding in Formation.** Phase 5b now links new concepts to already-grounded related concepts via existing predicates, forming a connected web for better recall.
- **Flashbulb salience protection in Maintenance.** Scoring now refines existing `salience_score` rather than blindly overwriting — flashbulb memories are preserved.
- **`resolve_contradiction` task action in Maintenance.** New action for reconciling conflicting facts (supersede the older, strengthen the current).
- **Strength-aware (asymmetric) decay in Maintenance.** Reinforced memories (high `evidence_count`, recent `last_observed`, high `salience_score`) decay slowly; low-salience/unreinforced facts fade faster — "use it or lose it" pruning.
- **Pattern K — Contextual Briefing in Recall.** Assembles identity + preferences + recent Events + commitments + Insights into a single composite briefing for the common "what should I know before I respond?" query.
- **Memory strength ranking in Recall.** Reinforced facts (high `evidence_count` + recent `last_observed`) now sort first; tie-break by recency then confidence.
- **`ModelEffort` wiring.** `ModelConfig` and `ModelConfigRef` now support an `effort` field (`serde` alias `e`), wired through to the engine. `main.rs` defaults to `ModelEffort::High`.

### Removed
- Redundant KIP `SPECIFICATION.md` links from all three prompts — the runtime auto-injects the primer.
- `Keep the response short` instruction from Formation's output format section — unnecessary constraint on the model's response style.

### Dependencies
- `anda_core` 0.12.3 → 0.12.4.
- `anda_engine` 0.12.23 → 0.12.24.
- `anda_kip` 0.7.12 → 0.7.13.
- `anda_cognitive_nexus` 0.7.18 → 0.7.19.
- `hyper` 1.9.0 → 1.10.0.
- `candid` 0.10.28 → 0.10.29.
- `zerocopy` 0.8.48 → 0.8.49.
- `displaydoc` 0.2.5 → 0.2.6.
- `socket2` 0.6.3 → 0.6.4.
- `mio` 1.2.0 → 1.2.1.
- `cmov` 0.5.3 → 0.5.4.

## [0.6.4] — 2026-05-27

### Changed
- **SKILL.md relocated from `anda_brain/` to `skills/anda-brain/`.** The skill file now lives in the top-level skills directory alongside other agent skills. Updated `handler.rs` `include_str!` path and `README.md` link accordingly.
- **`MODEL_CONTEXT_WINDOW` default reduced** from 1,000,000 to 400,000 in `main.rs` — reflects the typical context window of currently used models.

### Fixed
- ASCII art box alignment across all docs (`README.md`, `README_cn.md`, `anda_brain/README.md`, `WEBSITE.md`, `WEBSITE_cn.md`).

### Dependencies
- `anda_engine` 0.12.21 → 0.12.23.
- `reqwest` 0.13.3 → 0.13.4.
- `http` 1.4.0 → 1.4.1.
- `log` 0.4.29 → 0.4.30.
- `memchr` 2.8.0 → 2.8.1.
- `serde-saphyr` 0.0.26 → 0.0.27.
- `sval` family 2.19.0 → 2.20.0.
- `granit-parser` 0.0.2 → 0.0.3.

## [0.6.0] — 2026-05-21

### Changed
- **Project renamed from `anda-hippocampus` to `anda-brain`.** All crate names, directory names, asset files, OpenClaw plugin, CI workflows, Docker images, systemd service, Cargo/pnpm workspaces, Go module paths, and documentation updated accordingly.

## [0.5.4] — 2026-05-17

### Dependencies
- `anda_engine` 0.12.8 → 0.12.12.

**Engine changelog (cumulative 0.12.9–0.12.12):**

| Version     | Summary                                                                                                                                                                                                                                                                                                                                                              |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **0.12.9**  | `steering_message` / `follow_up_message` upgraded from `Vec<String>` to `Vec<ContentPart>` — multimodal passthrough for steer/follow-up content.                                                                                                                                                                                                                     |
| **0.12.10** | `implicit_context` — injectable one-shot context that doesn't persist in message history. Fixed prompt ordering (system messages now consistently first) across all 4 providers (Anthropic, Gemini, OpenAI, OpenAIv2).                                                                                                                                               |
| **0.12.11** | Prevent `implicit_context` injection on tool-call turns (only injects when assistant actually responds). **DeepSeek compatibility**: skip `tool_choice` parameter for DeepSeek models (API doesn't support it).                                                                                                                                                      |
| **0.12.12** | **Tool output splitting**: multi-tool-output `Message`s now split into separate tool-role `MessageInput`s, each with its own `tool_call_id` (fixes protocol violation). **Message round-trip rewrite**: image/audio/file/video/refusal content parts preserved during `MessageOutput → Message` conversion (were silently lost). `msg.name` now survives round-trip. |

## [0.5.3] — 2026-05-16

### Dependencies
- `anda_engine` 0.12.6 → 0.12.8.

**Engine changelog (0.12.8):** Major release — Anthropic/Gemini types, OpenAI Responses API support, `TryFrom` MIME detection, SubAgent enhancements. Paired with `anda_core` v0.12.1.

## [0.5.2] — 2026-05-12

### Changed
- **User init routed through RecallAgent.** `get_or_init_user` now calls `space.recall.get_or_init_counterparty()` instead of `space.memory.get_or_init_caller()`, aligning user identity management with the recall pipeline.
- **`GetOrInitUserInput.user` type relaxed.** `user` field changed from `Principal` to `String` for broader caller compatibility.
- **`Space.recall` now `pub`.** RecallAgent is publicly accessible for user initialization and other external callers.

### Improved
- **Human-readable datetime in agent prompts.** Replaced `rfc3339_datetime()` with `local_date_hour()` across Formation, Maintenance, and Recall agents — `YYYY-MM-DD HH(AM/PM) ±TZ` format is more compact and readable for LLM context.
- **Prompt section labels consistently capitalized.** ("Your Notes", "Counterparty Profile", "Current Datetime").

### Removed
- **`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`** from Formation, Maintenance, and Recall agent instruction prompts — simplifies prompt structure without loss of context.

### Dependencies
- `anda_engine` 0.12.2 → 0.12.6.

## [0.5.0] — 2026-05-07

### Features
- **Robust InputContext deserialization.** `InputContext` now accepts both a JSON object and a JSON string (1–2 levels of nesting), so clients that serialize context as a string work correctly. The `user` field is accepted as a legacy alias for `counterparty`. The OpenClaw plugin mirrors this behavior with a `normalizeInputContext()` helper.
- **Invocation Discipline for recall_memory.** Formation and Recall agent instructions now explicitly state that `recall_memory` is for long-term memory only — agents should answer from local context for facts already present in the active conversation. Formation runs asynchronously and fresh memories may take a minute or more to become searchable.
- **ConversationDelta HTTP endpoint and CLI support.** Incremental conversation fetching via delta tokens, enabling efficient long-running agent conversations without re-fetching the full history.
- **Dynamic token limits.** The model's context window is now read at runtime and used to compute the output budget, replacing hard-coded constants.
- **Conditional review trigger.** Formation review now obeys KIP spec alignment — only fires when meaningful change is detected in the knowledge graph.

### Refactors
- **Full model output budget.** Recall agent now uses the complete output budget available from the model, with the minimum floor raised to 32k tokens.
- **Remove deprecated `prune_raw_history_if`.** Cleaned up obsolete pipeline calls from the engine migration.

### Fixes
- **Note tool extension key.** Fixed incorrect extension key reference in the note tool.

### Internal
- Upgrade `anda_engine` dependency path from 0.11.22 → 0.12.0.
- Migrate `EngineModelConfig` from `label` to `labels` field.
- Bump all components to 0.5.0: `anda_brain`, `anda-cli`, `anda-brain-openclaw`.
