# AGENTS.md

Guidance for coding agents working in this repository.

## Agent Workflow

- Work independently as the current agent. Do not spawn or delegate work to
  subagents.
- Before editing, run `git status --short`, confirm the current branch, and
  inspect existing diffs in the files you intend to change. Preserve the user's
  existing work; do not overwrite or revert unrelated files or changes.
- Use `rg` for search and focused reads before editing. Do not assume module
  boundaries from filenames alone.
- Before committing, review the final diff and stage only the files or hunks
  belonging to the requested task.
- At completion, briefly summarize the changes, the checks actually run and
  their results, and any checks not run or blocked. Never report an unrun check
  as passing. When committing, include the branch and commit ID in the summary.

## Project Overview

Anda Brain is a Rust service that provides long-term memory for LLM agents. The
main crate is `anda_brain`, which exposes:

- Formation: encode conversations into structured memory.
- Recall: answer natural-language queries from memory.
- Maintenance: consolidate, prune, and optimize memory.

The service stores memory in an AndaDB-backed Cognitive Nexus and uses KIP 2.0
(Knowledge Interaction Protocol) internally. Business agents should not need to
write KIP directly.

The service tracks KIP `11a82ec` and `kip://profiles/cognitive-memory@2.0.0`
(content digest `sha256:734aa0fd…`; the draft rewrote 2.0.0 in place, so only the
digest names a revision). It depends on the 0.14 DB/KIP stack and `anda_engine` 0.16
from crates.io; `Cargo.lock` is the record of resolved versions. Brain needs at
least `anda_cognitive_nexus` 0.14.2 (0.14.1 cannot open a 0.14.0 store) and
`anda_core`/`anda_engine` 0.16.2 (the bounded note index API); the reference
supplement is generated from the locked `anda_kip` (`scripts/sync-kip-reference.mjs`
checks the source against `Cargo.lock`). `tiktoken-rs` is held to 0.12.x: the Recall
tokenizer identity `o200k_base@tiktoken-rs-0.12` names that line, and `…-0.12.0`
stays accepted. The Worker uses `@ldclabs/kip-do` 0.14 from npm. The commented
`[patch.crates-io]` block in `Cargo.toml` is for developing against sibling
`anda-db` / `anda` checkouts: enable the shared DB/KIP stack together and verify one
type identity with Cargo metadata.
KIP v2 has not been deployed. Use fresh v2 Spaces for current acceptance;
do not add pre-release old-data migration work (including 2.1.0-draft Spaces)
unless explicitly requested. The one requested exception is the standalone
`tools/migrate-draft-space` (2.1.0-draft Space → 2.0.0 Nexus); keep it out of the
library. Keep normal restart, eviction and unresolved-write recovery fully tested.

## Repository Layout

- `anda_brain/`: Rust library and binary for the service.
- `anda_brain/src/agents/`: Formation, Recall, and Maintenance agents.
- `anda_brain/src/space.rs`: space lifecycle, AndaDB setup, auth checks, and
  background flushing/eviction.
- `anda_brain/src/handler.rs`: HTTP route handlers and API entry points.
- `anda_brain/src/payload.rs`: JSON/CBOR/Markdown payload negotiation.
- `anda_brain/src/types.rs`: API input/output and persisted config types.
- `anda_brain/assets/`: agent prompts. The KIP syntax card
  and the Cognitive Memory Profile are **not** copied here — `anda_kip` ships
  them with the protocol, and `agents::prompts::system_prompt()` includes the full
  syntax, role cards and Profile in every model call, including budgeted Recall.
- `anda_brain/src/kip_reference.rs` and `assets/kip-reference/`: bounded reference
  discovery and the generated specification/schema supplement. Its manifest pins
  the published KIP source; do not hand-edit generated reference material.
- `anda_brain/src/kip.rs`: the KIP 2.0 envelope seam (request builders, the
  read-only gate, two-level response reading, and the KIP string/timestamp
  literal helpers).
- `anda_brain/src/memory_interface/` and `handler/memory_interface.rs`: the KIP
  Memory Interface at `memory_basic` (staged sources, intake ledger and receipts,
  recall briefings, recording repair, forget/ErasurePlan). Receipt progress is
  read from the Formation conversation and its persisted trace; keep it monotone
  and keep the advertised levels truthful.
