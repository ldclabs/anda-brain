# Changelog

All notable changes to the Anda Hippocampus project.

## [0.5.4] — 2026-05-17

### Dependencies
- `anda_engine` 0.12.8 → 0.12.12.

**Engine changelog (cumulative 0.12.9–0.12.12):**

| Version | Summary |
|---------|---------|
| **0.12.9** | `steering_message` / `follow_up_message` upgraded from `Vec<String>` to `Vec<ContentPart>` — multimodal passthrough for steer/follow-up content. |
| **0.12.10** | `implicit_context` — injectable one-shot context that doesn't persist in message history. Fixed prompt ordering (system messages now consistently first) across all 4 providers (Anthropic, Gemini, OpenAI, OpenAIv2). |
| **0.12.11** | Prevent `implicit_context` injection on tool-call turns (only injects when assistant actually responds). **DeepSeek compatibility**: skip `tool_choice` parameter for DeepSeek models (API doesn't support it). |
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
- Bump all components to 0.5.0: `anda_hippocampus`, `anda-cli`, `anda-hippocampus-openclaw`.
