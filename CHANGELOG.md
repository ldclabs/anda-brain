# Changelog

All notable changes to the Anda Hippocampus project.

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