- `anda_brain/src/assess/` and `assess.rs`: online diagnostic model routing,
  typed read observations, Recall trace/citations and metadata. Preserve these
  and the usage/correction ledgers; they are online diagnostics, not an offline
  evaluator.
- Offline product regressions live in sibling MIB. Do not add a comprehensive
  evaluator, an `eval` CLI or process-global prompt/policy overrides to Brain. The independent
  wiki corpus under `anda_brain/evals/wiki/` remains in use.
- `anda_brain/src/settlement/`: bounded decay, correction discovery and Watch
  scheduling. `watch.rs` reads ids, overall versions and WatchState generations;
  Nexus owns matching and authorized coverage. No local family-rate Skill verdict
  runs without an independent observer/trial/evaluation pipeline.
- `anda_brain/src/cognitive.rs`: model-facing host mechanics: full syntax,
  canonical content digests, protected Watch arming and bounded task leases.
- `anda_brain/src/attention/` and `space/attention.rs`: persistent Space discovery,
  bounded Watch/wake scheduling and optional semantic evaluation. Register direct
  native work before creation; preserve current native coverage and generation checks.
- `anda_brain/src/action/`: four-way decisions, clarification and fenced dispatch.
  Callbacks require explicit host bindings; unknown delivery requires reconciliation.
- `anda_brain/src/runtime_api/` and `handler/runtime.rs`: startup configuration,
  authenticated recipient-filtered inboxes, responses and runtime status.
- `anda_brain/src/consequence/` and `recall_receipt.rs`: independent outcomes,
  verifiable memory delivery, calibrated utility and scoped trust proposals.
  Governance application requires current native authority, never a model claim.
- `anda_brain/src/learning/`: optional trusted paired-trial contracts, Nexus
  evaluator and persistent host runtime (`learning` feature). Native dispatch
  gates require real leases, executable authority and dependency validation.
  Fixed-cutoff native verdicts, persistent reviews, safety revocation and a
  read-only Recall applicability gate require explicit host use. No production
  bindings or automatic adoption are enabled by compilation.
- `anda_brain/src/recall_budget/` and `agents/recall/budgeted.rs`: explicit
  Recall packet and cumulative planning-input budgets with a pinned tokenizer.
  Model selection names existing items only. Preserve required constraints,
  native uncertainty and current procedure checks; never claim semantic
  completeness or execution permission from a bounded packet.
- `anda_brain/src/vocabulary.rs` and `space/vocabulary.rs`: the Space's draft
  vocabulary (`kip://local/draft@0.0.0`, KIP §20.16), the Formation `DEFINE`
  gate helpers, `review_schema` queueing, the deprecated `declare_memory_symbols`
  shortcut, and the owner's promotion API. The legacy `kip://anda-brain/memory`
  host package stays read-only for Spaces that have it.
- `anda_brain/API*.md`, `anda_brain/README.md`, `anda_brain/SKILL.md`: public
  API and integration documentation.
- `anda_brain/RUNTIME.md`: host setup, scheduling, action contracts and recovery;
  the learning, utility, semantic Watch and trust runtime guides hold their specific
  configuration contracts. Each has a separate `_cn.md` edition. Public docs use
  capability names, not internal phase IDs, and never depend on private plans.
- `VALIDATION_PLAN.md` / `VALIDATION_PLAN_cn.md`: proposed observability and MIB
  validation work. These plans are not evidence that new metrics, real-model gains
  or deployment calibration have already been implemented or accepted.
- `skills/anda-brain/`: packaged integration skill for external agents. Keep its
  `SKILL.md` identical to `anda_brain/SKILL.md`, served by `GET /SKILL.md`.
- `anda-brain-worker/`: a compact Cloudflare Worker port on `@ldclabs/kip-do`,
  a second and independent KIP 2.0 engine. It shares the invariants below but
  not the code; its capabilities differ (no atomic batch across operations, no
  semantic search, and no retention-expiry sweep — `SET RETENTION` itself the
  engine has) and `anda-brain-worker/README.md` is the authority on which. Its
  prompts are vendored under `anda-brain-worker/assets/` and inlined by
  `pnpm run codegen:prompts`.
