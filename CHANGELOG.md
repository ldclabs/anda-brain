# Changelog

All notable changes to the Anda Brain project.

## [Unreleased]

### Documentation

- `README.md`, `README_cn.md` and `anda_brain/README.md` are rewritten against the
  current code: the release-by-release notes that had piled up in them are gone, the
  endpoint, MCP tool, configuration and `MemoryPolicy` tables match the service, and
  the vegetarian example now records a changed world as temporal succession rather
  than supersession. Anchors that other documents link to are kept.
- `API.md` and `API_cn.md` are reorganized: conventions, an Authentication section
  (admission classes, and the fact that `read` and `write` scopes do not include each
  other), the Memory Interface, the endpoint list with the runtime routes as 4.6, the
  MCP tools including the three runtime tools, errors, types, an example, the Rust host
  APIs and execution limits. Superseded version notes are removed; every contract
  detail is carried over unchanged.

## [0.13.4] — 2026-10-09

On the same KIP `11a82ec` / `cognitive-memory@2.0.0` stack.

### Fixed

- Formation, Recall and Maintenance no longer fail on Claude models. Through 0.13.3
  their `execute_kip` / `execute_kip_readonly` definitions from `anda_kip` 0.14.0
  carried a top-level `oneOf` (`command` versus `operations`), which Anthropic rejects
  with a 400 for the whole request. The release locks `anda_kip` 0.14.2, whose
  definitions use no schema combinators (a batch item is a `["string", "object"]` type
  list, and the `command` / `operations` rule is checked when the request is parsed),
  and `anda_engine` 0.16.8, whose Anthropic adapter also drops `oneOf`, `anyOf` and
  `allOf` from the top level of every tool schema.
- MCP tool schemas no longer contain `oneOf`, `anyOf` or `allOf`. schemars derived them
  from `Option<Struct>` fields, the untagged command and message-content enums and the
  tagged `AttentionResponse`, and model providers restrict them (Anthropic rejects them
  at the top level of a tool, strict modes anywhere). `AndaBrainMcpServer::new` now
  flattens each tool's schema: an optional struct field is its struct, alternatives of
  distinct types become one schema with a type list, and tagged variants become one
  object whose `kind` is an enum, with the variant-only fields optional and named per
  `kind` in the description. Arguments are still checked when they are deserialized,
  and a test keeps every MCP tool free of combinators.

### Dependencies

- `anda_engine` 0.16.8 (from 0.16.4), `anda_core` 0.16.3 (from 0.16.2),
  `anda_cognitive_nexus` 0.14.4 (from 0.14.2), `anda_db` 0.14.2 (from 0.14.1) and
  `anda_kip` 0.14.2 (from 0.14.0), under the existing requirements.
- `rmcp` / `rmcp-macros` 3.5.1 (from 3.5.0) and other lockfile-only patch updates.

### Known limits

- The embedded KIP reference supplement still names its source `anda_kip` 0.14.0
  (`assets/kip-reference/manifest.json`, `kip_reference::VERSION` and the Worker's
  `assets/kip-reference.json`). The documents it carries are byte-identical in
  0.14.2, so what the `kip_reference` tool serves is current, but
  `scripts/sync-kip-reference.mjs --check` against the locked 0.14.2 reports drift in
  `manifest.json` until the supplement is regenerated.

## [0.13.3] — 2026-09-29

On the same KIP `11a82ec` / `cognitive-memory@2.0.0` stack. A prompt and
tool-definition audit: instructions that contradicted the running system, and request
shapes that current Claude models reject.

### Changed

- Formation, Recall, Maintenance and budgeted Recall no longer force their first tool
  call (`tool_choice_required` is off). anda_engine sent it to Anthropic-protocol
  backends as `tool_choice: {type: "any"}`, which Claude Opus 5.5, Sonnet 5.5 and
  Fable 5.1 reject with 400. The prompts already name the tools to start with, and
  budgeted Recall still parses a selection returned as text. Budgeted Recall's
  normalized request digest changes with it.
- Wiki Digest asks for its extraction through `output_schema` (structured output on
  the Anthropic, OpenAI and Gemini adapters) instead of a "reply with ONLY JSON"
  instruction. The tolerant parser and the single retry stay for backends that ignore
  the schema.
- The Maintenance deployment contract (Rust and Worker) states the Memory Interface at
  `memory_basic` it actually serves; it said the opposite of the capability summary in
  the same system prompt. It no longer names Watch fields that exist nowhere
  (`due_seen_seq`, `delta_consumed_seq`) or a "former" Skill rule, and Formation and
  Maintenance no longer point the model at `memory_runtime syntax`, which returns the
  syntax already in its context.
- Recall's self-report asks for calibration without describing how it is audited, and
  says the answer example fixes the shape, not the length. The `recall_memory` query
  description drops its example queries; the MCP `anda_brain_execute_kip_readonly`
  description states its KQL/META-only gate, 15-second bound, belief semantics and
  when to use recall instead.
- `anda_brain/assets/RecallFunctionDefinition.json` is removed. No code loaded it and it
  lacked the required `budget` field; `RecallAgent::definition()` is the definition.
- SKILL.md no longer links the deleted OpenClaw plugin directory or sends readers to the
  discontinued console, and says `memory_strength_decay_factor` is accepted but ignored.

### Cloudflare Worker

- The Formation contract's examples write the counterparty key and Evidence fields as
  literals: the plan API never binds `:counterparty`, `:display_name`, `:evidence_key`,
  `:payload` or `:observed_at`. Its list of bound values now includes the Memory
  Interface's `:contexts`, `:scope_task` and `:orig`, and its example plan leaves the
  deprecated `types` / `predicates` empty, as the schema descriptions now say.
- The embedded-reference instructions say the full syntax is in context at every stage,
  not only planning.

### Dependencies

- `anda_engine` 0.16.4 (from 0.16.2), under the existing `0.16` requirement.
- `rmcp` / `rmcp-macros` 3.5.0 (from 3.4.1) and `tokio-rustls` 0.26.6 (from 0.26.5),
  lockfile updates only.

### Known limits

- Whether DeepSeek's Anthropic-compatible endpoint (the default `MODEL_API_BASE`)
  accepts `output_config.format` has not been verified; if it rejects the field, Wiki
  Digest fails there until the schema is made optional per provider.
- anda_engine's Anthropic adapter sets no `cache_control`, so the static system prefix
  is not prompt-cached on Claude. `MODEL_MAX_OUTPUT` defaults to 384000 for
  `deepseek-v4-pro`; a Claude model needs 128000 or less.

## [0.13.2] — 2026-09-27

On the same KIP `11a82ec` / `cognitive-memory@2.0.0` stack: Cognitive Nexus 0.14.2, and
a Recall tokenizer identity that names the tiktoken-rs 0.12 line. The CLI reports
0.13.2 and the Worker package is 0.1.2.

Upgrade notes:

- The first load of an existing Space under Nexus 0.14.2 upgrades its Nexus
  collection schemas and builds the `query_keys` and `lookup_key` indexes over the
  rows already stored. Let that load finish; do not cancel it.
- Do not load an upgraded Space with Brain 0.13.1 or earlier (Nexus 0.14.0). That
  version keeps the newer schema without maintaining the derived indexes, so rows it
  writes are missing from reverse `STRUCTURAL` and historical reads afterwards.
- Brain calls `anda_core` / `anda_engine` 0.16.2 APIs (the bounded note index) under
  a `0.16` requirement: a lockfile that still holds an older 0.16 release must update
  them.

### Changed

- The Recall tokenizer identity is `o200k_base@tiktoken-rs-0.12` (was
  `o200k_base@tiktoken-rs-0.12.0`): it names the o200k encoding of the tiktoken-rs
  0.12 line, which Brain now takes at 0.12.1 and admits patch releases of. Recall
  budgets, packets, receipts, the Memory Interface descriptor and the
  `recall_memory` tool schema report the new name. The old one is the same encoding
  and is still accepted from requests, Memory Interface budgets and stored
  `MemoryPolicy.recall_budget` values, and is read as the new name. The Worker
  advertises and accepts the same pair.
- The semantic Watch evaluator pin digests the tokenizer identity, so a host with a
  semantic Watch binding gets a new pin. As for any evaluator change, semantic
  Watches stop advancing ("semantic evaluator changed") until the operator reviews
  it, calls `reconfigure_evaluator` and re-arms the affected Watches.

### Dependencies