- `anda_brain/conformance/`: the KIP harness adapter (`adapter.mjs`) and the
  script that runs the scenarios it exercises without a model provider.
- `tools/migrate-draft-space/`: a standalone crate (outside the workspace) that
  links the 0.13 and 0.14 engines to move a 2.1.0-draft Space onto a fresh
  2.0.0 Nexus; see its README.
- `deploy/`, `anda-brain-demo/`, `anda-cli/`: deployment
  and integration material. Do not change these unless the task is explicitly
  about them.

## Development Commands

Run these from the repository root:

```bash
cargo fmt --check
cargo clippy -p anda_brain --all-targets --all-features -- -D warnings
RUST_MIN_STACK=16777216 cargo test -p anda_brain --all-features
```

Unoptimized async frames on the Space-loading and KIP paths can exceed Rust's 2 MiB
test-thread stack in debug builds, so keep `RUST_MIN_STACK=16777216` on Rust test
commands. Release code must fit the 2 MiB default worker stack: keep the
`crate::boxed` inlining barriers where a subsystem is entered (Memory Interface
dispatch, the recall pass and its channels, read-only KIP execution). Without
them, the optimizer merges async poll frames into one frame that keeps a slot
for every await.
`cargo test -p anda_brain --all-features` includes a bin test that binds an
ephemeral localhost port. In restricted sandboxes it may fail with
`PermissionDenied`; rerun it with the required permission rather than treating
that as a code failure.

The library is feature-gated (see "Cargo Features" below), so also check that
the lean build still compiles when you touch `space.rs`, `handler.rs`,
`authz.rs`, or `types.rs`:

```bash
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib --features wiki
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib --features mcp
```

For local manual testing:

```bash
cargo run -p anda_brain --features mcp,wiki
cargo run -p anda_brain --features mcp,wiki -- local --db ./db
```

Authentication is disabled when `ED25519_PUBKEYS` is empty. Do not assume this
is safe for production.

The Cloudflare Worker is checked separately, and its own checks must pass when
you touch `anda-brain-worker/`:

```bash
CI=true pnpm --filter @ldclabs/anda-brain-worker check
```

Install with the repository's pnpm lockfile: `CI=true pnpm install --frozen-lockfile`.
The check includes generated-asset verification, TypeScript, tests and a deployment
dry run; it does not deploy the Worker.

## Cargo Features

The `anda_brain` library defaults to memory only — formation, recall,
maintenance, and their HTTP routes. Optional features add:

- `wiki`: the structured wiki (documents, versions, ACL-scoped reads, OKF
  import/export), its agent tools, the WikiDigest graph extraction, the
  `/v1/{space_id}/wiki/*` routes, and the `wiki_*` fields of `SpaceInfo` and
  `UpdateSpaceInput`.
- `mcp`: the MCP channel (stdio and Streamable HTTP). With `wiki` also on, the
  wiki tools join the MCP tool router.
- `experiments`: trusted isolated runs, snapshots, business time, cost receipts,
  forced Recall-budget creation and bounded evaluator-only procedure audits.
  The audit is not an execution permit; unsupported MIB learning conditions
  must remain explicitly refused until their actual host bindings exist.
  Enables the sibling Nexus `simulation` feature only for host lifecycle tests.
  No serialized clock override or automatic learning is exposed.
- `learning`: trusted contracts/evaluator, native record adapters and persistent
  host trial runtime. Explicit registration and executor/observer bindings are
  required; do not widen model mutation permissions or auto-adopt candidates.

The `anda_brain` binary declares `required-features = ["mcp", "wiki"]`: it is
the full product, so every build of it must pass `--features mcp,wiki`. Cargo
silently skips the binary when they are absent.

When adding code that touches the wiki or MCP, gate it with
`#[cfg(feature = "…")]` rather than widening the default surface, and keep the
lean build compiling.

## Coding Conventions

- Follow the existing Rust 2024 style and keep `cargo fmt` clean.
- Prefer existing crate patterns over new abstractions.
- Keep behavior changes scoped to `anda_brain` unless the task explicitly
  targets demos, deploy files, or packaged skills.
- Avoid external network calls in tests. Unit tests should use in-memory or local
  storage and must not require a live model provider.
- Add tests close to the module being changed when behavior changes.
- Preserve JSON, CBOR, and Markdown payload compatibility in `payload.rs` and
  route handlers.
- Preserve compact persisted field aliases in `types.rs`; they are storage/API
  compatibility details.
- Never use `oneOf`, `anyOf` or `allOf` in a model-facing schema: agent tool
  definitions, MCP tool schemas, and structured-output schemas. Anthropic
  rejects them at the top level of a tool's `input_schema` and fails the whole
  request with a 400 (KIP's top-level `oneOf` once broke every Formation,
  Recall and Maintenance pass on Claude); strict modes reject them anywhere.
  Use a type list for nullable fields and for alternatives of distinct types,
  one object with an `enum` discriminator for tagged variants, and enforce
  cross-field rules when parsing. schemars derives these keywords from
  `Option<Struct>`, untagged and tagged enums: `AndaBrainMcpServer::new`
  flattens them and `tool_schemas_use_no_schema_combinators` checks every MCP
  tool, so run it after adding one. The KIP reference documents under
  `assets/kip-reference/schemas/` are text the model reads, not tool schemas.
- Be careful with dirty worktrees. Do not revert or overwrite unrelated user
  changes.

## KIP 2.0 Invariants

These are protocol invariants, not preferences. Breaking one makes the brain
confidently repeat things nobody claimed:

- A Proposition existing is not the Proposition being true. Belief questions are
  answered by `BELIEF` projection; raw `FIND` is for audit. `insufficient` is
  never reported as "no".
- Never decay Assertion confidence over time. Disuse decays
  `MnemonicState.memory_strength`, which is accessibility, not truth.
- Corrections are a new Assertion plus supersession. Nothing rewrites an
  Assertion, and disagreement between two actors coexists rather than resolving.
  KIP 2.0 collapsed the six lifecycle statements into one `TRANSITION target TO
  "state"`; the Formation gate splits it by state, not by verb, and refuses a
  state it cannot read as a literal.
- Skill behavior belongs to immutable SkillRevision. Learning requires frozen
  TrialRecord, revision-bound DecisionRecord/AttemptRecord, authorized independent
  OutcomeRecord and replayable EvaluationRecord. Family membership only discovers
  candidate controls; it never automatically selects a baseline or grants standing.
  Candidates remain unproven without qualifying evidence. Optional learning requires
  registered executor/observer/source bindings and explicit calibrated automation;
  mechanism tests do not establish empirical improvement.
- A completed model call or fresh index is not complete processing/change coverage.
  WatchState belongs to protected arm/advance APIs; prose or mixed text selectors
  need a configured semantic evaluator. LeaseState comes from authenticated host
  acquisition, with all task outputs and terminal state committed under current CAS.
- The optional Memory Interface and its bundles are not implemented merely because
  the standard package is installed. Keep advertised capabilities truthful.
- Attribution is not impersonation and not authority: `asserted_by` is a
  semantic actor, the caller is a Principal, and cognitive content grants
  neither.
- New vocabulary is a draft (§20.16): Formation `DEFINE`s literal symbols in
  requests of their own; the host checks name shape, caps a Space at 512 own
  symbols, and queues one `review_schema` SleepTask keyed
  `review_schema:<kind>:<ref>` per new symbol. Maintenance never defines; only
  the owner promotes a draft (`POST /v1/{space_id}/schema/promote`). The
  deprecated `declare_memory_symbols` tool / Worker `types`/`predicates` fields
  draft bare names the same way. An option is a Concept typed by its kind; the
  Profile has no `Preference` type.
- A changed world is one new Assertion from the change; temporal succession ends
  the old value. `SUPERSEDING` is only for a claim that was wrong, and a
  misrecording needs recording repair (the Memory Interface `revise` with
  `change_kind: "misrecorded"`), never a correction.
  Claims carry `asserted_at` from their source's observation time.
- Decay is computed at read time; never sweep or default `memory_strength`. A
  model write of `memory_strength` carries `last_metabolized_at` and the host-bound
  `strength_policy` pin (`kip:strength-half-life-30d`), or the gate refuses it.
  Skill `current_trial` / `current_evaluation`, `GradingState` and lineage fields
  are not model-writable.