- `anda_cognitive_nexus` 0.14.2 (from 0.14.0), with the 0.14.1 query work. 0.14.2
  fixes `connect`, which adopted the handle it had opened, without setup, to detect a
  KIP 1.x layout. Under 0.14.0 every Brain Space, which is reopened after it is
  created, therefore ran its Concept collection without the Nexus index hooks and with
  the default tokenizer. Under 0.14.1 the same defect also kept new Concepts out of
  `query_keys`, so a `FILTER` on their name found nothing (a promoted draft type
  queried by its installed name returned nothing), and a Space that 0.14.0 wrote
  failed to load ("collection is open with schema version 0, but version 1 was
  requested"). Brain never used 0.14.1.
- `tiktoken-rs` 0.12.1, within 0.12.x (see Changed). Its o200k tables and encoder
  are unchanged from 0.12.0; the release drops `lazy_static` and a duplicate
  `fancy-regex`.
- `mimalloc` 0.1.52. Since 0.1.49 mimalloc v3 is the crate default (the `v3` feature
  is gone and `v2` opts back), so Brain requires 0.1.52 rather than any 0.1 release,
  which would let a lockfile resolve one that defaults to v2.
- `anda_kip` stays at 0.14.0 under a `0.14` requirement; the embedded reference
  supplement is generated from it. `scripts/sync-kip-reference.mjs` now checks its
  source against the `anda_kip` in `Cargo.lock` instead of an exact `Cargo.toml` pin.

### Fixes

- `handler::tests::runtime_handlers_cover_parse_auth_and_readonly_paths` waits for
  the scheduled maintenance that loading a Space starts once it has formed memory,
  instead of racing it with a manual maintenance request.

### Known limits

- Nexus 0.14.2 does not rebuild index entries written under 0.14.0. A Concept written
  after its Space was reopened keeps default-tokenizer BM25 terms, so `SEARCH` does
  not find a word inside its Chinese name, until an update rewrites the indexed
  columns; its empty optional keys also stay indexed. The upgrade builds `query_keys`
  for every existing row.

## [0.13.1] — 2026-09-27

`anda_brain` 0.13.1 on the same KIP `11a82ec` / `cognitive-memory@2.0.0` stack, now
requiring `anda_core` / `anda_engine` 0.16.2 and pinning `anda_cognitive_nexus`
`=0.14.0`; the CLI reports 0.13.1 and the Worker package is 0.1.1. A few public Rust
items were removed or narrowed (listed under Changed); Anda Bot uses none of them.

### Added

- `AppState::with_audience_free_cwt` lets an embedded single-Space host accept a
  trusted user's CWT that names no audience as addressed to the requested Space,
  for credentials issued before audiences existed (Anda Bot browser tokens before
  0.13). Signature, expiry, subject and scope are still verified and a token
  naming another audience is still refused. It is off by default: a shared service
  keeps refusing unscoped tokens.

### Changed

- Formation and Maintenance no longer copy the whole note store (up to about
  160K characters) into every system prompt. On `anda_engine` 0.16.2, which Brain
  now requires with `anda_core` 0.16.2, the runner supplies a bounded index of
  note ids and short excerpts (at most 8 KiB). Each runner loads it afresh,
  including the replacement after a compaction handoff, and it never enters the
  persisted conversation. The model pages full notes through the `note` tool's
  `read`, `list` and `search`, and the deployment prompts direct changes to
  `upsert`/`delete` by id, because `set` also replaces notes the model has not
  read. A note store that cannot be read now fails the pass, which is retried,
  instead of passing as empty.
- Recall reports its host-injected notes as unavailable when their store cannot
  be read or the lookup times out; budgeted Recall marks the `notes` channel
  omitted rather than checked. The legacy note-store fallback, which was reached
  only on such errors, is gone. Clearing processing Notes for a managed change
  now fails that change if the note tool refuses the reset.
- `anda_cognitive_nexus` is pinned to `=0.14.0`. 0.14.1 regresses lineage
  matching of promoted draft vocabulary: a draft type promoted onto an installed
  one no longer answers a query by the installed name.
- The product-change fence is a read/write lock. Agent KIP reads and writes,
  notes and runtime tools share it, so concurrent Recalls on one Space no longer
  queue behind each other's reads; managed changes, forget and source suppression
  still hold it exclusively.
- Attention reconciles each registered Space every 15 minutes by default
  (`AttentionPolicy::reconcile_ms`, was 60 seconds). Due Watches and dirty
  generations schedule their own checks; the frequent reconcile reopened cold
  Spaces that idle eviction had just closed. A pass also writes the directory
  cursor once instead of once per Space, and reads the directory root once.
- Memory Interface recall reads its five host channels concurrently, reads each
  element and belief once per briefing, and finds the largest briefing that fits
  the output budget by bisection instead of recounting after every dropped item.
  Budgeted Recall evicts optional planning items the same way.
- HTTP 400 messages are the error's plain text rather than its debug rendering,
  which quoted messages and showed body-parse failures as
  `RpcError { message: … }`. The Memory Interface helper endpoints put the KIP
  `ErrorObject` in `RpcError.data` and its message in `RpcError.message`.
- Removed `RecallAgent::with_schema_generation` (a no-op) and
  `memory_interface::caller_namespace` (use `Caller::namespace`).
  `payload::AppError::bad_request` takes `impl Display` instead of `impl Debug`.

### Fixes

- Formation `DEFINE` requests and `declare_memory_symbols` share a vocabulary
  lock around their quota check and writes, so concurrent tool calls cannot
  exceed the Space's 512-symbol cap. Ordinary memory reads stay concurrent.
- Memory Interface requests stay well within a 2 MiB worker stack. The intent
  dispatch, the recall pass and its channels, attention, basis and retention,
  and read-only KIP execution are polled behind type-erased boxes, so an
  optimizer can no longer fold them into one poll frame that keeps a slot for
  every await. An Anda Bot development build aborted its daemon on the default
  worker stack when an agent turn opened a session briefing (about 2.0 MiB
  along that path); the same path now uses about 137 KiB in the Bot's release
  build and about 1.05 MiB in a development build, where the Bot's own
  unoptimized frames dominate.
- The memory self-test no longer sends `max_output_tokens`: some OpenAI-style
  backends reject the parameter, which failed every self-test there. The output
  share is still reserved from the budget, as budgeted Recall already does.
- A Formation conversation that fails every attempt no longer blocks the queue
  forever. After three failed rounds (an attempt and its retry each) spanning at
  least 30 minutes, it is cancelled with `formation_failed:` and its last failure,
  its Memory Interface receipt reports it failed, and later conversations are
  processed. Shorter outages are still retried.
- Conditional updates of Memory Interface journal records (receipt settlement,
  forget settlement, staged-source erasure) give up after eight attempts and
  report the last storage error, instead of retrying a failing write forever.
- A forget now clears usage-ledger rows for exactly the elements PURGE reports
  erased, instead of enumerating a Concept's Propositions beforehand.
- A Memory Interface scope keeps its memory after Maintenance archives, tombstones
  or merges the scope's handle. To Maintenance the handle is an ordinary Event, which
  the reference policy archives when stale (as the retention sweep does when its
  expiry lapses). Resolution read only active Concepts, so recall treated the scope
  as unbound and every memory in it dropped out, including new observations, which
  kept writing to the retired handle. The handle is now found by its key in any
  lifecycle state short of purged.

### Cloudflare Worker

- Evidence capture classes a message by its role through the role table's own
  entries. Staged-source roles are not validated, and a role such as `constructor`
  or `__proto__` resolved through the object prototype to a non-string Evidence
  class; any role outside the table is now a plain `message`.
- A staged-source forget verifies each indexed receipt still belongs to that
  source. Retrying rejected feedback with the same key and a different source
  no longer lets the old source's forget erase the new source's Evidence.
- Recall starts its deadline before opening the Durable Object pass, so initial
  reads consume the caller's budget and an expired pass cannot start a model call.
- Maintenance no longer shows the Memory Interface's scope-handle Events to the
  model. An archived handle took every memory in its scope out of recall, and the
  maintenance policy archives stale Events. A handle that a plan's Event selection
  archives anyway still resolves by its key, as in Rust.
- `GET /v1/{space}/info` counts active elements only; purged stubs and archived
  records are no longer counted, so a forget lowers the counts.
- A Formation `DEFINE` past the 512-symbol cap, or a draft review that cannot be
  queued, answers 422 with the KIP error instead of 500.
- Memory Interface recall passes `budget.deadline_ms` to the model deadline, so an
  expired pass is cancelled instead of running (and billing) after the response;
  the stray timer is gone.
- Staged-source `observed_at` follows the Formation `timestamp` rule (RFC 3339 with
  an offset, at most millisecond precision); other `Date.parse` forms answer 400.
- An `execute_kip` batch containing `PURGE` or `PURGE PAYLOAD` fences in-flight
  agent passes and scrubs product previews, as `memory/forget` does.
- A blank `AI_TIMEOUT_MS` means the default rather than failing every request.
- Correction discovery and attention recall select superseded Assertions and
  raising Activities through indexed matcher columns instead of loading every
  Assertion and Activity (which also stopped at the engine's 50,000-element load
  ceiling). The predicate census is one SQL group-by over active Propositions.
- Formation, Recall, probe and Maintenance open with one Durable Object call each
  (admission, primer and first reads together) and Maintenance closes with one;
  a typical pass makes two or three RPC round trips instead of four to seven.
- Memory Interface briefings read each element once per call, resolve the scope
  once and find the largest budget-fitting item set by bisection. A staged-source
  forget reads that source's receipts from an index instead of every receipt;
  sources staged earlier keep the full scan.
- Suppressed sources are stored one key each instead of in the product control
  record, which every call parsed and which could outgrow a single value. A control
  record that still carries a non-empty inline list is refused
  (`unsupported_product_state`) rather than read as suppressing nothing.
- Initialization and recovery run once per Durable Object call rather than once per
  batched operation, and the gates share one parse per command.
- `BrainRpc` is derived from `AndaBrain`; duplicated helpers were merged.
- Retained recall bases are still kept indefinitely, as in Rust; an ErasurePlan's
  `external_exports` scans them all.

### CLI

- HTTP errors are read from the server's `{"error":{"message","data"}}` envelope.
  The client looked for a top-level `message`, so against a real server it only
  showed the raw body and a wiki commit conflict never exposed `current_version`
  through `HTTPError.RPC`. A non-JSON error body (a proxy page) is cut to 1 KiB.
- A Memory Interface session releases a receipt that a recall reports `failed`.
  Brain lists a failed receipt as pending; the session kept waiting on it, so one
  failed Formation left every later recall `pending` with `action_eligible: false`
  until the session file was edited by hand. The recall that reports the failure
  explains it. A `failed` intent always exits nonzero, and its printed error keeps
  `category`, `hint`, `retry` and `details`.
- RPC commands print the result as the server sent it. `info` and `status` had
  dropped the `memory_interface` descriptor; members the CLI does not model yet
  now appear as well.
- `help <command>` and shell completion no longer require `--space-id`.
- `management revoke-token` exits nonzero when no token matched.
- Batch Formation sends each file's path relative to `--batch-dir` as its source,
  the checklist's key, instead of the absolute local path.
- `execute-kip-readonly` accepts one bare KIP command, as the server does, and
  `memory sources stage` reads stdin like `formation`.
- The README documents that the CLI loads `.env` from the working directory and
  what a `.env` there can redirect.
- Removed the deprecated `maintenance --memory-strength-decay-factor` flag, which
  the server ignores. In the Go `api` package, removed the unused
  `MessageContent.SizeBytes`/`Text`/`FirstText`, the `KipCommandItem` and
  `KipCommandObject` aliases, the deprecated `MaintenanceParameters` decay and
  backlog aliases, and `MemoryError` (`MemoryResponse.Error` is now `*KipError`).
  `GetOrInitUser` returns the RPC error in the response like every other method.
  Content parts always encode their own `type`.

## [0.13.0] — 2026-09-25

**Breaking for draft data.** The service now tracks KIP `11a82ec` (following the
earlier `3251912` and `597db44` passes) and
`kip://profiles/cognitive-memory@2.0.0` (content digest
`sha256:734aa0fd93b6258d433a6f71915d9d5118552e2ea0458a3050d368dcfcc1d1b3`). The draft
rewrote 2.0.0 in place and dropped 2.1.0, so Spaces activated under the 2.1.0 draft
are not compatible and are not migrated automatically; use new Spaces, or move one with
the standalone `tools/migrate-draft-space` (below). KIP 1.x Spaces still upgrade
automatically, and a 1.x `Preference` keeps an open legacy type. Rust depends on
`anda_kip =0.14.0`, `anda_cognitive_nexus` 0.14 and `anda_engine` 0.16 from crates.io,
and the Worker on `@ldclabs/kip-do` 0.14 from npm.

### Memory semantics

- An ingested message is one Evidence per request (KIP §71.1). A Formation
  request with several writes is several transactions, and KIP now refuses it
  unless every ingest entry carries `client_key`. The host already keys each
  captured message, so a pass that writes in several statements still records
  each message once. The host classifies writes with `anda_kip`'s
  `Command::opens_write_transaction`, which also keeps an observation off a
  request whose only KML operation is a standalone `DEFINE`.
- World changes are one new Assertion from the change; the engine's temporal
  succession ends the older value, which keeps answering for its time. Supersession
  is reserved for a claim that was wrong. Formation prompts, the review pass and
  both deployment contracts say so, and claims carry `at:` from the cited message's
  observation time. Evidence takes a message's own `timestamp` when it has one, and
  the model receives the captured Evidence times.
- Formation `timestamp` is validated at the API boundary: an RFC 3339 instant is
  canonicalized to millisecond UTC; an unparseable value or sub-millisecond
  precision is rejected with 400 instead of silently becoming the receipt time.
- Options are Concepts typed by their kind; the Profile has no `Preference` type.
- New vocabulary is the Space's draft vocabulary (KIP §20.16). Formation sends
  `DEFINE CONCEPT TYPE` / `DEFINE PREDICATE` with literal names and bodies, in a
  request (Rust) or as plan commands (Worker) that the host runs before the writes
  using them; a name that already resolves (`SchemaSymbolConflict`) stops nothing.
  The host checks name shapes, caps a Space at 512 symbols of its own (drafts plus
  any legacy host package) and queues one `review_schema` SleepTask per new symbol,
  keyed `review_schema:<kind>:<ref>`. Maintenance may not define: its `DEFINE` is
  refused, Rust Maintenance no longer gets `declare_memory_symbols`, and the Worker
  reports Maintenance `types` / `predicates` as refused. `declare_memory_symbols`
  and the Worker `types` / `predicates` fields remain for one release as deprecated
  shortcuts that draft bare names with a host description; nothing is added to
  `kip://anda-brain/memory` any more, and Spaces that have it keep it in force. The
  wiki digest drafts its symbols the same way. The Worker `vocabulary` response
  gains `draft_package`, `draft_types`, `draft_predicates` and `defined`, and its
  `package_ref` is `null` for a Space without the legacy package.
- `GET /v1/{space_id}/schema/drafts` lists the drafts with their definitions and
  promotion targets, and `POST /v1/{space_id}/schema/promote` (`{kind, from, to}`)
  lets the owner promote one onto an installed symbol — management CWT on Rust, the
  API key on the Worker. The CLI adds `schema drafts` and `schema promote --kind`.
- The Formation gate refuses an Assertion citing a captured `:msgN` without `at`
  (or a written `valid.from`), except the Brain's own `mode: "inferred"` claims;
  both writing gates refuse a `MnemonicState.memory_strength` written without
  `last_metabolized_at` and `strength_policy`. The host binds `:strength_policy`
  (the standard `kip:strength-half-life-30d` pin) and `:now` on every model write
  and refuses a model-bound pin, so the engine can compute `effective_strength`.
- CLI message files may give each message's `timestamp` as Unix milliseconds or an
  RFC 3339 instant; each message keeps its own observation time.
- Decay is computed at read time from base, anchor and a pinned strength policy.
  The settlement sweep and its 0.5 default are gone; `memory_strength_decay_factor`
  and `decay_floor` are deprecated, still accepted and range-checked, and ignored.
  Settlement reports drop `decayed`, `decay_ran`, `decay_error` and
  `retention.expired_assertions` (claim expiry is computed, never stored), and
  metrics drop `decayed`.
- Model plans can no longer write a Skill's `current_trial` / `current_evaluation`
  or the computed `derived_from`, `compiled_from`, `compiled_by`, `consolidated_to`;
  `TrialState` is gone and `GradingState` is a computed view. The learning runtime
  moves the pointers in its verdict transactions instead of writing caches.
- The unconsolidated-backlog probe reads formation/consolidation Activity provenance
  instead of the computed `consolidated_to` field.

### Product and attention

- Product `ChangeKind` gains `WorldChange` (one new claim; the old one stays active)
  and `Misrecorded`, which fails `unsupported_capability` here — recording repair runs
  through the Memory Interface `revise` intent — and is never mapped to a correction. `Correct` now supersedes the
  old claim and keeps the world interval it covered instead of retracting it and
  starting the new value at the correction time. Both keep the record's
  `context_refs` (now on `MemoryRecord`), so a scoped claim is revised in its own
  context set. The Worker's product contract matches.
- `GET /v1/{space_id}/memory/attention` (Rust and Worker) returns fired Watches and
  due Commitments ordered by `(raised_seq, ref)`. The cursor is the last delivered
  position — `attention:<seq>:<ref>` inside a commit, `attention:<seq>` after a
  whole one, `attention:start` before any — so a commit raising more items than a
  page is no longer cut short; `attention:-1` is still accepted. `limit` counts
  items. The settlement raises each due `pending`/`blocked` Commitment without a
  Watch natively, with one `commitment_review` Activity keyed
  `commitment_review:<id>:<due_at>`: a replay raises nothing and a new `due_at`
  raises it again. The settlement report gains `commitments`. Reading is read-only
  and grants nothing. The authenticated runtime inbox is unchanged.

### Memory Interface (`memory_basic`)

- Both adapters serve the KIP 2.0 Memory Interface at the `memory_basic` level:
  `POST /v1/{space_id}/memory` takes one of `observe`, `recall`, `revise`,
  `feedback`, `forget` per request and answers a `Response`; failures ride inside it
  as KIP errors. `POST …/memory/sources` stages a captured source and returns its
  `source_ref`; `GET …/memory/receipts/{ref}`, `…/memory/sources/{ref}` and
  `…/memory/plans/{ref}` read progress, a staged source and an erasure plan. Rust MCP
  adds `anda_brain_memory`, `anda_brain_stage_memory_source` and
  `anda_brain_memory_receipt`. The descriptor is on `GET /info` and
  `GET /v1/{space_id}/info` (`memory_interface`), and the Nexus reports the same
  binding in `DESCRIBE CAPABILITIES` / `DESCRIBE PRIMER`. `requires` naming an
  unadvertised level fails `UnsupportedCapability`.
- Staged sources, idempotency keys, receipts, retained recall bases and erasure
  plans are scoped to the caller and the Space and survive restart. The same key and
  meaning replays the receipt without re-running extraction; a different meaning is
  `IdempotencyConflict`. Admission precedes capture, so bytes a forget excluded are
  refused.
- observe and revise run as Formation conversations carrying the intent. Progress is
  `recorded` until the pass completes, then `available` with the disposition the
  host reads from what the pass committed (`formed`, `evidence_only`, `skipped`); a
  source excluded before processing, a failed predecessor or an interrupted pass is
  `failed`. Scoped intake binds `:contexts` / `:scope_task`, refuses a scoped
  ASSERT without `context: :contexts`, and scopes the captured Evidence with a
  MemoryScope Facet.
- revise writes one history per `change_kind`: `correction` supersedes the actor's
  own claim; `world_change` and `unspecified` may not supersede or retract;
  `misrecorded` with a `target_ref` runs Nexus recording repair after the pass,
  with the replacements it wrote from the original source; without a target the
  report is kept as Evidence and the result is `partial`. feedback is host-captured
  Evidence classed by role (`agent_statement` for a self-report), never an Outcome.
- forget runs an ErasurePlan: `payload_only` purges Evidence payloads or a staged
  source's bytes; `semantic` (owner CWT) erases a claim's closure through the
  product deletion path, suppresses its sources and scrubs staged bytes, Formation
  and Recall transcripts, usage-ledger rows and the probe cache. `completed` needs
  the Nexus to validate the plan; holds are `blocked`, unverifiable sources
  `partial`.
- recall waits on `after` receipts, runs one Recall pass, and builds the Briefing on
  the host: cited claims are re-read through `BELIEF` under the scope, `valid_at`
  and `as_of_seq`; out-of-scope memory is dropped; constraints (Insights with
  `insight_class: "constraint"`) and open commitments are read exactly and never
  dropped for budget; failures, experiences and skills come from bounded searches;
  dependency caveats come from the items' own validity. `max_output_tokens` counts
  the serialized briefing under the advertised tokenizer (the Worker bounds by
  UTF-8 bytes, which never undercounts). The basis, coverage and per-channel
  RecallPlans are retained for `detail: "evidence"`, which reads the pinned element
  versions. `attention` and a minimal `resume` return the scoped attention page.
  Returned elements are logged as `retrieved` in the Nexus exposure log.
- The Maintenance assessment gains `exposures` (with `exposures_truncated`): the
  next bounded batch of the exposure log since the last cycle, tallied per
  element as `retrieved` / `used` counts, with a persisted cursor so a batch is
  counted once. It is the input for explicit, guarded reinforcement; the usage
  ledger stays an observation. Both Maintenance contracts say so.
- Formation and Recall prompt contracts gain the intent, scope, constraint and
  repair rules; the reference halves are unchanged. Product `Misrecorded` still
  fails `unsupported_capability` and points at the `revise` path.

- Trusted Rust hosts stage with `Space::stage_host_memory_source` and a
  `HostSource` (their product source identity and Formation context), so an
  embedding host's product deletion keeps covering what an observe forms; an
  excluded identity fails `SourceAdmissionError::Suppressed`.
  `Space::memory_receipt_state` returns a receipt's progress and its Formation
  conversation. Neither is exposed over HTTP or MCP. Anda Bot uses both.

### Memory Interface review fixes

- Staging retries without `observed_at` reuse the first capture time. Direct feedback
  and untargeted misrecording reports retain their requested MemoryScope.
- Recall audits returned Evidence as well as memory citations, replaces answers based
  on invalidated or out-of-scope reads, and retains the same snapshot used by host
  reads. Changes to the live search index make its coverage incomplete.
- Source forgetting includes newly created Events, Insights, Experiences and
  Commitments while preserving shared identity/option Concepts. Missing ownership
  traces or unfinished processing report partial coverage. Overlapping Assertion
  closures are deleted once, and Memory Interface work drains before database close.
- CLI sessions retain task/context scope. `memory forget --mode … --dry-run` is
  explicitly refused before sending a request because this interface has no preview.
- The standalone migration preflights reference closures before dropping collections;
  Assertions, Evidence and Activities that cannot resolve across capsules travel
  together, with each non-reusable element imported once.

### Draft-Space migration

- `tools/migrate-draft-space` (a standalone crate linking the 0.13 and 0.14
  engines) moves a Space written under the `cognitive-memory` 2.1.0 draft onto a
  fresh 2.0.0 Nexus in the same database: every element in any storage state is
  exported, references move to 2.0.0, each draft `Preference` becomes the option
  kind the owner chooses (a draft-vocabulary type), the written `derived_from`
  lineage becomes its `extraction` Activity, and the Nexus collections are rebuilt
  and imported in transaction-sized chunks with ids, keys, archived states and the
  self Concept preserved. The host's own collections and extensions stay. On a copy
  of the Anda Bot database it rebuilt 14,655 elements with no id change and a
  matching census; see its README for what is not carried.

### Maintenance of copied material

- Reference halves, the Worker's verbatim cards and the reference supplement were
  regenerated from `anda_kip` 0.14.0 by script; the supplement adds the Brain Runtime
  and Validated Learning companions and the common schema.
- The default Recall `context_tokens` rises from 32768 to 49152: the static Recall
  prefix alone is about 28k tokens after the synchronization.

### Fixes

- Core and Engine require at least 0.16.1, so downstream lockfiles cannot retain
  the 0.16.0 releases that use incompatible DB/KIP 0.13 types.
- The tokenizer is pinned to `tiktoken-rs =0.12.0`, matching the implementation
  version advertised by Recall budgets, receipts and the Memory Interface.
- The usage ledger's `last_self_test_at` (added after 0.12.1) is optional, so a
  Space created by 0.12.1 opens again: a schema upgrade may only add optional
  fields, and the required one refused the `memory_usage` collection.
- A KIP 1.x Space opens again. The one-time bookkeeping reset read the usage ledger
  under its stored schema and left that handle open, so the ledger's schema upgrade
  was refused and the Space failed to load. Verified on a copy of an Anda Bot
  database written by Brain 0.11.
- The lean library build no longer warns about a vocabulary helper only the wiki
  digest uses.

### Known limits

- The Memory Interface is advertised at `memory_basic` only. `memory_experience`
  needs a KIP-CognitiveMemory Nexus (computed GradingState and lineage views, and
  selection-dependency evaluation, are not built), and `durable_brain_runtime` is
  not declared. `resume` has no WorkingState; a Formation pass interrupted by a
  close is reported `failed` with an unknown outcome and not re-run; recording
  repair needs the extraction's source held inline; Recall transcripts are scrubbed
  by scanning every stored Recall conversation.
- `anda_brain/conformance/adapter.mjs` is a KIP harness engine adapter: it drives
  a real `anda_brain` process over the HTTP binding against a controlled model
  that forms nothing. KIP2-MIF-002, KIP2-MIF-005 and KIP2-REL-013 pass through the
  KIP runner (mechanism evidence); the other MIF/REL vectors depend on what a model
  extracts and were not run (no live provider run is recorded here).
- Capsule import mapping is not exposed by this service.
- A build with `wiki` (including the service binary) does not open a Space whose wiki
  collections Brain 0.11 created: they lack `generation` / `digest_pending`, which
  0.12.0 added as required fields. The lean library, which Anda Bot embeds, is not
  affected.
- Behavioral gains from these changes are not measured here (`not_run` until MIB).

## [0.12.1] — 2026-09-23

### CLI correctness and batch performance

- CLI help no longer exposes token, signing-key or provider-key environment values. Wiki JSON commits reject conflicting field flags, and exported bundles can be imported unchanged.
- JSON decoding preserves large integers in dynamic payloads and tool error markers. Structured Recall receipts, WikiDigest failures, conversation continuation metadata and agent output details survive CLI output. Reported business failures retain their JSON report and return nonzero exit status.
- Formation batch checklists bind the endpoint, Space and shard, detect changed content and retain unresolved failures. Submission receipts are labeled `submitted`; interrupted work requires explicit reconciliation before forced resubmission. Per-entry JSONL progress replaces repeated full-checklist rewrites, with restart replay and final JSON compaction. Dry runs do not write state.
- Commands return errors through Cobra, propagate output failures and reject ignored arguments. RPC wrappers share decoding, the inaccurate client-only Formation token limit is removed, and the CLI reports the existing 0.12.1 release version. Regression tests cover actual command requests, outputs, exit status and batch restart behavior; no live model is required.

### Runtime correctness and bounded work

- Formation and Maintenance now share atomic writer admission, including settlement and automatic handoff. Maintenance claims transfer by ownership without leaked Arc references. Compaction consumes the configured model-turn allowance before another call can start.
- Model-call concurrency covers background work, compaction, WikiDigest and diagnostic judges; HTTP/MCP request admission remains separately bounded. Closing a Space cancels model waits and tracks remaining background orchestration, while admitted native settlement writes drain before database recovery and close.
- Eviction retains a failed-close owner for retry and closes outside the global Space map lock. Warm Space loads reuse completed attention discovery. Formation queries indexed pending candidates instead of reading every numeric conversation id, and queue read errors are no longer skipped.
- Self-test stores its own last-tested timestamp for 30-day retesting, counts the actual host request with the pinned tokenizer and reserves a requested output cap. This does not guarantee provider billing/tokenization, and absent usage stays unknown.
- Conversation/Wiki page reads use bounded concurrency; Recall packets borrow selected content and use indexed membership while retaining exact final token checks. Conditional JSON persistence shares bounded readback code; the obsolete usage dirty index and writes are retired without deleting compatibility fields.

### Recoverable memory product contracts

- Trusted Rust hosts can inspect Assertion-backed records and captured Evidence sources, prepare conditional corrections, suppress or purge a reviewed claim closure, and resume admitted changes after restart. Corrections append a new attributed claim; native history remains intact. Sources must match the captured Formation message or a confirmed correction receipt before they can authorize a managed change.
- Managed changes persist source exclusions and a processing epoch before native writes. Stale Formation, Maintenance, Notes, vocabulary and protected runtime mutations are fenced; Recall rechecks its epoch before returning content. Budgeted Recall excludes old processing history, and pre-change pagination must restart while newer continuations remain usable.
- Purges clear affected saved preview and correction text, discard uncommitted intents, and verify live Evidence before exposing a correction source. Direct Rust `/memory/forget` also clears affected product previews. Native legal holds and per-target versions remain enforced.
- Recipient-owned record Watches use native arming and authorized coverage. Cancellation archives the Watch; retries do not re-arm it or restore a revoked grant. Runtime configuration gains static validation and optional isolated-workflow learning readiness reporting.

### Worker parity and limits

- Model Sessions now exclude inactive elements before structural joins, Proposition-id reads and aggregation after managed changes; administrative audit access remains intact.
- Maintenance persists unacknowledged correction pages and run ownership, fences expired writers, rotates bounded native snapshot candidates and excludes terminal tasks. The model receives content, versions and a live primer; explicit review acknowledgements do not claim complete dependency coverage.
- Worker responses retain per-operation statuses and distinguish no-effect plans from successful changes. Missing model usage stays nullable with known subtotals. One configurable deadline covers all model stages and fences late output; provider errors use 502 and timeouts 504, with receipt-preserving post-write review failures remaining 422.
- Grouped predicate counts replace repeated per-predicate reads, vocabulary inspection reads only active package artifacts, and batch erasure performs one paged, recoverable preview cleanup that also clears expired draft content.

- The Worker includes complete KIP syntax in every model stage, a single receipt-preserving review for large Formation inputs, grounding before Recall planning, and explicit Evidence/Activity erasure with per-kind counts, including archived records.
- Trusted Worker RPCs provide source-backed records, reviewed changes, source fences, durable per-step recovery and processing-epoch checks. Native read limits apply to record projections. Explicitly bound record Watches support host-polled advancement and cancellation, including interrupted creation.
- The Worker uses `@ldclabs/kip-do` 0.13.2 while compiled protocol references remain pinned independently to `anda_kip` 0.13.1 and verified by hash. It still has no cross-operation atomic batch, semantic search, Wiki, budgeted Recall packet, background inbox delivery or learning execution runtime; a nonempty Recall `budget` is rejected.

Mechanism tests cover source identity, authority, retention, cancellation and restart recovery. They do not establish empirical model gains, production learning calibration or provider-cost completeness.

## [0.12.0] — 2026-09-20

**KIP 2.0 / CognitiveMemory 2.1 is a breaking release.** Anda Brain and its
Cloudflare Worker now use the 2.0 memory model. Existing natural-language Brain
endpoints remain, but direct KIP clients, response readers and persisted 1.x
spaces must follow the changes below. A Proposition records meaning; an
Assertion records an actor's stance and Evidence. Existence is not belief.

### Wiki refactor

- Wiki reads authorize and select content from one document snapshot; BM25 candidates are checked against current version, archive state and ACL. History and verification follow the published parent chain, excluding failed commits even after a successful retry.
- Separated publication/recovery, authorized reads, retrieval and source outlines. H1–H6 TOCs and section ranges no longer depend on chunk packing; sections include descendants and all text selectors are capped at 256 KiB. Expansion reads immutable source content, and retrieval anchors select a section that covers merged sibling chunks.
- OKF uses standard YAML parsing/serialization. Unknown values survive title/tag edits, quoted comma tags round-trip, and imports propagate field deletions while preserving ACLs and unrelated host metadata. Comments, ordering and scalar formatting are canonicalized. Wiki storage is for fresh Spaces; no preproduction wiki migration or compatibility layer is provided.
- WikiDigest remains opt-in. Durable document generations queue content and lifecycle changes, unchanged bodies reuse prior ledgers, and concurrent edits fence stale graph writes. A bounded extraction omission cannot retract a claim: every source batch must explicitly review it as absent. Failed documents remain queued, appear in `WikiDigestReport.failed`, and rotate through the bounded retry scan without starving later work; source withdrawal retracts document-owned Assertions without touching other sources. Graph reconciliation remains asynchronous, and tests use deterministic model substitutes rather than establishing empirical extraction quality.
- Native wiki writes survive cancelled waiters and drain before database close. Regression coverage includes ACL changes during reads/extraction, failed-update history, OKF value preservation, full-batch withdrawal checks, pending work across restart, and cancellation followed by close/reopen.

### Upgrading a KIP 1.x space

- Ground budgeted Recall in a bounded search for the actual question before
  selection. Return compact, provenance-bearing views and individually packed
  candidates rather than repeated legacy payloads; only unresolved commitments
  consume mandatory space.
- Deliver explicit partial host-read results when planner input cannot fit,
  while retaining required constraints, warnings and native procedure checks.
- Use published Nexus 0.13.4's guarded repair for previously migrated completed
  Commitments; historical completion does not become a blocked obligation.

- Local source builds include the Nexus migration fixes for staging sets over
  1000 rows and legacy terminal SleepTasks. Lease-less historical running,
  completed and failed tasks become blocked; their original status and result
  remain in LegacyRecord. Regression coverage includes reopening those tasks.

- Stop the 1.x writer, take a consistent backup and rehearse on a copy. Each
  space migrates on **first access**, replacing its 1.x graph collections in
  place. The migration is one-way; rollback requires the original backup.
  It checkpoints extraction and vocabulary so interrupted runs can resume.
- The migration preserves original rows in `kip_legacy_v1` and `LegacyRecord`,
  maps recorded claims, valid time, retention, pinning and mnemonic values,
  and retains conversations, policies, tokens and wiki data. Reconstructible
  same-actor corrections become native supersession; ambiguous old corrections
  remain auditable rather than becoming current belief. Unsupported learning
  and operational artifacts keep Legacy identities without acquiring standing
  or leases. It does not invent Evidence, verified identity, trust or authority.
  Malformed identifiers and unresolved references can still require repair.
- Stored 1.x element ids such as `C:7` and `P:11:has_allergy` must be
  re-resolved; 2.0 uses ids such as `C-7`, `P-11` and `A-3`. Old-id usage,
  derived metrics and miss caches reset once. The migration is tested against
  an object-store snapshot produced by the published 0.11 packages. See the
  [upgrade guide](anda_brain/README.md#upgrading-a-space-written-by-a-kip-1x-build).
- Pre-release 2.0 development spaces with an older same-version Cognitive
  Memory Profile may need recreation if package installation reports
  `DigestMismatch`. The Worker has no 1.x Durable Object migration and starts
  with a clean KIP 2.0 store.

### Breaking KIP and API contracts

- Belief questions use `BELIEF` projection; raw `FIND` remains for audit.
  `insufficient` is not "no". Corrections create a new Assertion and
  supersede the old one instead of rewriting a claim. Assertion confidence
  does not decay with time.
- Schema is protected, versioned control state rather than graph content.
  Each space activates the Cognitive Memory Profile and its own
  `kip://anda-brain/memory` package. Formation and Maintenance can propose
  types and predicates through the host's `declare_memory_symbols` tool;
  Recall cannot publish vocabulary, and KML cannot declare it.
- `POST /v1/{space_id}/execute_kip_readonly` accepts the KIP 2.0
  `{command}` or `{operations}` envelope (also a bare JSON command string).
  Responses have top-level `status`, per-operation `results[]` and optional
  request-level errors. Callers must check per-operation status, including
  `no_effect` and `skipped`, and the top-level `outcome_unknown`; the absence
  of an error does not prove a commit. Error codes are named registry values
  such as `NotFoundOrNotVisible`, which does not distinguish absence from
  invisibility, with `retry.class` rather than `KIP_` numbers.
  Read-only and model write gates inspect parsed commands, not labels.
- `Concept` now uses `schema_ref`, immutable `key`, `facets` and `_system`
  instead of 1.x `type`/`metadata`; `name` remains a mutable label.
  `get_or_init_user` matches on `key`. The `unsorted` graph counter becomes
  `unconsolidated`; orphan and predicate-type counters follow the 2.0 model.
  Maintenance settings become `memory_strength_decay_factor` and
  `unconsolidated_max_backlog`; their old names still deserialize for stored
  policy compatibility.

### Formation, recall and maintenance

- Rust Formation keeps one semantic review for inputs of at least 10,000
  estimated tokens. Review now retains tool receipts, identifies the current
  conversation and captured Evidence window across compaction, and checks only
  material omissions or misrepresentation with targeted reads and minimal
  repairs. It accepts no changes, preserves uncertain outcomes and actor
  boundaries, and reports missing source coverage. This is a best-effort check,
  not an exhaustive-processing guarantee or measured real-model improvement.
  Formation now attaches ingestion only to requests containing a parsed
  mutation, so pure KQL/META grounding and review reads no longer fail for
  lacking an ingestion transaction. Readbacks use committed Evidence ids;
  `:msgN` remains a write-time binding.
- Every Rust agent system prompt now includes the full version-pinned KIP 2.0
  syntax from `anda_kip`, alongside its Profile, role cards and deployment policy.
  A shared assembler covers ordinary and budgeted Recall, Formation, Maintenance
  and experiment prompt identities, including instance-specific section A edits.
  Reference tools remain available for supplemental details. Syntax adds about
  40 KiB per request, counts toward cumulative Recall input budgets and does not
  expand host permissions. Regression fixtures check actual model requests and
  parse executable syntax/card examples; they do not establish real-model accuracy.
- Formation captures one Evidence record per input message from the received
  bytes, with a stable ingestion key and a bound `:msg1`…`:msg16` reference.
  Replays deduplicate instead of retyping, truncating or duplicating what a
  speaker said. Attribution remains separate from the authenticated caller.
- Disuse settlement changes `MnemonicState.memory_strength` on Concepts,
  never Assertion confidence. Recall records diagnostics and delivery provenance;
  retrieval alone does not reinforce mnemonic values. Pinning uses a retention
  class. Full Rust maintenance cycles
  expire lapsed Assertions and archive records past `retention.expires_at`,
  respecting legal holds. `POST /v1/{space_id}/memory/forget` performs an
  authorized `PURGE` and leaves an erased identity stub; archive and
  tombstone do not fulfill erasure.
- Correction discovery has a durable transaction/id cursor, reports incomplete
  pages and supplies revised roots with bounded `LIST DEPENDENTS` results to
  Maintenance. Vocabulary publication invalidates Recall's cached primer.
  Wiki withdrawal transitions exact document-owned Assertions under version
  checks; restoring even the same document version creates a new Assertion
  generation.
- Watch arming and advancement use protected Nexus state, generations,
  authorization coverage and version checks. Structured conditions advance
  from the Change Stream; a silence Watch fires only after complete coverage
  through its deadline. Text and mixed conditions remain deferred without a
  semantic evaluator. Completing a model call does not prove stream coverage.
  SleepTask completion likewise requires a live lease and guarded commit.
- Optional Recall budgets return a host-packed memory packet using the pinned
  `o200k_base@tiktoken-rs-0.12.0` counter. Required constraints and warnings
  precede optional content; packet and cumulative planning-input limits are
  enforced without claiming semantic completeness or execution permission.
  Requests without a budget policy retain ordinary Recall behavior.
- Budgeted Recall no longer sends a hard-coded `max_output_tokens` parameter,
  allowing backends that reject explicit output limits to use their provider
  defaults. Host packet and cumulative planning-input budgets remain enforced.
- Failed budgeted Recall packets include an optional static `failed_reason`
  when it fits the counted packet budget, distinguishing model/provider failure
  from token exhaustion. Tiny budgets retain the existing packet or `null`
  fallback. Provider error details are limited to 512 characters in local logs;
  they are not delivered in packets or stored in conversation diagnostics.

### Durable attention and authenticated outcomes

- Cognitive Nexus 0.13.1 atomically commits Watch firing, `watch_fire` Activity
  and protected wake, with replayable receipts and fenced leases. A persistent,
  bounded directory discovers registered Spaces after eviction/restart and
  advances structured Watches independently of Full Maintenance. Registration,
  dirty generations, fair cursors and owned writes preserve recovery and shutdown.
- Explicit host bindings connect wakes to `act`/`ask`/`defer`/`silence` decisions,
  immutable Decision/Attempt records and current-authority dispatch. Clarification
  uses a separate authorized child action; an answer never grants business
  permission. Unknown delivery keeps the same attempt and requires reconciliation.
- `BRAIN_RUNTIME_CONFIG` / `--runtime-config` installs compiled per-Space adapters
  and identity mappings before loading Spaces. The `attention_inbox_v1` adapter
  provides durable idempotent inbox delivery. Authenticated attention, response,
  outcome and runtime-status HTTP routes preserve JSON/CBOR/Markdown responses.
  MCP adds inbox read, response and status tools, with no observer/governance writer.
- Runtime channels require verified credentials even on public/local Spaces.
  Independent HTTP outcomes require signed observer credentials and current native
  `record_outcome`; ordinary Space tokens cannot self-grade. Receipts distinguish
  persistence, learning eligibility and unresolved safety work. Registered trial
  attempts retain a single native writer, fixed cutoff and idempotent recovery;
  late/conflicting/corrected inputs remain separately auditable.

### Optional learning, semantic evaluation and calibration

- Learning adds the compiled `workflow_http_v1` executor/observer/source adapter,
  bounded independent background advancement, persistent review and safety
  consumers, and terminal archival. `maximum_jobs` limits the hot working set;
  retained identities, replay, review and revocation survive archival. Automatic
  trials/reviews require actual bindings, approved calibration and explicit switches.
- Bounded Recall retains verifiable off-graph delivery receipts. Actual decisions
  bind delivered/used references; independent single-contribution witnesses or
  frozen paired-revision comparisons can calibrate Concept utility through atomic
  native receipts. Retrieved-only or inseparable bundled use receives no invented
  individual credit. Corrected evidence suspends ranking, and calibrated utility
  only orders peers within existing Recall priorities.
- Optional text/mixed Watch evaluation uses a pinned, bounded Chat Completions
  adapter or trusted Rust evaluator and native prepare/read/commit pages. Model
  calls run outside native locks and structured scans. Missing, unknown, truncated
  or mismatched judgments cannot advance coverage; governed artifacts support
  replay, erasure and lost-ACK recovery. Evaluator changes require explicit review.
- Optional contextual trust uses independently verified binary facts and exact
  actor/predicate/context proposals. Root/claim deduplication, explicit parameters,
  uncertainty, current `manage_trust` and native atomic proposal/control/audit
  application prevent unqualified or duplicate changes. Restoration appends a new
  governed version. Global weights, Assertion confidence and execution authority
  are preserved; bootstrap never grants trust governance.
- Runtime status distinguishes configured, enabled and currently authorized
  capabilities. Semantic/utility/trust automation defaults off; compilation,
  mechanism tests and calibration attestations do not establish empirical quality.
  See [runtime integration](anda_brain/RUNTIME.md) or the
  [Chinese edition](anda_brain/RUNTIME_cn.md).

### Learning, experiments and removed interfaces

- Skill behavior now lives in immutable `SkillRevision` records. Family
  membership discovers comparison candidates; it neither picks a baseline nor
  grants standing. Model plans cannot write trial, outcome, evaluation,
  grading or lease control state. Unproven candidates remain unproven.
- The opt-in Rust `learning` feature adds trusted frozen paired-trial
  contracts, native record adapters, a Nexus evaluator, persistent host trial
  runtime, fixed-cutoff settlement, review and safety revocation, and a
  read-only Recall applicability check. Actual executor and independent
  observer bindings are required; compiling the feature does not enable
  production dispatch or automatic adoption. The optional Memory Interface
  and its bundles are not advertised by either deployment.
- The opt-in `experiments` feature adds isolated stores, snapshots, business
  time, cost receipts, forced Recall budgets and bounded evaluator-only
  procedure audits. The Rust `anda_brain::eval` API, `eval` CLI, global
  prompt/policy overrides and bundled offline product fixtures are retired;
  [MIB](anda_brain/README.md#offline-regression-and-instance-configuration)
  owns product regressions. Online diagnostics, usage/correction ledgers,
  shadow reports and the independent wiki retrieval corpus remain. The
  `anda-brain-openclaw` package was removed.

### Cloudflare Worker

- The Worker runs an independent KIP 2.0 engine on `@ldclabs/kip-do` 0.13.1,
  with one SQLite Durable Object per space. Direct KIP accepts structured
  operations, parameters, `op_id` and `execution` modes `independent` or
  `sequence` with stop-on-error; `atomic` is refused because no transaction
  spans operations. Mutation outcomes live in
  `extensions["kip-do/outcome"]`.
- The Worker has host-owned vocabulary, keyword `SEARCH`, deterministic
  mnemonic settlement, correction discovery, protected structured Watches,
  task leases and `SET RETENTION`. It has no semantic/hybrid search, atomic
  operation batch or retention-expiry sweep. It does not provide the Rust
  service's wiki, MCP, CBOR/Markdown negotiation or asynchronous Formation
  queue. Model maintenance cannot `PURGE`, `PURGE PAYLOAD` or set a legal hold.
- Rust and Worker reference assets track published KIP 0.13.1. KIP timestamps
  use canonical UTC `YYYY-MM-DDTHH:mm:ss.SSSZ`; Worker input validation and
  regression fixtures now enforce that form. Bounded reference discovery and
  generated manifests remain aligned with the shipped protocol.

### Known limits and validation scope

- Rust bulk decay, correction discovery and self-test sampling still use
  full-scan KQL capped at 65,536 solutions. On larger graphs these passes can
  stop and report incomplete work; see the
  [maintenance notes](anda_brain/README.md).
- Runtime reports provide bounded counts and recovery reasons; complete aggregated
  stage latency, backlog age, coverage lag and cost metrics remain planned. Real
  learning gains, semantic error rates, utility attribution and trust calibration
  still require independent business-data acceptance. The executable next-work
  plan is in [English](VALIDATION_PLAN.md) and [Chinese](VALIDATION_PLAN_cn.md).
- KIP v2 has not been deployed; current acceptance uses fresh v2 Spaces and
  excludes pre-release old-data migration. Existing KIP 1.x compatibility remains
  documented above. One live owner per storage shard is still required.
- Runtime guides now have separate English and Chinese editions. Integration
  skills and contributor instructions describe actual capabilities without private
  implementation-stage identifiers.

### Fixes and release engineering

- The disk-backed HTTP learning fixture syncs its writable file handle before
  closing and replacing the snapshot. This fixes Windows reset failures caused
  by syncing a read-only handle, which previously surfaced as a timeout waiting
  for a model decision. The fixture now checks disk creation/replacement up front
  and reports scheduler state if dispatch still fails to reach the decision.
- Formation normalizes external RFC 3339 observation times, including offsets
  and fractional precision. Invalid or missing strings fall back to the durable
  conversation receipt time; retries retain the same Evidence identity. Markdown
  input is captured verbatim as one user message. Worker Formation/Maintenance
  use the same timestamp tolerance, with a per-request fallback.
- Counterparty lookups without a name preserve the existing Person display name;
  explicitly supplied names still update it.
- Rust `memory/forget` accepts explicit Evidence and Activity IDs in addition to
  Concepts, Propositions and Assertions. Per-kind deletion counts include native
  cascades; legal holds still prevent erasure. This does not scrub conversation,
  wiki or external copies.
- The exact `anda_kip = "=0.13.1"` dependency pin matches the generated reference
  manifest and restores the reference synchronization/check script.
- Search citations read the hit's nested `element`; probe misses are cached
  only after exhaustive search. `IS_NULL` on absent paths works, and bare
  `{"id": ...}` references no longer count as recalled memories. Both
  engines reuse Propositions staged earlier in the same transaction.
- Published Rust dependencies replace sibling `[patch.crates-io]` paths:
  `anda_kip`, `anda_cognitive_nexus`, AndaDB and `anda_object_store` use 0.13;
  `anda_core` and Anda Engine use 0.16; `cose2` uses 0.5 and
  `ic_cose_types` 0.11. The Worker installs published `@ldclabs/kip-do`
  0.13 from the pnpm lockfile. CI no longer checks out sibling runtimes.
- KIP 2.0 reference policies and role cards are synchronized with the
  published runtime; the Worker vendors and generates its prompt assets.
  Deployment-specific `# A.` sections remain separate. The Rust runtime
  supplies the syntax card and Profile from `anda_kip`.

## [0.11.0] — 2026-08-07

### Changed
- **The wiki and MCP surfaces are now optional cargo features, off by default.** The default `anda_brain` library is memory only — formation, recall, maintenance, and their HTTP routes. `wiki` adds the structured wiki (documents, versions, ACL-scoped reads, OKF import/export), the wiki agent tools, WikiDigest extraction, the `/v1/{space_id}/wiki/*` routes, and the `wiki_*` fields of `SpaceInfo`/`UpdateSpaceInput`; `mcp` adds the stdio and Streamable HTTP MCP servers. `sha3` and `unicode-normalization` are now pulled in only by `wiki`, and `rmcp` and `schemars` only by `mcp`. The MCP wiki tools live in their own tool router so `mcp` can be built without `wiki`.
- **The `anda_brain` binary declares `required-features = ["mcp", "wiki"]`.** It is the full product and its behavior is unchanged, but building or running it now requires `--features mcp,wiki`; Cargo skips the binary target without them. Every in-repo build command, Dockerfile, workflow, and doc was updated.
- **Anda engine dependencies upgraded to `0.15`,** along with `base64` 0.23, `cose2` 0.4, `http` 1.5, and `clap` 4.6. No HTTP, MCP, or storage contract changes.
- **Memory collections adapted to the engine's narrowed API.** `anda_engine::memory` no longer exposes the collections behind `MemoryManagement`/`Conversations`, so the space keeps its own handles to the conversation collections for the collection-level state the wrappers do not cover: the `brain_processed` and maintenance watermarks, conversation counts, and cursor paging. The space still opens every collection with its leaner index layout first, and the engine wrappers adopt those handles.
- **The brain's KIP tool definition moved to `agents::KIP_FUNCTION_DEFINITION`** and is installed on the writable `execute_kip` tool through `MemoryManagement::with_kip_function_definitions`, keeping `parameters` optional as before and keeping the read-only tool's arguments identical to the writable one's.
- **Release version advanced to `0.11.0`.** This release narrows the crate's public Rust API (breaking for library consumers); the HTTP and MCP wire contracts are unchanged except for the error-semantics fixes below.
- **Crate interface narrowed to what callers actually use.** The `wiki`, `ledger`, and `authz` modules are now crate-internal; `Space`'s handles (`db`, `formation`, `recall`, `memory`, `wiki`, `wiki_digest`) and 15 internal-only methods are no longer public; `payload` drops its unused RPC-request type and internal helpers; dead constants and the unwired retrieval-eval fixtures (now test-only) were removed or gated.
- **Space forking has a single entry point.** `AppState::fork_space` owns the whole fork protocol (object copy, state fork, load without background autostart), and `Space::close` is the public way to close throwaway spaces — the eval CLI no longer reaches into the DB handle.
- **Eval orchestration moved into the library (`eval::run`).** `EvalRunEnv`, run-scoped space hosting, suite and shared-formation runs, and the zero-score failure boundary now live in `eval/run.rs` with tests; `bin/main.rs` only parses flags and writes reports. The three hand-copied load→run→close sequences were unified behind one `with_eval_space` helper.
- **Shared test fixtures.** A `testkit` module replaces six verbatim copies of the test `AppState` wiring and space bootstrap across the unit-test suites.

### Fixed
- **Shared-formation eval forks no longer resume background work.** Profile forks previously loaded with background autostart, letting the inherited formation cursor and wiki-digest backlog burn model tokens and mutate forks mid-replay; forks now load through `fork_space` (autostart off), keeping A/B comparisons reproducible.
- **MCP errors mirror HTTP classification.** Caller-fixable failures (invalid input, guard rejections) now surface as JSON-RPC `invalid_params`/`invalid_request` instead of internal errors, and a wiki commit conflict carries the same `current_version`/`current_checksum` retry payload as the HTTP `409` body, so MCP agents can follow the documented re-read → merge → retry protocol.
- **`update_space_tier` reports an unknown space as `404`** (previously `400`), matching every other space endpoint.
- **Recall conversation snapshots no longer swallow serialization failures silently**; they are logged like the formation and maintenance paths.

## [0.10.2] — 2026-07-31

### Changed
- **Release version advanced to `0.10.2`.** `anda_brain` now reports `0.10.2`, and the lockfile was refreshed for the release.
- **AndaDB and Cognitive Nexus dependencies upgraded to `0.11`.** `anda_object_store`, `anda_db`, `anda_db_tfs`, `anda_cognitive_nexus`, and `anda_kip` now use the new release line.
- **MCP and HTTP dependencies upgraded.** The service now uses `rmcp` 3.0 and `tower-http` 0.7.

### Fixed
- **Database pagination now follows the AndaDB 0.11 query API.** Conversation and wiki listing, housekeeping, and ledger scans use ordered ID queries followed by targeted record reads, retaining bounded pages and stable cursors under the new API semantics.
- **Graph statistics remain compatible with Cognitive Nexus 0.11.** Space status and graph-counter fallbacks now use the public concept and proposition accessors.
- **Streamable MCP HTTP retains configured session behavior.** The migration maps the existing stateful-session setting to rmcp 3.0's legacy session mode, and KIP parameterized commands use the updated protocol representation.

## [0.10.1] — 2026-07-21

### Fixed
- **Wiki extraction now normalizes schema identifiers.** Extracted concept types are converted to `UpperCamelCase` and predicates to `snake_case` before KML is rendered, preventing malformed or duplicate KIP schema entries.
- **Orphan metrics now use a valid per-type census.** The graph-health sweep inventories registered concept types and totals their unassigned concepts instead of issuing an unsupported all-concepts query; failed legs return an unknown metric rather than a partial count.
- **KIP metric failures are visible.** Read-only count and orphan-census failures now emit warnings, and KIP string-literal escaping is centralized so search commands cannot drift.

### Changed
- **Release version advanced to `0.10.1`.** `anda_brain` now reports `0.10.1`, and the lockfile was refreshed for the release.

## [0.10.0] — 2026-07-16

### Changed
- **Release version advanced to `0.10.0`.** Upgraded `anda_db`, `anda_db_tfs`, `anda_object_store`, `anda_cognitive_nexus`, and `anda_kip` to `0.10`; migrated the MCP integration to `rmcp` 2.2 (`ContentBlock`); and refreshed the lockfile.

### Fixed (full-crate review, 8 P1 + 15 P2 + 30 P3 — see CODE_REVIEW.md)
- **Label ACL closed on the conversations channel.** `get/list_conversation` (HTTP and MCP) reject label-restricted tokens with 403 via a shared guard — recall conversations persist the unrestricted runner history, which bypassed the token's wiki ACL.
- **Token management hardened.** Minting a `*`-scope space token now requires a `*`-scope CWT; `list_space_tokens` redacts values to a display prefix; token `name` is required/unique and `revoke_space_token` accepts it as the revocation handle; mints are serialized against the count/uniqueness checks.
- **Read-only KIP errors are "unknown", never "absent".** `forget_memory` existence checks report an error entry instead of a clean `existed:false`; dream self-test skips (without stamping) candidates whose grounding search or concept lookup errored; eval assertion probes stop feeding errored searches to the judge as `satisfied:false`.
- **Formation/maintenance mutual exclusion has no settlement window.** `Space::maintenance` claims the processing slot *before* deterministic settlement and `MaintenanceAgent::run` inherits the claim, so a formation cycle can no longer start mid-settlement and write the graph concurrently.
- **Runner loops are bounded.** Formation/maintenance agent loops carry a 200-turn / 30-minute guardrail that fails the conversation into the existing retry path instead of holding the processing slot forever on a tool loop.
- **The optimizer no longer loses paid generations.** Proposal/parse failures record a rejected generation and continue; evaluation failures return (and `main` writes out) the partial report with all accepted genes; a prompt-genome guard mirrors the policy guard so unvalidated candidates cannot leak into the process-wide override. Rejections restore the run-start genome, not compiled defaults.
- **Eval harness degrades instead of aborting.** Transport-level recall/simulator/trace failures become findings; a failed scenario yields a zero-score placeholder report instead of discarding the suite; shape-mismatched judge output errors into the lexical fallback instead of scoring all-zero; blank rubric terms fail validation; empty-evidence probes short-circuit without a judge call.
- **HTTP layer: concurrency limits and honest errors.** Global tower load-shed cap (503) plus a stricter cap on LLM-billed routes (429); error bodies follow `Accept: application/cbor`; `/recall` honors `Accept: text/markdown`; a dead server task now cancels the process instead of leaving a zombie; nonexistent spaces map to 404 with internals kept out of response bodies; invalid `Shard-Id`/`X-Shard`, unknown `collection`, half-open ranges, and empty formation/recall inputs are 400s; `Bearer` parsing is case-insensitive and single-strip.
- **Wiki correctness and bounds.** `expand_hits` actually merges overlapping expanded hits (the bridge pass was dead code); `sweep_doc` reconciliation paginates past 1000 chunks; digest no longer marks a whole document superseded when racing a concurrent commit; atomic chunk units cap at 32 KiB; caller checksum mismatches are citation errors, not forged corruption events; restored documents re-enter the digest queue; OKF re-imports propagate tag deletion, imports are race-free under the write lock, exports are size-capped.
- **Assorted races and accounting.** `record_miss` checks the clear stamp under the write lock; `mark_flushed` tolerates rows removed by a forget cascade; `Space::update` performs fallible I/O before any in-memory mutation; per-run `MaintenanceParameters` validate against the policy bounds; the first `memory_status` census is single-flighted; failed recalls persist their real token usage; PII scrubbing preserves fractional-second timestamps; MCP auto-created spaces record the verified caller as owner; the MCP channel accepts JSON-string `context` and rejects unknown `wiki_search` modes.
- **Performance.** Context assembly, checkpoint samples/probes, shadow-eval forks/replays, wiki doc+TOC reads, and the schema census fan out concurrently (bounded); runner conversations persist every 5 turns instead of every turn (O(turns²) → O(turns)); wiki commit builds chunks outside the global write lock; `verify_recent` groups version loads; `prune_events` batch-derives digest ledger heads.

### Fixed (pre-launch review, memory evolution)
- **Usage counts can no longer be lost by settlement.** The reinforcement flush scans a `dirty` flag on ledger rows (schema v2) instead of a time-window watermark: rows whose KIP write fails, arrive past a batch limit, or are recorded concurrently with a settlement stay dirty and are retried by every later settlement. `mark_flushed` re-checks the row under the write lock so a recall racing the flush re-dirties it.
- **Correction discovery no longer starves behind its scan window.** Processed superseded links get a `correction_settled` graph marker and leave the result set — the marker is the cursor, so backlogs larger than one `LIMIT 500` batch drain across cycles instead of hiding all newer corrections forever.
- **Dream self-test coverage now slides across the whole graph.** Sampling excludes links already tested (`self_tested_at` stamp, 30-day retest horizon) or already recall-reinforced *in the query*, so every pass reaches new rows; previously the fixed lexicographic prefix was re-read until coverage stalled after ~4 cycles. Unresolvable candidates are stamped too. The self-test token budget now shrinks the candidate batch before the LLM call instead of warning after it.
- **Full-scan engine-cap failures are loud and partial, not silent and total.** A failing bulk-decay pass (e.g. KIP_4002 past the engine's 65,536-row full-scan cap) degrades with `log::error` and a `decay_error` report field while corrections and the schema census still run; the correction scan and self-test sampling log the same way. The ceiling and the pending predicate-sharding work are documented in README and the plan.
- **Decay can no longer push confidence through the floor.** The bulk-decay `CLAMP` lower bound is the policy `decay_floor` (was 0.0), so a 0.31-confidence link stops at 0.30 instead of landing permanently below the floor.
- **`forget` cascade is complete and cannot race maintenance.** Deleting a concept now enumerates its propositions first and removes their usage-ledger rows (their ids embed predicate names — usage traces of the forgotten memory); a successful forget clears the negative-knowledge cache (rows carry raw query text); and forget is rejected while a maintenance cycle runs, since the LLM's context could re-materialize the deleted entities.
- **Mined-scenario PII scrubbing covers every string field.** `scrub_scenario` round-trips the whole scenario JSON, reaching `required_answer_terms`/`forbidden_answer_terms`/`assertion`/`messages`/`scoring_rubric` — exactly the fields the miner LLM is told to put corrected facts in; a scenario that cannot be scrubbed is dropped. Emails are masked before digit runs, so `alice12345678@example.com` no longer half-leaks.
- **`MemoryPolicy` integer knobs are capped.** `self_test_queries_per_cycle` ≤ 100, token budget ≤ 1M, and bounds on the remaining integer fields: the policy is settable over HTTP, and an unbounded budget was a per-cycle cost bomb that only the optimizer path guarded against.
- **The negative-knowledge cache is bounded and fully invalidated.** Hard cap of 1024 rows with expired-row purge at the cap and a 512-char query limit (anonymous probes on public spaces can no longer grow it without bound); wiki-digest and maintenance graph writes now clear it like formation always did, so fresh memory stops being masked for up to an hour.
- **Shadow evaluation is isolated and serialized.** Forks open in no-autostart mode — they no longer resume the live space's formation backlog or wiki digest (double LLM spend, drifting A/B state); a per-space `shadow_lock` rejects concurrent runs (each holds two full in-memory space copies); the source space loads unpinned.
- **Dead knobs left the policy genome.** `recall_reinforcement`, `correction_penalty`, `recall_search_threshold`, and `recall_max_rounds` have no runtime consumers yet and were removed from `POLICY_PATCH_FIELDS` — mutating them measured pure sampling noise that the accept gate could bless as an "improvement". The optimizer also warns when running without variance data (`checkpoint_samples = 1`).

### Fixed (second review pass)
- **The recall footer cannot leak through side channels.** `split_recall_meta` strips *every* `<memory_meta>` occurrence (an echoed prompt example no longer reaches plain `/recall` clients) and keeps prose after an unclosed tag instead of discarding it (a truncated JSON footer is still dropped and salvaged); plain `/recall` also strips the footer from `chat_history` and `failed_reason`, which previously carried the raw model output.
- **Citations carry the provenance the plan promised.** `MemoryCitation` gains `source` and `created_at`, harvested deterministically from tool-output metadata like `confidence` already was.
- **Uncertainty calibration counts real traffic.** Plain `/recall` self-reports now feed `avg_uncertainty` (previously only `recall_structured` — a blind spot over most production traffic), and failed recalls no longer pollute the sample on either path.
- **`memory_status` no longer runs full scans per request.** Graph counters (orphans/unsorted/predicate types) are censused at settlement time into the `memory_graph_counters` extension (with `as_of`); the anonymous-reachable endpoint reads the cache. A failed per-predicate census count is now *omitted* from `schema_audit` instead of recorded as 0, which pointed the merge guidance at the busiest predicate.
- **Probe stops counting graph plumbing as memory.** Hits on meta-schema, domains, sleep tasks, and `$`-identities are filtered before `found` is decided — the engine's keyword fallback has no relevance threshold. Negative-cache keys are normalized (whitespace/case), and a `last_cleared_ms` guard drops misses whose search started before a concurrent cache clear (the miss could be answerable by the memory that just formed).
- **Self-test SleepTasks join the `System` domain** at creation, so each dream pass no longer inflates the orphan metric it reports on.
- **Shadow evaluation can actually measure decay knobs.** Fork settlements bypass the weekly decay rate limit (forks inherit the live `decay_applied_at` stamps, which made every decay comparison a systematic tie); the replay sample defaults to the policy's `shadow_replay_sample`; and the `JUDGE_MODEL_*` variables now configure an independent judge in *service* mode too (`AppState` installs it on every loaded space), so on-line verdicts stop falling back to the evaluated space's own model.
- **Optimizer gates hardened.** The holdout baseline is monotone (it was re-baselined downward on accept, letting N generations ratchet holdout down by N×ε); duplicate-field patch sets are rejected (chaining three patches on one field compounded past the ±50% step bound); and a drop-guard clears the process-wide policy override on any exit path, so a panic or early return cannot leak a candidate policy into later evals.
- **Mined scenarios never overwrite pending reviews.** Same-slug output files get a numeric suffix instead of silently replacing an earlier mined scenario awaiting human review.
- **Correction rates gain a denominator.** Full-scope settlements census per-source total link counts into `source_reliability.total_links`, turning raw correction counts into rates for encode-time source discounting (P3).
- **Auth-matrix tests for the six evolution endpoints** (`probe`, `recall_structured`, `memory_status`, `memory/pin`, `memory/forget`, `management/shadow_eval`): 401 when private, public-space bypass for reads only, and write/management gates verified end to end. BrainMaintenance.md §3.2 no longer showcases a bare bulk-decay `UPDATE` that contradicted Phase 7's "runtime-settled" contract, and the README documents the single-writer-per-space deployment assumption.

### Added
- **Shadow evaluation (evolution plan M11).** `POST /v1/{space_id}/management/shadow_eval` compares a candidate `MemoryPolicy` against the current one on the production distribution: the space forks twice into isolated in-memory stores, both forks settle under their policies, recent real recall queries replay on each, and the judge blind-compares answers with deterministic A/B alternation. The live space is only read (fork replays cannot touch its ledger/metrics — guardrail 4); the report persists in the `shadow_report` extension and promotion stays human via `update_space`.
- **Memory observability (`memory_status`, evolution plan M12).** `GET /v1/{space_id}/memory_status` returns incrementally-maintained counters (every evolution module bumps its own at write time — reads never run heavy queries), derived rates (probe hit rate, correction rate, mean self-reported uncertainty, maintenance-tokens-per-recall ROI proxy), graph counts, and the latest settlement/self-test/shadow reports.
- **Schema-metabolism census (evolution plan M8).** Full-scope settlements record a per-predicate link census into the `schema_audit` extension; `GraphStats`/`memory_status` gain a `predicate_types` schema-sprawl indicator, and BrainMaintenance.md Phase 6 now carries guarded predicate-merge guidance (bounded batches, core predicates untouchable, "unsure → review SleepTask, a wrong merge is worse than sprawl").
- **Policy genome optimization (evolution plan M10).** `anda_brain eval --optimize policy` evolves the numeric `MemoryPolicy` knobs: the optimizer LLM proposes 1–3 bounded mutations per generation (±50% steps, range-validated in code), candidates install through a process-wide eval policy override that run-scoped spaces pick up, and the accepted policy is written to `--optimize-out/memory_policy.json` for human review. `OptimizeConfig::default()` now also carries the documented noise-band floors (`min_delta` was silently 0 on the CLI path before).
- **Holdout gate for the optimizer (evolution plan M9).** `--holdout-scenario` runs a held-out suite whenever train accepts: candidates that improve train but regress holdout beyond `holdout_epsilon` are rejected as overfitting and reverted. Per-generation holdout totals are recorded in the optimize report.
- **Independent judge model (evolution plan M9).** `JUDGE_MODEL_*` env/CLI args route judge completions (checkpoint scoring, semantic assertion probes) through a separate model via the new `AssessContext::judge_complete`, installed on every run-scoped eval space including shared-formation forks. Judge scores stop sharing the evaluated system's blind spots.
- **Scenario mining (evolution plan M9).** `anda_brain eval --mine` distills an existing space's correction ledger into eval scenarios: superseded memories plus their source-conversation excerpts feed an LLM that writes correction-replay scenarios, strictly parsed, validated like hand-written fixtures, and PII-scrubbed (emails/long numbers masked on both LLM input and output). Mined files land in a review directory (`--mine-out`) outside the auto-validated fixture glob.
- **Dream self-test (evolution plan M7).** After each maintenance cycle completes, the runtime samples recent unused memories, generates one natural probe query per memory (single budgeted LLM call), and deterministically checks whether search surfaces them. Unfindable memories become pending `review` SleepTasks (source `memory_self_test`) with re-encode guidance for the next full cycle; BrainMaintenance.md Phase 2 documents how to process them. Self-test retrievals count only into the ledger's isolated `self_test_count`, the pass report persists in the `memory_self_test` extension, and the groundability rate surfaces as a new optional `GraphStats.groundability`.
- **Metamemory probe with negative-knowledge cache (evolution plan M5).** `POST /v1/{space_id}/probe` is an LLM-free existence check (hybrid/keyword search) returning `found` plus citation-shaped hits. Empty results are cached in the new `recall_misses` collection and answered from cache until formation completes (which clears the cache) or a 1h TTL expires — repeated dead-end queries stop costing graph work.
- **Pin and privacy-grade forget (evolution plan M6).** `POST /v1/{space_id}/memory/pin` marks entities `pinned` (exempt from confidence decay, already enforced by the M2 settlement); `POST /v1/{space_id}/memory/forget` physically deletes entities (concepts detach with their propositions; KIP_3004 keeps protecting system nodes) with `dry_run` preview and per-entity error reporting, cascading to usage-ledger rows. Archive does not satisfy forget.
- **Memory usage ledger (evolution plan M1).** Every completed recall records the graph entities its trace actually surfaced into the new `memory_usage` collection (`ledger::UsageLedger`): recall counts, last-recalled timestamps, and correction counts, with self-test counters reserved and isolated so the brain testing itself never counts as usage. Real usage is now the selection-pressure signal for memory evolution.
- **Deterministic usage-modulated metabolism (evolution plan M2).** Before each maintenance cycle the runtime settles memory metabolism in code (`Space::settle_memory_metabolism`): ledger counters flush onto graph metadata (`last_recalled_at`/`recall_count`), full cycles run the bulk confidence decay as a code-built KIP `UPDATE` (recently recalled, pinned, superseded, and system-truth links exempt; policy factor/floor; weekly rate-limited via `decay_applied_at`), and the report persists in the `memory_settlement` extension. BrainMaintenance.md Phase 7 now instructs the agent *not* to bulk-decay — only the semantic residue (re-confirmation, review flags) stays with the LLM.
- **Correction ledger and source reliability (evolution plan M3).** Settlement discovers newly superseded links, records them as corrections in the usage ledger, and aggregates correction counts per `metadata.source` into the `source_reliability` space extension — the raw material for encode-time source discounting.
- **Structured recall output (evolution plan M4).** New `POST /v1/{space_id}/recall_structured` returns `RecallOutput`: the answer plus trace-derived memory citations (entity id, type, name, confidence — never model-claimed), and the model's self-reported `found`/`uncertainty` from a new `<memory_meta>` footer contract in BrainRecall.md. The footer is stripped from all plain `recall` responses, so existing clients are unaffected.
- **Shared assessment instruments (`anda_brain::assess`, evolution plan M0).** The semantic-assertion judge, recall trace extraction, KIP probe helpers, and JSON-payload parsing moved out of the eval harness into a shared `assess` module behind a minimal `AssessContext` trait (implemented by `Space`, supertrait of `EvalDriver`). The offline harness and the upcoming maintenance self-test (plan M7) now consume identical instruments. Pure refactor: `eval::RecallTrace`/`ToolTrace` re-export from their new home.
- **Per-space `MemoryPolicy` (evolution plan M-P).** A versioned, validated policy object collects the numeric knobs of memory behavior (decay factor, stale-event threshold, backlog targets, plus fields reserved for reinforcement/self-test/recall/shadow phases). Stored in the `memory_policy` space extension, settable via `update_space`, readable via `Space::memory_policy()`. Maintenance cycles without explicit `parameters` now run under the space policy; defaults equal the values documented in BrainMaintenance.md, so an unset policy is not a behavior change.
- **Configurable semantic probe search.** Memory expectations accept `search_threshold` (default 0.35) and `search_limit` (default 8) for assertion probes; search text is now escaped for backslashes as well as quotes, and both fields are validated offline.
- **Eval report provenance.** `EvalReport` now records `profile_id`, the checkpoint `model`, and `started_at`, so reports can be compared across runs without external bookkeeping; checkpoint turns also carry the representative sample's model.
- **Fixture globbing in CI and `make eval-validate`.** Both now validate every `anda_brain/evals/*.json` fixture automatically (`*_profile.json` as profiles, the rest as scenarios), and a unit test (`bundled_eval_fixtures_parse_and_validate`) parses and validates the same set in `cargo test`. The wiki retrieval fixture moved to `anda_brain/evals/wiki/retrieval.json` to keep the top-level directory harness-only.
- **Checkpoint sampling with variance-aware gates.** Eval profiles accept `checkpoint_samples: N` (or `--checkpoint-samples`) to run Recall N times per checkpoint, reporting mean scores plus a propagated `total_stddev`; findings only count with majority support across samples, and `--confidence-z` makes `--min-score` gate on the lower confidence bound instead of a single noisy roll.
- **Shared-formation experiments.** `anda_brain eval --shared-formation` replays formation once per scenario, snapshots the space objects, and forks the snapshot into an isolated in-memory store per profile (`space::copy_space_objects` + `AppState::fork_with_store`), so maintenance policies are compared on identical encoded memory without formation LLM variance — and the most expensive phase runs once instead of once per profile.
- **LLM-as-judge scoring.** Profiles with `"judge": "llm"` score checkpoint answers against the rubric's previously unused `scoring_rubric` and the scenario `hidden_profile`: paraphrases count fully, correct meta-references to superseded facts are no longer penalized as stale, and the judge emits attributed findings plus a per-checkpoint satisfaction signal. Lexical scoring remains the deterministic default.
- **Semantic graph probes.** Memory expectations accept a natural-language `assertion` (with optional `search` text) instead of hand-written KQL; the harness runs a semantic search and lets the judge decide whether the evidence shows the asserted memory state, staying correct across valid graph-encoding variations.
- **Noise pressure and simulated users.** Scenarios accept a deterministic `noise` config (seeded corpus injection between anchors) and `"type": "simulated"` turns whose messages are written by an eval-only user simulator from the hidden profile, transcript, and satisfaction trail; reports carry a `satisfaction_trajectory`.
- **Trajectory metrics and real graph health.** Aggregate scores weight later checkpoints more, `evolution_quality` now measures late-vs-early checkpoint improvement instead of re-averaging other components, and `graph_health` reads real metabolism counters (unsorted backlog, orphans) through read-only KIP.
- **Prompt optimization loop.** `anda_brain eval --optimize formation|recall|maintenance|auto` treats the three agent prompts as an evolvable genome: attributed failures drive an optimizer LLM that proposes surgical find/replace edits, candidates are re-evaluated on fresh spaces, and edits are kept only when they beat the baseline beyond the sampling noise band. Accepted prompts and the decision log are written to `--optimize-out` for human review; agents read prompts through a new `agents::prompts` override layer.
- **Longitudinal memory eval harness.** `anda_brain::eval` can replay user timelines through Formation, optional Maintenance, and Recall checkpoints; score memory utility, forgetting quality, graph health, uncertainty, latency, and token cost; and attribute failures to Formation, Recall, Maintenance, grounding, synthesis, or overconfidence.
- **`anda_brain eval` CLI command.** Local eval runs now support single scenarios, scenario suites, profile comparisons, JSON report output, score/finding gates for CI, and starter scenarios/profiles under `anda_brain/evals/`.
- **Eval gate artifacts.** Gated `anda_brain eval` runs now embed the gate criteria, pass/fail state, and failure messages in the JSON report before returning a non-zero CI exit.
- **Eval validate-only mode.** `anda_brain eval --validate-only` now checks scenario/profile inputs offline, emits an `EvalValidationReport`, and fails before model or storage initialization when inputs are unsafe.
- **Eval fixture CI and summaries.** CI now runs the starter eval fixtures through offline validation, `anda_brain eval --summary-only` prints compact human-readable summaries, and the starter suite includes additional fact-correction, counterparty-boundary, travel-logistics, and expiring-discount scenarios.
- **Hermetic eval runs with automatic cleanup.** Every eval path (including single scenario + single profile) now runs in a freshly created, run-scoped space (`{space_id}_{profile}_{scenario}_{run_id}`), so reruns never score against memory left over from a previous run; run-scoped spaces are deleted from the object store after their report is collected unless `--keep-spaces` is passed (`space::delete_space_objects` + `AppState::evict_space`).
- **Strict eval fixture parsing.** Scenario and profile JSON now rejects unknown fields, turning rubric typos (e.g. `forbidden_terms` for `forbidden_answer_terms`) into load errors instead of silently weakened rubrics that still pass validation.

### Fixed
- **Agent failures are attributed to the stage that ran them.** A Formation agent failure now counts as `formation_miss` and a Maintenance failure as `bad_consolidation` (previously both were recorded as `bad_synthesis`), keeping attribution and the prompt optimizer's target selection honest.
- **Probe transport errors degrade instead of aborting.** A failed read-only KIP request during memory probing becomes a `graph_probe_error` finding and the run continues; previously it aborted the scenario and discarded every completed report in the suite.
- **Errored probes are no longer double-counted as memory failures.** An expectation whose probe errored (transport or KIP `Response::Err`) is scored as unknown — excluded from presence/forgetting weights — instead of also producing a `formation_miss`/`bad_consolidation` finding.
- **Unused expectation `answer_terms` now lower the lexical score.** Lexical utility averages probe-verified presence, required-term coverage, and expectation answer-term coverage, so a memory that exists but never reaches the answer costs points, not just findings.
- **Symmetric judge/harness finding dedup.** A judge finding with an expectation id no longer double-counts a harness finding of the same kind recorded without one.
- **Run-scoped space ids always satisfy AndaDB naming rules.** Composed eval space ids are lowercased to `[a-z0-9_]` and capped at 64 chars with a hash suffix, so uppercase/hyphenated scenario or profile ids and long `--space-id` values no longer fail at space creation.
- **Aborted eval runs no longer leak spaces.** Run-scoped spaces are closed and deleted even when a scenario or phase fails, across the suite, shared-formation, and optimizer paths.
- **Eval logs to stderr.** The eval command now initializes a stderr logger (stdout stays reserved for reports), so judge fallbacks and space setup are no longer silently dropped.
- **Trace grounding attribution matches tool outputs only.** Term evidence is no longer searched in tool names/args, where recall echoes the user's query, misclassifying grounding failures as synthesis failures.
- **Removed the ineffective `--auto-create-space` eval flag.** Run-scoped spaces are always freshly created; the boolean flag could not be disabled from the CLI anyway.
- **Eval turns no longer race in-flight maintenance.** Maintenance turns wait for the processing flag even when a cycle was already running (the agent returns no conversation id in that case), and checkpoints wait for maintenance to go idle before probing, so hook-triggered auto-maintenance can no longer let probes read a graph mid-consolidation.
- **Stuck background stages degrade to findings instead of aborting the suite.** Formation/maintenance wait timeouts are recorded as `formation_miss` / `bad_consolidation` findings and the run continues; failure attribution now also counts findings from non-checkpoint turns.
- **Judge findings no longer double-count harness findings.** Under the LLM judge, a judge finding duplicating an already-recorded probe/term finding of the same kind (and expectation) is dropped, keeping `--max-findings` gates honest.
- **Checkpoint token budgets no longer double-count cached tokens.** `max_checkpoint_total_tokens` now budgets input + output tokens only, since the OpenAI adapter already includes cached tokens in `input_tokens` while the Anthropic adapter reports cache reads separately.
- **`--optimize` now rejects `--min-score`/`--max-findings`** instead of silently ignoring them, and `--confidence-z` feeds the optimizer's accept/reject noise band.

### Changed
- **Shared-formation policy phases run concurrently.** Each profile's fork lives in its own in-memory store, so the policy replays now run in parallel per scenario.
- **JSON fixture load errors include the file path.**

## [0.9.2] — 2026-06-28

### Changed
- **Release version advanced to `0.9.2`.** `anda_brain` now reports `0.9.2`, and the lockfile was refreshed for the release.
- **Recall requests now use medium model effort.** Brain Recall raises its model effort from low to medium to improve answer quality while keeping the bounded runtime guardrails introduced in `0.9.1`.

## [0.9.1] — 2026-06-27

### Changed
- **Release version advanced to `0.9.1`.** `anda_brain` now reports `0.9.1`, and the lockfile was refreshed for the release.
- **Recall requests now use a leaner bounded runtime context.** Brain Recall limits carried history to the latest completed conversation, caches primer context briefly, loads counterparty/profile data concurrently, and no longer exposes the notes tool as a model dependency while still injecting notes into the prompt.

### Fixed
- **Recall execution now has explicit time and turn guardrails.** Recall runs enforce a total timeout and model turn limit, persist failed conversations consistently, and return the last available output with failure details when guardrails trip.

## [0.9.0] — 2026-06-27

### Added
- **Built-in MCP server support.** `anda_brain` now exposes memory operations through MCP over both stdio (`anda_brain mcp --space-id <spaceId> ...`) and Streamable HTTP (`/mcp/<spaceId>`), enabling MCP-capable agents to connect directly to Brain spaces.
- **MCP memory tools.** The MCP server provides tools for remembering conversations, recalling memory, running maintenance, and executing readonly KIP queries.
- **Remote MCP configuration controls.** HTTP service mode now includes MCP path prefix, host/origin allowlists, optional space auto-creation, stateful sessions, and keep-alive configuration.

### Changed
- **Release version advanced to `0.9.0`.** `anda_brain` and `anda-cli` now report `0.9.0`, and the lockfile was refreshed for the release.
- **Documentation and agent skill guidance now cover MCP integration.** English and Chinese READMEs, API docs, and SKILL files describe stdio and Streamable HTTP MCP setup plus the exposed memory tools.
- **Dependencies updated for MCP support.** The workspace now depends on `rmcp` and `schemars`, and `object_store` was refreshed to the `0.14` line.

### Fixed
- **Memory agents compact long-running contexts before continuing.** Formation, Maintenance, and Recall now perform runner handoffs when context windows fill, preventing large review prompts or extended tool loops from overrunning model limits while preserving conversation progress.

## [0.8.1] — 2026-06-20

### Changed
- **Release version advanced to `0.8.1`.** `anda_brain` now reports `0.8.1`, and the lockfile was refreshed for the release.
- **COSE/CWT test fixtures now use `cose2`.** The remaining direct `coset` dev dependency has been replaced with `cose2`, keeping token and COSE key fixtures aligned with the current COSE stack.
- **`make fix` now formats before applying clippy fixes.** The fix target runs `cargo fmt --all` before `cargo clippy --fix --workspace --tests`.
- **Anda ecosystem dependencies were refreshed to current patch releases.** The lockfile now resolves the latest compatible 0.13/0.8 runtime, database, KIP, and object-store crates.

## [0.8.0] — 2026-06-13

### Changed
- **Release version advanced to `0.8.0`.** `anda_brain` and `anda-cli` now report `0.8.0`, and the lockfile was refreshed for the release.
- **Anda ecosystem dependencies moved to the 0.13/0.8 line.** Brain now depends on `anda_core`, `anda_engine`, `anda_engine_server`, and `anda_web3_client` 0.13, plus the latest `anda_object_store` and `anda_db_tfs` 0.8 releases.
- **CBOR handling now uses `cbor2`.** Request parsing, CBOR responses, and payload tests now use canonical CBOR encoding/decoding through `cbor2` instead of `ciborium`.
- **Space token generation no longer depends on `ic_cose`.** Token entropy now comes from `rand`, allowing the direct `ic_cose` dependency to be removed while retaining 20-byte random token material.

## [0.7.2] — 2026-06-12

### Added
- **Maintenance status now exposes the latest task start time.** `MaintenanceAt` includes `start_at`, persisted when a maintenance cycle begins and surfaced through the Rust API, generated TypeScript API docs, Chinese API docs, and Go CLI client types.

### Changed
- **Release version advanced to `0.7.2`.** `anda_brain` and `anda-cli` now report `0.7.2`, and the lockfile was refreshed for the release.

## [0.7.1] — 2026-06-12

### Changed
- **Release version advanced to `0.7.1`.** `anda_brain` now reports `0.7.1`, and the lockfile was refreshed for the release.
- **Dependencies updated for the latest engine runtime fixes.** `anda_engine` 0.12.36 → 0.12.37, with transitive patch updates for `block-buffer`, `memchr`, and `smallvec`.
- **KIP prompt syntax summaries now match the expanded 0.8 grammar.** Brain Formation, Maintenance, Recall, and shared KIP syntax guidance show comma-separated multi-key `ORDER BY` and proposition-level `EXPECT VERSION` guards in their compact syntax blocks.

## [0.7.0] — 2026-06-11

### Added
- **KIP prompt assets now document the 0.8 protocol surface.** Brain Formation, Maintenance, Recall, and shared KIP syntax guidance cover reserved `_` metadata, `EXPECT VERSION` optimistic concurrency, predicate variables, multi-key `ORDER BY`, semantic/hybrid `SEARCH`, `UPDATE`, `MERGE`, and `EXPORT` patterns.
- **Regression coverage was added for new runtime guardrails.** Formation high-water processed markers, Maintenance history ordering and input validation, Recall empty-output preservation, token revoke safety, and conversation list limit clamping now have focused tests.

### Changed
- **Release version advanced to `0.7.0`.** `anda_brain` and `anda-cli` now report `0.7.0`, and the lockfile was refreshed for the release.
- **Anda ecosystem dependencies moved to the 0.8 line.** `anda_db`, `anda_cognitive_nexus`, and `anda_kip` now use the `0.8` series.
- **Brain service lifecycle handling is more robust.** Shutdown closes loaded spaces concurrently, idle eviction closes databases before removing entries, and scheduled maintenance trigger construction is simplified while preserving the existing cadence.
- **Shared request handling has been consolidated.** API handlers use a common sharding validator, and the Chinese website response is pre-rendered with the correct `zh-CN` document language.
- **`anda-cli` now targets the local Brain service by default and exposes deployment controls.** The CLI default base URL is `http://127.0.0.1:8042`, and new `--shard`/`ANDA_SHARD` plus `--timeout`/`ANDA_TIMEOUT` options send `Shard-Id` headers and tune HTTP request timeouts.
- **`anda-cli` documentation now reflects the current command surface.** The README documents `status` for service metadata, `info` for space details, `formation-status`, `get-or-init-user`, BYOK retrieval, `daydream` maintenance scope, batch formation exclusions, and single-command KIP readonly requests.
- **Documentation now reflects self-hosted deployment.** READMEs, website copy, and skill files remove discontinued hosted-service guidance, point users to self-hosted setup, link Anda Bot as a ready-to-run agent, and fix current CLI/API examples and model defaults.
- **Brain agent instructions are stricter and more operational.** Prompt assets emphasize empty-write discipline, bounded extraction, KIP error recovery, read-modify-write version guards, bulk update patterns, memory portability, and the absence of read-access statistics.

### Fixed
- **Formation resumes correctly from an empty processed marker.** Spaces restart formation from the beginning when no processed marker exists, so conversations queued before the first successful formation pass are not left stuck.
- **Formation processed markers are monotonic high-water marks.** Reprocessing an older conversation can no longer rewind `brain_processed`.
- **Agent records preserve original input on anomalous empty rounds.** Formation, Maintenance, and Recall no longer clear a conversation's original messages when a cancelled or empty round returns no chat history.
- **Agent context history now keeps completed conversations in runtime queue order.** Maintenance and Recall initialization filter out in-progress conversations and restore completed history oldest-to-newest, avoiding transient conversations leaking into later context while preserving newest entries correctly.
- **Maintenance startup and input handling are safer.** Malformed maintenance input is rejected before claiming the processing slot or creating a conversation.
- **Idle probes for unknown spaces no longer grow the space map indefinitely.** Uninitialized placeholder entries are evicted once idle, while initialized spaces are still protected against concurrent users and processing work.
- **Space token lookup and revocation are restricted to token-prefixed credentials.** Token verification and revocation now reject non-`ST` keys, keeping platform extensions such as tier and BYOK out of the token path.
- **Conversation listing clamps zero limits to safe bounds.** `limit=0` no longer allows an empty-page panic or unbounded scan.
- **`anda-cli formation` now rejects malformed JSON message payloads instead of storing them as plain text.** Valid JSON arrays and objects must decode to messages with role and content, while non-JSON log-like text still submits as a user message.
- **`anda-cli formation` batch mode avoids submitting bookkeeping and hidden files.** Recursive batch scans skip dot-prefixed entries, the checklist file, and temporary report files while preserving user/agent/topic context and only filling per-file `source` when it was not already provided.
- **`anda-cli execute-kip-readonly` accepts single-command requests cleanly.** Requests may now use either `command` or `commands`, object command `parameters` are optional and omitted when empty, and the two forms are validated as mutually exclusive.
- **`anda-cli conversations` supports 64-bit conversation IDs.** Conversation detail and delta commands parse IDs as unsigned 64-bit values to match server-side identifiers.

## [0.6.11] — 2026-06-10

### Changed
- **Dependencies updated for the latest engine runtime fixes.** `anda_engine` 0.12.32 → 0.12.35, bringing follow-up delivery, structured subagent arguments, and HTTP response decoding fixes into Brain.

### Fixed
- **Brain agents now read notes through the current engine note extension shape.** Formation, Maintenance, and Recall use `items` from the current notes payload while falling back to legacy notes storage when needed, preserving existing note context during the engine upgrade.

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