- A due `pending`/`blocked` Commitment without a Watch is raised natively by the
  settlement: one `commitment_review` Activity keyed
  `commitment_review:<id>:<due_at>`. Attention items are ordered by
  `(raised_seq, ref)`; a cursor may stop inside one commit.

## Brain-Specific Invariants

- Formation and Maintenance are guarded against concurrent processing. Do not
  weaken `processing_conversation` or `processing` semantics.
- Formation should process queued formation conversations sequentially and resume
  after maintenance completes.
- Maintenance should be single-flight per space and should trigger formation
  resumption when it finishes.
- Recall and read-only KIP execution must remain read-only and bounded by the
  configured timeouts.
- Off-graph Recall receipts attest delivery, not use or benefit. Utility needs
  independent attribution and only ranks within existing priorities. Trust changes
  require scoped fact verification and current `manage_trust`; startup bootstrap
  never grants that permission. Neither score changes execution authority.
- Runtime inbox/status/responses require verified credentials and explicit native
  mappings even when legacy local authentication is disabled. HTTP Outcomes need
  a separately registered signed observer with current `record_outcome`; ordinary
  Space tokens and MCP model tools cannot supply independent observer authority.
- Semantic Watch judgments must cover every authorized candidate under the pinned
  evaluator. Unknown, missing or truncated output cannot advance native coverage;
  configuration changes require explicit review and never auto-rearm old work.
- Owned native writes survive cancelled waiters and drain before database close.
  Keep one live owner per storage shard; CAS does not establish multi-host ownership.
- Runtime status and mechanism fixtures are not empirical learning/calibration
  evidence. Missing measurements and provider costs stay unknown, never zero.
- Space-level token scopes are `read`, `write`, and `*`; keep auth changes
  explicit and test them.
- Space metadata and database extension updates must be persisted with
  `save_extension*`, `flush_metadata`, `flush`, or `close` as appropriate.
- On shutdown or eviction, close databases when possible so AndaDB flushes
  collections and metadata.

## API and Docs

When changing public request/response shapes, auth behavior, content negotiation,
or endpoints:

- Update `anda_brain/API.md` and `anda_brain/API_cn.md`.
- Update `anda_brain/README.md` and `README*.md` when user-facing behavior
  changes.
- Update `anda_brain/SKILL.md` and `skills/anda-brain/SKILL.md` when integration
  instructions or endpoint usage changes.
- Keep English and Chinese docs in sync for user-facing API changes.
- Keep English and Chinese runtime guides in separate files, with language links
  and matching formulas, limits, API names and examples. Chinese entry points link
  to `_cn.md`; never remove necessary detail while separating translations.
- Change the release version only when explicitly asked. Record completed
  behavior and known limits in `CHANGELOG.md`; do not list planned work as shipped.

## Prompt and Asset Changes

Trusted Rust hosts may use immutable `AgentPrompts` via
`AppState::with_agent_prompts` before sharing a host or loading a Space. Only
section A can be replaced; the compiled KIP reference prefix stays intact.
Runtime policies are per-Space `MemoryPolicy` values. Neither configuration
uses a process-global mutable override, and experiments pin actual instance
prompt content in their manifests.

Agent prompts in `anda_brain/assets/` are part of runtime behavior. Edit them
only when the task calls for prompt behavior changes, and describe the intended
agent behavior clearly in the diff. Avoid prompt edits as a workaround for a
code bug.

Each `Brain{Formation,Recall,Maintenance}.md` — in `anda_brain/assets/` and in
`anda-brain-worker/assets/` — is two halves. Everything above `# A.` is the KIP
2.0 reference policy vendored from `anda-db/rs/anda_kip/brain/`; everything from
`# A.` down is that deployment's own contract. **Do not hand-edit the reference
half**: run

```bash
node scripts/sync-kip-assets.mjs
pnpm --filter @ldclabs/anda-brain-worker run codegen:prompts
```

which re-copies the reference half from `anda_kip` in all six files, copies the
Worker's verbatim assets (syntax, Profile and the four role/Memory Interface cards),
and leaves every `# A.` section untouched. The script uses a sibling `anda-db`
checkout by default; set `ANDA_KIP_SOURCE` to a downloaded `anda_kip` crate
directory when syncing to a published version. Edit section A by hand; that is
the half that is ours.
