# Anda Brain API Reference (with TypeScript Types)

**[English](API.md) | [中文](API_cn.md)**

This document specifies the HTTP and MCP interfaces of the Rust `anda_brain` service
(0.13.4, KIP `11a82ec` / `kip://profiles/cognitive-memory@2.0.0`). For an overview,
configuration and deployment, see the [technical reference](README.md); for
agent-facing usage, [SKILL.md](SKILL.md); for runtime setup and recovery,
[RUNTIME.md](RUNTIME.md). The Cloudflare Worker is a separate implementation with a
smaller surface, described in [its README](../anda-brain-worker/README.md) and
[host contracts](../anda-brain-worker/PRODUCT.md).

## Contents

1. [Common Conventions](#1-common-conventions)
2. [Authentication](#2-authentication)
3. [Memory Interface](#memory-interface)
4. [Endpoint List](#4-endpoint-list) — [public](#41-public-endpoints),
   [Space business](#42-space-business-endpoints-v1space_id),
   [wiki](#43-wiki-endpoints-v1space_idwiki),
   [management](#44-space-management-endpoints-v1space_idmanagement),
   [admin](#45-admin-endpoints-admin),
   [runtime](#authenticated-runtime-inbox-and-observations)
5. [MCP Server](#5-mcp-server)
6. [Error Semantics](#6-error-semantics)
7. [TypeScript Type Definitions](#7-typescript-type-definitions)
8. [Frontend Call Example](#8-frontend-call-example-ts)
9. [Rust Host APIs](#9-rust-host-apis)
10. [Execution and Resource Limits](#10-execution-and-resource-limits)

---

## 1) Common Conventions

- Base URL: `http://{host}:{port}` (the service listens on `127.0.0.1:8042` by default).
- Auth header: `Authorization: Bearer <token>`; see [Authentication](#2-authentication).
- Sharded deployments: send `Shard-Id: <index>` (or `X-Shard`) matching the server's `SHARDING_IDX`; the default is `0`.
- Supported serialization formats:
  - Request: `Content-Type: application/json | application/cbor | text/markdown`
  - Response: `Accept: application/json | application/cbor | text/markdown`
  - Content negotiation applies to success bodies. Handler errors use JSON regardless of `Accept`; middleware load shedding (`429`/`503`) and unmatched routes can return plain text or an empty body.
- Most endpoints return an RPC envelope, `RpcResponse<T>`. `POST /v1/{space_id}/memory` returns a Memory Interface `Response` and `POST /v1/{space_id}/execute_kip_readonly` a `KipResponse` instead.
- String timestamps are RFC 3339 and are canonicalized to millisecond UTC (`YYYY-MM-DDTHH:mm:ss.SSSZ`); numeric timestamps are Unix milliseconds.
- MCP clients can use the built-in Streamable HTTP endpoint `/mcp/<space_id>` or the local stdio server `anda_brain mcp --space-id <space_id> [local|aws]`; see [MCP Server](#5-mcp-server).

### Memory semantics the API keeps

- A conversation id returned by Formation or Maintenance tracks a run; it is not a processing receipt. Use Memory Interface receipts (or, in Rust, `Space::processing_report` / `wait_for_processing`) to learn when memory is available.
- Mnemonic decay is computed at read time from a pinned strength policy (KIP Spec §59.1). Nothing sweeps `memory_strength`, settlement reports carry no decay fields, and `memory_strength_decay_factor` is accepted but ignored. Claim expiry (`valid_time.until`) is also evaluated at read time.
- The host gates model writes: an Assertion citing a captured message needs `at` (or `valid.from`), and a `MnemonicState.memory_strength` needs `last_metabolized_at` and the host-bound `strength_policy` (both `ConstraintViolation`). Model-generated Formation requests cannot replace the captured ingest/`:msgN` bindings, and learning/runtime Facet writes fail with `UnsupportedCapability`. Authorized raw administrative KIP remains subject to the engine's full contracts.
- Attention is ordered by `(raised_seq, ref)`, and settlement raises due Commitments natively (`commitments` in its report). The settlement report's Watch `disarmed` count also includes Nexus expiry, and text Watch conditions stay deferred without a configured semantic evaluator. Host pass errors reach the Maintenance model as `assessment.settlement_errors`.
- Settlement `skills.unsupported_reason` says why no Skill verdict ran when no trusted learning pipeline is configured; the legacy counters stay zero.

---

## 2) Authentication

Requests carry `Authorization: Bearer <token>` with one of two credentials:

| Credential | What it is | Where it comes from |
| --- | --- | --- |
| CWT | A Base64 COSE Sign1 token signed by an Ed25519 key listed in `ED25519_PUBKEYS`, with claims `sub` (principal), `aud` (the Space id, or `*`) and `scope` | Whoever holds a trusted signing key, e.g. `anda-cli cwt` |
| Space token | An opaque `ST…` value with a `scope`, an optional `expires_at` and optional wiki ACL `labels` | `POST /v1/{space_id}/management/add_space_token`; the full value is returned only once |

**Scopes** are `read`, `write` and `*` (`TokenScope`). `*` satisfies every
requirement; `read` and `write` satisfy only endpoints that require exactly that
scope, so a `write` credential cannot call a `read` endpoint of a private Space. An
agent that both writes and recalls needs a `*` credential, or one credential of each
scope. Labeled Space tokens must have `read` scope.

Endpoints admit callers in one of these ways:

| Class | Admits | For example |
| --- | --- | --- |
| Public read | Anyone on a public Space. A supplied Space token is still verified there so its labels keep applying; label-restricted tokens are refused where a result spans all labels. Private Spaces need a `read` credential. | `/recall`, `/recall_structured`, conversations, wiki reads |
| Lenient public read | The same, except that a token is not verified on a public Space | `/info`, `/status`, `/formation_status`, `/memory_status`, `/memory/attention`, `/schema/drafts`, `/probe`, `/execute_kip_readonly` |
| Credentialed | A CWT or Space token with the required scope | `/formation`, `/maintenance`, `/memory/pin`, `/memory/forget`, Memory Interface mutations |
| Management | A CWT for this Space; Space tokens are rejected | `/management/*`, `/schema/promote`, a `semantic` forget |
| Admin | A CWT whose `sub` is listed in `MANAGERS` | `/admin/*` |
| Runtime | Verified credentials plus explicit `BRAIN_RUNTIME_CONFIG` mappings | `/attention`, `/attention/{id}/responses`, `/outcomes`, `/runtime/status` |

If `ED25519_PUBKEYS` is empty, authentication is disabled for every class except
Runtime: requests are accepted without signature verification. Never deploy that way
outside local development. The runtime routes always require verified credentials and
mappings, and independent HTTP outcomes stay disabled without a signed CWT verifier.

---

<a id="memory-interface"></a>

## 3) Memory Interface (KIP 2.0, `memory_basic`)

Every Space serves the optional KIP 2.0 Memory Interface
(`KIP-2.0-Memory-Interface.md`, wire shapes in `kip-memory.schema.json`) at the
`memory_basic` level: five intents — `observe`, `recall`, `revise`, `feedback`,
`forget` — over one request shape, one intent per request. `memory_experience`,
`memory_learning`, `durable_brain_runtime`, `receiver_fencing` and Capsule exchange
are not advertised, and a request that `requires` one fails `UnsupportedCapability`
before anything runs. The existing endpoints are unchanged; this is the path a
business Agent should use.

The descriptor is on `GET /info` (without a default Space) and on
`GET /v1/{space_id}/info` as `memory_interface`; raw KIP clients see the same
binding in `DESCRIBE CAPABILITIES` and `DESCRIBE PRIMER`:

```json
{
  "kip_memory": "2.0",
  "bundles": ["memory_basic"],
  "default_budget": {"max_output_tokens": 4096, "deadline_ms": 30000},
  "tokenizer": "o200k_base@tiktoken-rs-0.12",
  "minimum_response_tokens": 256,
  "default_space": {"id": "my_space"}
}
```

| Endpoint | Auth | Body / result |
| --- | --- | --- |
| `POST /v1/{space_id}/memory` | recall: `read` (public spaces allow anonymous); mutations: `write`; a `semantic` forget needs a CWT (owner) | Memory Interface `Request` → `Response` (not an `RpcResponse`) |
| `POST /v1/{space_id}/memory/sources` | `write` | `StageSourceInput` → `RpcResponse<StagedSourceRef>` |
| `GET /v1/{space_id}/memory/sources/{source_ref}` | `read` credential | `RpcResponse<StagedSource>` for the caller that staged it |
| `GET /v1/{space_id}/memory/receipts/{receipt_ref}` | `read` credential | `RpcResponse<{receipt, progress, result, warnings}>` |
| `GET /v1/{space_id}/memory/plans/{plan_ref}` | `read` credential | `RpcResponse<{plan_ref, receipt_ref, plan, host_surfaces}>` |

Once a caller is admitted, `/memory` answers HTTP 200 and carries failures inside
the Response as `status: "failed"` with a KIP `error`
(`InvalidRequestEnvelope`, `UnsupportedCapability`, `IdempotencyConflict`,
`NotFoundOrNotVisible`, `PreconditionFailed`, `ResultLimitExceeded`,
`CursorInvalid`, `NotAuthorized`, `OutcomeUnknown`, `LegalHoldConflict` …). The
helper endpoints use the ordinary HTTP error mapping (404 for an unknown or
foreign handle, 409 for a staging-key conflict); their `RpcError.message` is the
KIP error message and `RpcError.data` the KIP `ErrorObject` with its `code`.

**Handles are scoped to the caller.** Staged sources, idempotency keys, receipts,
retained recall bases and erasure plans belong to the authenticated caller — the
CWT subject, a Space token by name, or the anonymous reader — and the Space.
Another caller, or another Space, gets `NotFoundOrNotVisible`, never a hint that
the handle exists.

```ts
export interface StageSourceInput {
  messages: Message[]; // 1–16 observed messages; a message's `timestamp` (Unix ms) is when it was observed
  observed_at?: string; // RFC 3339, canonicalized to millisecond UTC; defaults to capture time
  kind?: 'message' | 'tool_trace' | 'artifact';
  order?: { stream_ref: string; event_ref: string; ordinal: number; predecessor_receipts?: string[] };
  idempotency_key: string; // same key + same bytes → same handle; other bytes → 409
}
export interface StagedSourceRef { source_ref: string; source_digest: string; captured_at: string }

export interface MemoryRequest {
  kip_memory: '2.0';
  request_id?: string;
  operation: 'observe' | 'recall' | 'revise' | 'feedback' | 'forget';
  space?: { id: string }; // must be this Space when given
  scope?: { task_ref?: string; context_refs?: string[] };
  budget?: { max_output_tokens?: number; deadline_ms?: number; tokenizer?: string };
  idempotency_key?: string; // required on every mutation, refused on recall
  requires?: ('memory_basic' | 'memory_experience' | 'memory_learning')[];
  input: object; // per operation, below
}
```

**Staging before intake.** `observe`, `revise` and `feedback` cite a `source_ref`:
a staged handle (`src-…`) or an existing active Evidence id. Admission runs before
anything is stored: bytes a forget excluded are refused and bind no key.

**Scope.** `task_ref` and `context_refs` are exact handles: an active Concept id of
this Space, or an opaque host string that the host maps to a scope Concept (an
`Event` with `event_class: "memory_scope"`, keyed `memory_scope:<handle>`), created
on the first mutation that names it. The key finds the handle in any lifecycle state
short of purged, so a handle that Maintenance archived, tombstoned or merged still
names its scope. The canonical context set is the task's
Concept plus every context's. Formation writes every claim from a scoped source
with `context: :contexts` (the gate refuses one that does not), puts a MemoryScope
Facet on the Evidence and on the Events, Insights, Experiences and Commitments it
creates, and recall admits a record only when its context set is inside the
request's (KIP Spec §25.3). A task label or topic string never selects scope.

**Idempotency.** A mutation's key is scoped to `(caller, Space, operation)`; its
meaning is the operation, the requested scope, the input and the source's identity
and digest — never `request_id` or `budget`. The same key and meaning returns the
original `receipt` (with current progress) and never re-runs extraction; the same
key with another meaning is `IdempotencyConflict`. Keys, receipts and staged
sources survive restart. When staging omits `observed_at`, a retry reuses the
original observation time rather than assigning a new one.

**Progress.** Each mutation returns an immutable `receipt` (`receipt_ref`,
`operation`, `space_id`, `accepted_seq`) and its current `progress`:

| Phase | Here |
| --- | --- |
| `recorded` | The source and intent are durable and a Formation conversation is queued, running or being retried after a failed attempt (the reason is in `progress.reason`). |
| `available` | The pass completed; search indexes are synchronous, so `available_seq = resolved_seq`. `disposition` is `formed` (it wrote memory), `evidence_only` (it captured Evidence only) or `skipped` (it wrote nothing — an honest answer, reported in `warnings`), or `erased` for a completed forget. |
| `failed` | Terminal: the source was excluded before it ran, a predecessor receipt failed, the pass was interrupted and its outcome is unknown (`OutcomeUnknown`), or a recording repair was refused. Never reported as memory. |

A mutation answers `succeeded` only with available progress, `pending` while
recorded, and `failed` with the error. Pass `budget.deadline_ms` on a mutation to
wait for its pass inside the request. The disposition is decided by the host from
what the pass actually committed, not by the model. `SourceOrder.predecessor_receipts`
must be the caller's receipts; Formation processes the Space's queue in order, and
a successor whose predecessor failed is failed, never formed.

**observe** (`{source_ref}`) runs Formation over the staged messages with the
intent and scope in its prompt; the result is a `FormationResult`
(`summary`, `memory_refs` — the elements it wrote).

**revise** (`{source_ref, target_ref?, change_kind?}`) writes one of three
histories (KIP Spec §14.2):

- `correction` — the actor's earlier claim was wrong: Formation supersedes it by
  the same actor and keeps the corrected interval.
- `world_change` — one new Assertion from the change; temporal succession ends the
  old value. The gate refuses any supersession or retraction in the pass.
- `misrecorded` — the Brain recorded what the actor never said. With `target_ref`
  (the wrong Assertion) the host runs **recording repair** (Spec §57.8): the pass
  may write the claim the original source actually made, citing it (`:orig`) with
  the original source's `asserted_at`; the host then invalidates the target
  (`_system.recording_validity: invalidated`) with those replacements in one
  protected transaction. Nothing is superseded or retracted, and the actor's
  lifecycle and the source bytes are unchanged. Without `target_ref` the report is
  preserved as Evidence and the response is `partial`.
- `unspecified` (default) — recorded as new claims only; never superseded on a
  guess, and the response says so in `warnings`.

**feedback** (`{source_ref, decision_ref?, attempt_ref?}`) is captured by the host
as Evidence classed by the role that was staged — an assistant's self-report is
`agent_statement`, a person's is `user_statement` — never an Outcome, and grades
nothing. The captured Evidence carries the request’s scope, including when the
source was already captured as Evidence. `decision_ref` / `attempt_ref` must be elements of this Space.

**forget** (`{target_ref, mode}`) runs an ErasurePlan (Spec §60.7):

- `payload_only` purges an Evidence payload (`E-…`), or a staged source's bytes and
  the Evidence captured from it.
- `semantic` — the owner's decision — erases a claim through the product deletion
  closure (its tuple, every Assertion on it, their Evidence and recorded
  dependents), purges uncited Evidence or the named element, suppresses the
  sources so Formation never re-ingests them, and scrubs the host's copies: staged
  bytes, Formation transcripts, Recall transcripts that quoted an erased element,
  usage-ledger rows and the probe cache. Retained recall bases that delivered an
  erased element are listed in `external_exports`.

The result is a `ForgetResult` (`status`, `plan_ref`, `summary`, `coverage_ref`).
`completed` (with an `erased` disposition) is reported only after the Nexus
validated the plan against its storage and every host surface was verified; a
legal hold is `blocked`; a target whose sources cannot be enumerated is `partial`.
A source target also covers narrative Concepts newly created by its processing
trace (Events, Insights, Experiences, Commitments), without deleting shared
identity/option Concepts. Missing ownership traces or unfinished processing make
coverage partial. Read the plan at `GET …/memory/plans/{plan_ref}`. `POST /memory/forget` remains the
technical element-purge endpoint without a plan.

**recall** (`{query?, target_ref?, mode?, goal?, context?, after?, detail?, time?,
attention_cursor?}`) returns a `Briefing`:

- `after` receipts must be the caller's (else `NotFoundOrNotVisible`); the host
  waits for each until `deadline_ms`. Unfinished ones are listed in
  `coverage.pending_receipts` and make the response `pending` with
  `action_eligible: false`; failed ones are reported in `uncertainties`. A fixed
  `time.as_of_seq` older than an `after` receipt's `available_seq` is
  `PreconditionFailed`.
- `mode: "attention"` returns the attention raised in scope after
  `attention_cursor` (the same items as `GET /memory/attention`) and a new cursor;
  it runs no model and writes nothing. `mode: "resume"` adds that page to a scoped
  briefing (the minimal resume: no WorkingState is maintained yet).
- `answer` (default) and `action` run one Recall pass for the question; its prose
  becomes the `summary` only when everything it cited is in scope. The items are
  built by the host: each cited claim is re-read through `BELIEF` under the
  request's context set, `time.valid_at` (`FOR TIME`) and `time.as_of_seq` (`AS OF
  SEQ`), so `epistemic_status` is final belief (`insufficient` is never a no);
  raw Evidence is `source`; claims the Brain misrecorded are excluded.
- The seven channels: `constraints` (exact: Insights with `insight_class:
  "constraint"` in scope) and `commitments` (exact: pending/blocked Commitments)
  are required and never dropped for budget; `failures`, `experiences` and
  `skills` are bounded searches for the question (approximate, complete for their
  declared plan); `dependencies` checks the returned items' own
  `_system.dependency_validity`; `evidence` is the Recall pass. Procedures are
  labeled `standing: "unproven"` (or `revoked`) and grant nothing.
- `action_eligible` needs complete coverage, satisfied barriers and no unverified
  precondition (in `action` mode a contested or uncertain fact is one); it
  describes memory sufficiency, never permission.
- `budget.max_output_tokens` bounds the serialized briefing under the advertised
  tokenizer, metadata included. Optional items are dropped first (their channel
  becomes `incomplete`); when the required ones do not fit the result is
  `ResultLimitExceeded`. Another tokenizer is `UnsupportedCapability`.
- Each briefing is retained behind `basis_ref`: the ProjectionBasis, the
  RecallCoverage with one RecallPlan per channel, and the element versions behind
  each item. `target_ref` (a `basis_ref` or an item `ref`) with `detail:
  "evidence"` returns them in `details`, reading each element at the version that
  produced the item; a changed or erased one is reported unavailable, never
  replaced by a newer version. An expansion has its own default budget (65,536
  tokens) and is never action-eligible.
- Recall writes no memory. Returned elements are recorded as `retrieved` in the
  Nexus exposure log (Spec §66.8), which is not cognition and reinforces nothing.

```ts
export interface MemoryResponse {
  kip_memory: '2.0';
  request_id?: string;
  operation: MemoryRequest['operation'];
  status: 'succeeded' | 'pending' | 'partial' | 'failed';
  receipt?: { receipt_ref: string; operation: string; space_id: string; accepted_seq: number };
  progress?: {
    receipt_ref: string;
    phase: 'recorded' | 'processed' | 'available' | 'failed';
    disposition?: 'formed' | 'evidence_only' | 'skipped' | 'erased';
    resolved_seq?: number; available_seq?: number; reason?: string; error?: KipError;
  };
  result?: unknown; // FormationResult | ForgetResult | Briefing
  error?: KipError;
  warnings: string[];
}
```

MCP exposes the same binding as `anda_brain_memory` (`{request}`),
`anda_brain_stage_memory_source` and `anda_brain_memory_receipt`.

Rust briefings pin host reads and retained elements to one snapshot; concurrent
index changes are reported as incomplete search coverage. Summary validation
includes Evidence reads and invalidated extractions, without adding provenance
records to the memory usage ledger.

Known limits: `memory_experience` needs a KIP-CognitiveMemory Nexus (computed
GradingState and lineage views and selection dependencies are not built yet), so it
is not advertised; `resume` has no WorkingState; Formation that is interrupted by a
close is reported `failed` with an unknown outcome and is not re-run; a
misrecording can be repaired only for an extraction whose source bytes are held
inline; the conformance adapter's real-model run is not recorded as evidence.

---

## 4) Endpoint List

## 4.1 Public Endpoints

### GET `/favicon.ico` and GET `/apple-touch-icon.webp`

- Description: Static product icons
- Auth: None
- Response: `image/x-icon` or `image/webp`

### GET `/info`

- Description: Service information
- Auth: None
- Response (JSON): `ServiceInfo`

### GET `/SKILL.md`

- Description: Returns the skill description in Markdown
- Auth: None
- Response: `text/markdown`

---

## 4.2 Space Business Endpoints (`/v1/{space_id}`)

### POST `/v1/{space_id}/formation`

- Purpose: Submit a memory formation task
- Auth: SpaceToken/CWT `write`
- Request body: `FormationInput` (raw string is also accepted in Markdown mode)
- Observation time accepts an RFC 3339 instant in any offset with up to millisecond precision and canonicalizes it to `YYYY-MM-DDTHH:mm:ss.SSSZ`; an unparseable value or sub-millisecond precision is rejected (400) rather than replaced. A missing timestamp uses the stored conversation creation time; retries use the same fallback. A message's own `timestamp` (Unix ms) is that message's observation time. Formation writes each claim's `asserted_at` from the observation time of the message it cites, never from when formation ran (KIP Spec §13.2). Markdown text is captured verbatim as one user message with the same `:msg1` Evidence binding.
- Response (JSON/CBOR): `RpcResponse<AgentOutput>`
- Response (Markdown): `string` (returns only `AgentOutput.content`)

### POST `/v1/{space_id}/recall`

- Purpose: Recall memory via natural-language query
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token). Label-restricted space tokens receive `403` because agentic Recall can traverse all wiki labels.
- Request body: `RecallInput` (raw string is also accepted in Markdown mode)
- Response (JSON/CBOR): `RpcResponse<AgentOutput>`
- Response (Markdown): plain `AgentOutput.content`

### POST `/v1/{space_id}/recall_structured`

- Purpose: Return a synthesized answer with trace-derived memory citations, `found`, and optional uncertainty.
- Auth and request body: same as `/recall`; label-restricted tokens receive `403`.
- Response (JSON/CBOR): `RpcResponse<RecallOutput>`
- Response (Markdown): plain `RecallOutput.answer`

<a id="recall-budget-contract"></a>

### Recall budget contract

Optional `budget` enables a host-selected JSON memory packet in `content` rather
than a free-form answer. `memory_policy.recall_budget` can enforce the same
limits for every Recall; a request may tighten them but cannot raise or disable
the policy. An absent/null policy and request preserve the legacy response.

Each request first runs a bounded, parameterized concept search for its query.
Unresolved commitments (`pending`/`blocked`) remain mandatory; terminal
commitments are optional historical candidates. Returned element views retain
IDs, versions and provenance, omit duplicate LegacyRecord bodies, and list
omitted fields in `recall_detail`. Required attributes and native procedure
checks are never summarized away. The complete Primer remains planning context;
the delivered packet carries its compact execution basis. Explicit KQL field
projections can fetch details when needed.

Whole optional candidates are admitted independently; `coverage.partial` and
`coverage.omitted` identify incomplete delivery. If the cumulative model-input
budget is exhausted, the host can still return its authorized read candidates
with a required warning naming `recall_context_budget_exhausted`. This is a
partial `bounded` packet, not model synthesis or proof of relevance. A missing
required read or an output budget too small for all constraints/warnings still
returns `budget_insufficient`. Provider failures remain failures.

The fixed codec counts the entire compact packet, including escaping and
coverage. `context_tokens` additionally bounds the cumulative versioned
serialization of planner input across this Recall. Provider message templates,
billing and RPC/MCP transport replicas are outside these scopes. No model-name
tokenizer guessing or heuristic fallback is used. Diagnostic histories, thoughts,
artifacts and tool calls are not returned alongside the budgeted packet.

`recall_structured` puts this same packet in `answer`, leaves trace citations
out of the envelope, and adds `memory_budget` with `tokenizer`, `token_limit`,
`tokens`, and `context_token_limit`; `found` means non-primer candidates were
delivered, not that semantic relevance or completeness was proved. Markdown
returns the same packet text. `budget_insufficient` or literal `null` with a
static `failed_reason` is unusable/incomplete, not a successful empty answer.
When it fits, an optional `failed_reason` inside the packet exposes the same
static code, so model/provider failures are distinguishable from token exhaustion.
It is included in the packet token count; provider error bodies are never delivered.
Budget-mode failures use fixed codes: `recall_output_budget_exhausted`,
`recall_required_read_incomplete`, `recall_context_budget_exhausted`,
`recall_deadline_reached`, `recall_model_unavailable`,
`recall_planner_incomplete`, or `recall_procedure_window_incomplete`.
A packet is always a candidate read (`semantic_complete=false`, `action_ready=false`).
Required constraints and warnings are an indivisible set; if they do not fit,
ordinary memories are withheld. No fallback token estimate or free-form answer
bypasses this contract.

### POST `/v1/{space_id}/maintenance`

- Purpose: Trigger maintenance (sleep/consolidation)
- Auth: SpaceToken/CWT `write`
- Request body: `MaintenanceInput`
- Response: `RpcResponse<AgentOutput>`

Explicit `parameters` override the space policy; omitted members use policy defaults. The same effective parameters reach deterministic settlement and the model, without changing the persisted space policy.

### POST `/v1/{space_id}/memory/pin`

- Purpose: Pin or unpin one graph entity (a `pinned` retention class, kept out of retention archival).
- Auth: SpaceToken/CWT `write`
- Request body: `MemoryPinInput`; `pinned` defaults to `true`.
- Response: `RpcResponse<MemoryPinOutput>`

### POST `/v1/{space_id}/memory/forget`

- Purpose: Physically remove graph entities; inspect `dry_run: true` before deletion.
- Auth: SpaceToken/CWT `write`
- Request body: `MemoryForgetInput`; `dry_run` defaults to `false`.
- Response: `RpcResponse<MemoryForgetReport>`; per-entity errors appear in `result.entities`.

Accepts explicit Concept (`C-*`), Proposition (`P-*`), Assertion (`A-*`), Evidence (`E-*`) and Activity (`X-*`) IDs, including the Evidence containing captured message text. Native legal holds and reference checks still apply; successful purges leave erased identity stubs. Counts report each erased kind, including cascades. This removes the selected graph records; stored conversations, wiki documents and external copies have separate lifecycles.
Saved product previews that reference successfully purged elements are scrubbed before the response is reported as clean; a preview-cleanup failure is reported on that entity.

### GET `/v1/{space_id}/memory/attention`

- Purpose: Attention recall (KIP Memory Interface §4): the Watches that fired and the Commitments the settlement found due, after the cursor the caller kept.
- Auth: SpaceToken/CWT `read`; public spaces permit anonymous reads.
- Query: `AttentionRecallInput` — `attention_cursor` (optional), `limit` (1–50 items, default 20).
- Response: `RpcResponse<AttentionRecall>`, with existing JSON/CBOR/Markdown negotiation. An invalid cursor or limit is 400.

```ts
export interface AttentionRecallInput {
  attention_cursor?: string; // opaque; absent, "attention:start" or the older "attention:-1" reads from the first raise
  limit?: number; // 1–50 items; default 20
}

export interface AttentionRecall {
  items: AttentionItem[]; // ordered by (raised_seq, ref)
  // The last delivered position: "attention:<seq>:<ref>" when the page stopped
  // inside one commit, "attention:<seq>" when every item up to that commit was
  // delivered, the input cursor (or "attention:start") when nothing was new.
  attention_cursor: string;
  complete: boolean; // every raise after the input cursor was read
}

export interface AttentionItem {
  ref: string; // the Watch or Commitment raised
  kind: 'watch_fired' | 'commitment_due';
  summary: string;
  raised_seq: number; // space_seq of the watch_fire / commitment_review Activity
  due_at?: string;
  target_refs: string[]; // a Watch's `watches` targets, or the Commitment
  priority?: number;
}
```

Every item is raised by a commit: a `watch_fire` Activity, or a `commitment_review` Activity naming a due Commitment. The settlement raises each due `pending`/`blocked` Commitment without a Watch natively, with one Activity keyed `commitment_review:<commitment id>:<due_at>` (Profile §17): a replay raises nothing, and only a new `due_at` raises the Commitment again. The Commitment's status is not changed. One commit may raise several items, and a page may stop between them; the next page continues after the last item delivered. Reading changes nothing in memory, and the cursor does not expire; the caller keeps it once it has taken the items. An item grants nothing: acting on it passes the action gate and Governance like any other act. This is separate from the authenticated runtime inbox (`GET /v1/{space_id}/attention`), which pages the action gate's wake records.

### GET `/v1/{space_id}/schema/drafts`

- Purpose: The Space's draft vocabulary (KIP §20.16): every symbol Formation drafted with `DEFINE` (or the deprecated `declare_memory_symbols`), its definition and, once promoted, the lineage it joined.
- Auth: SpaceToken/CWT `read`; public spaces permit anonymous reads.
- Response: `RpcResponse<SchemaDrafts>`.

```ts
export interface SchemaDrafts {
  package_ref: 'kip://local/draft@0.0.0';
  schema_environment_version: number;
  symbols: DraftSymbol[];
}

export interface DraftSymbol {
  kind: 'ConceptType' | 'PredicateType';
  name: string;
  ref: string; // kip://local/draft@0.0.0/<name>; elements keep it forever
  definition: object; // as drafted
  promoted_to?: string; // the target lineage, e.g. kip://profiles/cognitive-memory/Person
}
```

Each new draft queues one `review_schema` SleepTask keyed `review_schema:<kind>:<ref>` (with `symbol_kind` / `symbol_ref` attributes). Maintenance reviews it and may propose a promotion in its summary; it never defines or promotes a symbol. A Formation request that defines symbols contains only `DEFINE`s (at most eight), with literal names and bodies in this deployment's name shapes; the Space holds at most 512 symbols of its own.

### POST `/v1/{space_id}/schema/promote`

- Purpose: Promote one draft symbol onto an installed symbol of the same kind (KIP §20.16). This is a Schema migration: elements written under the draft keep their exact `schema_ref` / `predicate_ref`, and type and predicate matching read the two lineages as one from the new Schema Environment version on. A draft is promoted at most once; nothing promotes implicitly.
- Auth: management CWT `write` (it stands for `manage_schema`); Space tokens cannot promote.
- Request body: `{kind: 'ConceptType' | 'PredicateType', from: string, to: string}` — `from` is the draft's local name or exact ref, `to` the installed symbol's exact ref or an unambiguous local name.
- Response: `RpcResponse<{promoted: string, to: string, schema_environment_version: number}>`. An unknown draft, a kind mismatch, an unresolved target or a repeat promotion is 400.

### GET `/v1/{space_id}/memory_status`

- Purpose: Read memory statistics and the latest maintenance report.
- Auth: SpaceToken/CWT `read`; public spaces permit anonymous reads.
- Response: `RpcResponse<MemoryStatus>`, with existing JSON/CBOR/Markdown negotiation.
- `result.last_settlement.correction_scan_incomplete`: the bounded scan has not proved the backlog exhausted; later maintenance resumes within the transaction.
- `result.last_settlement.correction_scan_through_seq`: largest transaction sequence completely read.
- `result.last_settlement.correction_scan_error`: discovery failure; failed scans retain their cursor.
- These fields describe correction discovery, not completed model review or authorized Watch coverage.

### POST `/v1/{space_id}/execute_kip_readonly`

- Purpose: Execute a KIP 2.0 request (read-only: KQL and META)
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token)
- Request body: `KipRequest`, or a bare JSON string read as one command
- A batch with more than one `operations` entry must declare `execution.mode`; check `status` and each `results[].status` even when HTTP returns `200`.
- Response: `KipResponse<T>` (the result type follows the commands)
- Read-only is enforced on what each command *parses to*, so a KML mutation is refused here however the request labels it

### POST `/v1/{space_id}/get_or_init_user`

- Purpose: Get or initialize a user concept node for the given principal
- Auth: SpaceToken/CWT `write`
- Request body: `GetOrInitUserInput`
- Omitting `name` preserves an existing display name. An explicit `name` updates it; new unnamed users use their key as the initial display name.
- Response: `RpcResponse<Concept>`

### GET `/v1/{space_id}/info`

- Purpose: Get space status and statistics
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token)
- Response: `RpcResponse<SpaceInfo>`

### GET `/v1/{space_id}/status`

- Alias for `/v1/{space_id}/info`, with the same authorization and response.

### POST `/v1/{space_id}/probe`

- Purpose: model-free retrieval reachability; `found:false` is not a rejected belief.
- Auth: the existing Space `read` permission and public-space read rules.
- Request: `{"query":"...", "limit":8}`.
- Response: `RpcResponse<ProbeOutput>` with `found`, `negative_cached`, optional `hits` and optional `search_exhaustive`. Coverage comes from the SEARCH result's top-level field; omission means unknown and remaining pagination means false. The negative cache stores only explicitly exhaustive search misses, not proof of absence or rejected belief.

### GET `/v1/{space_id}/formation_status`

- Purpose: Get formation status
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token)
- Response: `RpcResponse<FormationStatus>`
- This is lightweight monitoring; cursors are not per-job success evidence. Rust hosts can use `Space::processing_report` / `wait_for_processing` to distinguish queued, running, failed, cancelled, interrupted and timed-out work. Reconcile the same conversation ID after timeout instead of resubmitting.

### GET `/v1/{space_id}/conversations/{conversation_id}?collection=<collection>`

- Purpose: Get a single conversation detail
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token); tokens restricted by ACL labels are rejected with `403` — conversations persist the full agent runner history, which is not label-scoped; `collection=recall` additionally rejects anonymous access on public spaces with `403` (recall runs from a private era may embed labeled wiki content)
- Query:
  - `collection?: string` // "formation" (default), "recall" or "maintenance"; unknown values are rejected with `400`
- Response: `RpcResponse<Conversation>`

### GET `/v1/{space_id}/conversations/{conversation_id}/delta?collection=<collection>&messages_offset=<n>&artifacts_offset=<n>`

- Purpose: Get incremental conversation updates after client-side offsets
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token); tokens restricted by ACL labels are rejected with `403` (`collection=recall`: anonymous access on public spaces is also rejected)
- Query:
  - `collection?: string` // "formation" (default), "recall" or "maintenance"; unknown values are rejected with `400`
  - `messages_offset?: number` // returns only messages after this offset, defaults to `0`
  - `artifacts_offset?: number` // returns only artifacts after this offset, defaults to `0`
- Response: `RpcResponse<ConversationDelta>`

### GET `/v1/{space_id}/conversations?collection=<collection>&cursor=<cursor>&limit=<n>`

- Purpose: List conversations with pagination
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; private spaces require a valid token); tokens restricted by ACL labels are rejected with `403` (`collection=recall`: anonymous access on public spaces is also rejected)
- Query:
  - `collection?: string` // "formation" (default), "recall" or "maintenance"; unknown values are rejected with `400`
  - `cursor?: string`
  - `limit?: number`
- Response: `RpcResponse<Conversation[]>` (next page cursor is returned via `next_cursor`)

---

## 4.3 Wiki Endpoints (`/v1/{space_id}/wiki`)

The wiki is the space's versioned reference memory (policies, manuals, SOPs, API docs). Writes are git-like immutable commits with CAS concurrency control; searches return verifiable `wiki://` citations. ACL: documents may carry an `acl_label`; space tokens with `labels` see unlabeled content plus their granted labels — prefiltered in the query and checked against the current document version, state and ACL. Authorization and version selection use the same document snapshot. Anonymous readers of public spaces see unlabeled content only; denials surface as 404.

Wiki-specific error semantics: `409` commit conflict (`RpcError.data.current_version` carries the version to rebase on), `413` content over 1 MiB, `404` not found / ACL-denied.

The TOC comes from Markdown ATX headings (`#`–`######`) independently of retrieval chunks; a section includes its descendant headings. Anchors derive from heading names, with numeric suffixes for duplicates. `full`, `range` and `section` return at most 256 KiB. When `truncated` is true, continue with the returned `byte_range`. History and citation verification accept only versions in the published parent chain; incomplete commits are not history.

OKF imports replace the file-owned title, tags, resource, type and unknown frontmatter values; deleting a field clears its imported state. Existing ACLs and unrelated host metadata are preserved. Export retains unknown key/value pairs even after commit changes the title or tags. Standard YAML parsing and serialization preserve values, not comments or original formatting.

WikiDigest is disabled by default. When enabled, commit, archive, restore and ACL changes persist a per-document pending flag. Startup, post-maintenance hooks and explicit calls process up to 20 documents per run; failed documents remain pending for retry. An unchanged body checksum reuses the ledger without a model call. Omitted extraction results do not withdraw claims: every source batch must explicitly review an old claim as `absent`; missing, duplicate or `unknown` reviews retain it. Archive or labeling causes a later digest to retract this source's Assertions; graph synchronization is asynchronous. Changes during extraction fence out the stale graph write and leave the new generation queued. The observed `wiki_digested` high-water mark does not mean the queue is empty.


### POST `/v1/{space_id}/wiki/docs`

- Purpose: Commit a document (create, or CAS update with `doc_id` + `parent_version`); identical content is a no-op
- Auth: SpaceToken/CWT `write`
- Request body: `WikiCommitInput` (raw Markdown string is also accepted; the title derives from the first heading)
- Response: `RpcResponse<WikiCommitOutput>`

### GET `/v1/{space_id}/wiki/docs?namespace=<ns>&status=<status>&tag=<tag>&cursor=<cursor>&limit=<n>`

- Purpose: List documents with pagination
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Response: `RpcResponse<WikiDocInfo[]>` (next page cursor via `next_cursor`)

### GET `/v1/{space_id}/wiki/docs/{doc_id}`

- Purpose: Document metadata plus table of contents
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Response: `RpcResponse<{ doc: WikiDocInfo; toc: WikiTocEntry[] }>`

### GET `/v1/{space_id}/wiki/docs/{doc_id}/content?version=<id>&anchor=<anchor>&start=<n>&end=<n>`

- Purpose: Progressive reading — `anchor` reads one section, `start`+`end` a byte range, neither reads the bounded full text; `version` time-travels
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Response: `RpcResponse<WikiReadOutput>`

### GET `/v1/{space_id}/wiki/docs/{doc_id}/versions?cursor=<cursor>&limit=<n>`

- Purpose: Version history (immutable commit chain)
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Response: `RpcResponse<WikiVersionInfo[]>` (next page cursor via `next_cursor`)

### POST `/v1/{space_id}/wiki/docs/{doc_id}/archive`

- Purpose: Archive a document (hidden from search, still readable by id, restorable)
- Auth: SpaceToken/CWT `write`
- Response: `RpcResponse<WikiDocInfo>`

### POST `/v1/{space_id}/wiki/docs/{doc_id}/restore`

- Purpose: Restore an archived document into search
- Auth: SpaceToken/CWT `write`
- Response: `RpcResponse<WikiDocInfo>`

### POST `/v1/{space_id}/wiki/search`

- Purpose: BM25 keyword retrieval over document passages, returning snippets with verifiable citations
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Request body: `WikiSearchInput` (raw query string is also accepted)
- Response: `RpcResponse<WikiSearchOutput>`

### POST `/v1/{space_id}/wiki/verify`

- Purpose: Verify a citation against immutable stored content
- Auth: SpaceToken/CWT `read` (public spaces are unauthenticated; ACL labels apply)
- Request body: `WikiVerifyInput` (raw `wiki://` URI string is also accepted)
- Response: `RpcResponse<WikiVerifyOutput>`

### GET `/v1/{space_id}/wiki/events?kind=<kind>&doc_id=<id>&cursor=<cursor>&limit=<n>`

- Purpose: Query the append-only audit log (writes, imports, digests; reads too when `wiki_audit_reads` is enabled)
- Auth: SpaceToken/CWT `read`; tokens restricted by ACL labels are rejected with `403`
- Response: `RpcResponse<WikiEventInfo[]>` (next page cursor via `next_cursor`)

### POST `/v1/{space_id}/wiki/import`

- Purpose: Import an OKF v0.1 bundle; checksum-idempotent (re-imports never grow version chains); unknown frontmatter key/value pairs survive edits and round-trip structurally; comments and formatting are canonicalized
- Auth: SpaceToken/CWT `*` (full scope)
- Request body: `WikiImportInput`
- Response: `RpcResponse<WikiImportOutput>`

### GET `/v1/{space_id}/wiki/export?namespace=<ns>`

- Purpose: Export one namespace as an OKF bundle (concept `.md` files + `index.md` + `manifest.json` with checksums); replayable into an empty space
- Auth: SpaceToken/CWT `*` (full scope)
- Response: `RpcResponse<WikiExportOutput>`

### POST `/v1/{space_id}/wiki/digest`

- Purpose: Distill pending wiki versions into the Cognitive Nexus — each fact becomes a Proposition plus an Assertion attributed to the Brain, citing its passage as Evidence. An old claim explicitly reviewed as unsupported across every source batch has the digest's own Assertion retracted; omitted or unknown reviews do not retract it; the Proposition and anyone else's Assertions about it are untouched. Requires `update_space {"wiki_digest": true}`
- Auth: SpaceToken/CWT `write`
- Response: `RpcResponse<WikiDigestReport>`

---

## 4.4 Space Management Endpoints (`/v1/{space_id}/management`)

### GET `/v1/{space_id}/management/space_tokens`

- Purpose: List Space Tokens
- Auth: Must pass CWT `write` (user management-level auth)
- Response: `RpcResponse<SpaceToken[]>` — the `token` field is redacted to a display prefix (e.g. `STabc123…`); full token values are only returned once, by `add_space_token`. Save them at mint time, or revoke by `name`.

### POST `/v1/{space_id}/management/add_space_token`

- Purpose: Add a Space Token
- Auth: Must pass CWT `write` (user management-level auth). Minting a `*` (full-scope) token requires a `*`-scoped CWT — a `write` CWT cannot mint tokens above its own scope.
- Request body: `AddSpaceTokenInput` — `name` is required and unique within the space. `labels` is allowed only for `read` tokens; `[]` restricts reads to unlabeled wiki content, while omission is unrestricted.
- Response: `RpcResponse<SpaceToken>` (new token, always prefixed with `ST`; this is the only response that carries the full token value)

### POST `/v1/{space_id}/management/revoke_space_token`

- Purpose: Revoke a Space Token
- Auth: Must pass CWT `write` (user management-level auth)
- Request body: `RevokeSpaceTokenInput` — either `token` (the full token value) or `name` (the unique token name, for managers who did not save the value at mint time)
- Response: `RpcResponse<boolean>` (whether revocation succeeded)

### PATCH `/v1/{space_id}/management/update_space`

- Purpose: Update space information, wiki settings, and the optional `memory_policy` (validated and persisted as a replacement policy).
- Auth: Must pass CWT `write` (user management-level auth)
- Request body: `UpdateSpaceInput`
- Response: `RpcResponse<true>`

### POST `/v1/{space_id}/management/shadow_eval`

- Purpose: Compare a candidate memory policy against the current one by replaying recent Recall queries on forks. This can make multiple model calls.
- Auth: CWT `write` (space tokens are not accepted).
- Request body: `ShadowEvalInput`; `replay_sample` defaults to the space policy and is capped at `16`.
- Response: `RpcResponse<ShadowReport>`

### PATCH `/v1/{space_id}/management/restart_formation`
- Purpose: Restart a formation task by conversation ID (for failed/stale formations)
- Auth: Must pass CWT `write` (user management-level auth)
- Request body: `FormationRestartInput`
- Response: `RpcResponse<true>`

### GET `/v1/{space_id}/management/space_byok`
- Purpose: Get BYOK (Bring Your Own Key) configuration, i.e., use custom model configuration
- Auth: Must pass CWT `write` (user management-level auth; response includes provider credentials)
- Response: `RpcResponse<ModelConfig>`

### PATCH `/v1/{space_id}/management/space_byok`
- Purpose: Update BYOK (Bring Your Own Key) configuration, i.e., use custom model configuration
- Auth: Must pass CWT `write` (user management-level auth)
- Request body: `ModelConfig`
- Response: `RpcResponse<true>`

---

## 4.5 Admin Endpoints (`/admin`)

### POST `/admin/create_space`

- Purpose: Create a space
- Auth: Platform admin + CWT `write`
- Request body: `CreateOrUpdateSpaceInput`
- Response: `RpcResponse<SpaceInfo>`

### POST `/admin/{space_id}/update_space_tier`

- Purpose: Update space tier
- Auth: Platform admin + CWT `write`
- Request body: `CreateOrUpdateSpaceInput`
- Response: `RpcResponse<SpaceTier>`

---

<a id="authenticated-runtime-inbox-and-observations"></a>

## 4.6 Runtime Endpoints (authenticated inbox and observations)

Configure `BRAIN_RUNTIME_CONFIG` before startup. See [runtime setup, contracts,
recovery and examples](RUNTIME.md) and [runtime.example.json](runtime.example.json).
These endpoints require real credentials even on public/local Spaces.

| Endpoint | Required access | Result |
| --- | --- | --- |
| `GET /v1/{space_id}/attention?limit=20&cursor=...` | Verified read/* credential, explicit native identity and audience | `AttentionPage`; reading never claims work |
| `POST /v1/{space_id}/attention/{id}/responses` | Verified write/*, current visibility and correct recipient | `ResponseReceipt`; body `AttentionResponse` |
| `POST /v1/{space_id}/outcomes` | Signed observer-mapped CWT + current `record_outcome` + registered contract | `ObservationReceipt`; body `OutcomeInput` |
| `GET /v1/{space_id}/runtime/status` | Verified read/*, explicit mapping when configured | `RuntimeStatus`; counts cover only the visible bounded page |

The URL `id` is the returned wake hash, not its slash-containing `wake_ref`.
Cursor tokens expire after five minutes and are authenticated/encrypted for the
caller/instance/configuration. Cross-caller or stale tokens are rejected. Public
Spaces and ordinary write tokens do not authenticate independent observers. MCP
exposes `anda_brain_get_attention`, `anda_brain_respond_attention` and
`anda_brain_get_runtime_status`; observer writes are not model tools.

POST bodies are structured JSON/CBOR; responses retain JSON/CBOR/Markdown
negotiation. Same event/body is idempotent; changed content returns 409 with retained
audit. Future or wrong-instance input is rejected. Late/correction/safety receipt
status, native persistence and learning eligibility remain separate. Registered
learning measurements must match the existing `OutcomeMeasurements` contract and
are routed exclusively to its controller, including baseline Attempts.

```ts
type RuntimeScope = { space_id: string; space_instance: string };
type AttentionQuery = { cursor?: string | null; limit?: number | null };
type AttentionResponse =
  | { kind: "clarification"; event_key: string; answer: string }
  | { kind: "agent_statement"; event_key: string; statement: string };
type ResponseReceipt = {
  receipt_id: string; status: string; evidence_ref: string | null;
};
type AttentionPage = {
  scope: RuntimeScope; items: AttentionItem[];
  next_cursor: string | null; complete: boolean;
};
type AttentionItem = {
  id: string; wake_ref: string; parent_id: string | null;
  watch_ref: string; fire_activity_ref: string; summary: string;
  state: "pending" | "running" | "blocked" | "completed" | "cancelled";
  reason: string | null; decision: Record<string, unknown> | null;
  decision_ref: string | null; attempt_ref: string | null;
  dispatch_ref: string | null; clarification: Record<string, unknown> | null;
  delivery: Record<string, unknown> | null;
};
type OutcomeStatus = "success" | "partial" | "failure" | "aborted" | "unknown";
type OutcomeInput = {
  space_instance: string; attempt_ref: string;
  observer_configuration_digest: string; event_key: string;
  observed_at: string; metric: string; window: string;
  observation:
    | { kind: "measurement"; terminal: boolean; outcome_status: OutcomeStatus;
        magnitude?: number | null; payload: unknown }
    | { kind: "learning"; measurements: Record<string, unknown> };
  correction_of?: string | null; safety_signal?: string | null;
  utility?: { witness_ref?: string; witness?: ContributionWitness } | null;
};
type ObservationReceipt = {
  format: "anda-brain:observation-receipt-v1";
  receipt_id: string; scope: RuntimeScope; event_key: string;
  body_digest: string; observer: string; received_at_ms: number;
  observed_at: string; status: string;
  native_committed: boolean; learning_eligible: boolean;
  outcome_status: OutcomeStatus | null; outcome_ref: string | null;
  observation_ref: string | null; reason: string | null; safety_pending: boolean;
  safety_evaluation_ref?: string; // Native revocation that resolved/covered this signal.
};
type RuntimeStatus = {
  supported: boolean; configured: boolean; scope: RuntimeScope | null;
  attention_enabled: boolean; actions_enabled: boolean;
  observation_enabled: boolean; observer_authenticated: boolean;
  blocked_reasons: string[]; visible_items: number; inventory_complete: boolean;
  utility?: UtilityStatus;
  learning?: LearningRuntimeStatus;
  semantic_attention?: SemanticAttentionStatus;
  trust?: TrustRuntimeStatus;
};
```

### Observation and response semantics

Attention pages contain 1–50 items and at most 256 KiB of visible output. `complete`
means the end of this snapshot walk, not task completion or semantic completeness.
Each page rechecks current visibility, including native evidence behind gate packets.
MCP responses use `{id, response}` and listing uses `{cursor, limit}`. Neither reads
nor answers claim work. Clarifications require the committed ask, its recipient and
an unexpired deadline; a response enters a fresh gate without granting authority.
An `agent_statement` creates attributed Evidence and an Activity, never an independent
OutcomeRecord. For example:

```json
{"kind":"clarification","event_key":"answer-42","answer":"Tomorrow"}
```

A measurement body uses real retained identities and digests:

```json
{
  "space_instance":"sha256:INSTANCE_DIGEST",
  "attempt_ref":"X-42",
  "observer_configuration_digest":"sha256:REGISTERED_METHOD_DIGEST",
  "event_key":"instrument-event-42",
  "observed_at":"2026-09-17T10:00:00.000Z",
  "metric":"delivery",
  "window":"durable_inbox_v1",
  "observation":{
    "kind":"measurement",
    "terminal":true,
    "outcome_status":"success",
    "magnitude":null,
    "payload":{"delivery_digest":"sha256:ACTUAL_DELIVERY_REQUEST_DIGEST"}
  },
  "correction_of":null,
  "safety_signal":null
}
```

The observer contract pins principal, method/configuration digest, control domain,
task family, metric, window and allowed delay. Intake verifies the actual native
act/Attempt, transaction author, prior dispatch and observation time; the controller
cannot observe its own Attempt. Inbox success also requires the persisted delivery
and matching digest/time. It proves durable inbox delivery, not human reading or
business success. Other callbacks require their own independent measurement contract.

Accepted ordinary measurements atomically create Outcome Evidence and an
`outcome_observation` Activity. Raw material stays in Evidence payload, outside the
closed OutcomeRecord Facet. Nonterminal/unknown observations do not finish dispatch;
missing magnitude/cost remains missing. Terminal evidence reconciles the same
Attempt, including after cancellation. Native evidence and dispatch reconciliation
are separate recoverable steps; retry an unresolved write with the same event/body
and fresh authentication. Caller cancellation does not interrupt admitted writes.

Authenticated late, conflicting and nonterminal learning material remains auditable
without changing old trial eligibility. Corrections name an earlier event from the
same observer/Attempt and add new evidence; they never rewrite old Outcome/Evaluation
records. Safety signals remain discoverable when late/excluded. An explicitly enabled
learning consumer uses fresh observer authority, resolves native revocation, then
acknowledges `safety_pending` and records `safety_evaluation_ref`.

For registered learning Attempts, use `observation:{"kind":"learning", "measurements":{...}}`.
Membership comes from the retained ticket, including baseline Attempts with
`trial_ref:null`. Only `LearningRuntime::submit_outcome` writes the native result;
frozen cutoff, comparability and ACK recovery remain intact. An unavailable controller
causes refusal, never a generic fallback. `learning_eligible:true` means contract
acceptance, not a positive grade. Inputs are capped at 16 KiB and event keys at
256 bytes. See the [runtime guide](RUNTIME.md) for identity setup and storage bounds.

### Learning runtime status

The existing status route and MCP tool add `learning`; no cohort-enrollment or
observer-credential tool is exposed to a model. `capacity` and `last_pass` are null
unless the caller is an explicitly mapped auditor. Maintenance's optional
`skills.runtime` contains the separate scheduler status; its old Skill counters
do not count work from prior scheduler passes. See [the learning guide](LEARNING_RUNTIME.md).

```typescript
type LearningRuntimeStatus = {
  compiled: boolean; registered: boolean; registration_enabled?: boolean;
  bindings_ready: boolean; automatic_allowed: boolean; running?: boolean;
  automation?: { trials: boolean; reviews: boolean; archive: boolean; safety: boolean };
  blocked_reasons?: string[];
  capacity?: { hot_jobs: number; maximum_hot_jobs: number; archived_jobs: number;
    retained_identities: number; reserved_bytes: number;
    storage: { maximum_records: number; maximum_reserved_bytes: number } } | null;
  last_pass?: { started_at_ms: number; finished_at_ms: number; enrolled: number;
    driven: number; observed: number; settled: number; archived: number;
    reviews_checked: number; reviews_enrolled: number; safety_resolved: number;
    blocked: string[] } | null;
};
```

### Delivery and utility metadata

Structured Recall additionally returns optional `recall_receipt` transport metadata;
the answer/semantic packet remains unchanged. Plain/direct-agent calls retain a
receipt discoverable by their conversation ID through the trusted Rust API.
Outcome observers can provide exactly one inline witness or prior native witness
reference in `utility`; this never relaxes signed observer authentication. Runtime
status additionally reports `utility` switches, calibration state and recovery
reason. There is no model-facing setter or new MCP mutation tool. Detailed witness,
method and audit formats are in [UTILITY_RUNTIME.md](UTILITY_RUNTIME.md).

```typescript
type RecallReceiptRef = {
  id: string; digest: string; scope: RuntimeScope;
};
// RecallOutput.recall_receipt?: RecallReceiptRef
// OutcomeInput.utility?: { witness_ref?: string; witness?: ContributionWitness }
// RuntimeStatus.utility?: UtilityStatus
type UtilityStatus = {
  configured: boolean; automatic: boolean; apply: boolean;
  calibrated: boolean; ranking: boolean; running: boolean;
  reason: string | null;
};
```

### Semantic attention status

Runtime status adds optional `semantic_attention` with `configured`, `automatic`,
`running`, `pin`, `reason`, and nullable `last_pass`. The latter contains `scanned`,
`calls`, `input_tokens`, `advanced`, `fired`, `expired`, `deferred`, and `reason` for
one bounded pass, and is returned only to configured auditors. It is not complete
inventory or proof that all text Watches were evaluated. Existing JSON/CBOR/Markdown
and HTTP/MCP request shapes are unchanged. No model configuration or evaluation
write endpoint is added; operators install per-Space `semantic` startup bindings.
See [SEMANTIC_WATCH_RUNTIME.md](SEMANTIC_WATCH_RUNTIME.md) for budget, pin migration,
unknown/deferred semantics, native Artifact erasure and idempotent recovery.

```typescript
type SemanticAttentionStatus = {
  configured: boolean; automatic: boolean; running: boolean;
  pin: { id: string; digest: string } | null; reason: string | null;
  last_pass: {
    scanned: number; calls: number; input_tokens: number; advanced: number;
    fired: number; expired: number; deferred: number; reason: string | null;
  } | null;
};
```

### Contextual trust status and discovery hints

The existing HTTP status route and MCP status tool add optional `trust` metadata.
`governor_authorized` reflects the configured principal's current native authority;
it is not permission granted to the caller. No trust-management or independent-fact
writing tool is added to HTTP/MCP. Existing JSON/CBOR/Markdown requests remain valid.
A normal authenticated measurement payload can optionally include
`trust_verification_ref: "E-…"`; it announces an existing native factual verification
for bounded discovery, without converting action success/failure into a trust score.
Trusted Rust fact intake, review, application, version-conflict recovery and scoped
restoration are documented in [TRUST_RUNTIME.md](TRUST_RUNTIME.md).

```typescript
type TrustRuntimeStatus = {
  configured: boolean; automatic: boolean; apply: boolean; automatic_apply: boolean;
  calibrated: boolean; governor_authorized: boolean; running: boolean;
  reason: string | null;
};
```

---

## 5) MCP Server

By default, the HTTP service exposes a Streamable HTTP MCP endpoint for MCP-capable agents; `MCP_HTTP_ENABLED=false` disables it:

```text
https://your-brain-host/mcp/my_space_001
```

Clients select the target memory space from the URL path and pass the same CWT or space token used by REST as `Authorization: Bearer <token>`. This is the recommended mode for internal multi-user agent platforms where each employee receives a dedicated Brain space.

Anda Brain can also run as a local MCP stdio server:

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" \
  anda_brain mcp --space-id my_space_001 local --db ./data
```

Both MCP modes use the same model, auth, and storage configuration as the HTTP service, and share its model concurrency budget. For stdio, the nested storage subcommand is optional; omit it for in-memory development, use `local --db ./data` for local persistence, or use `aws --bucket ... --region ...` for S3.

| Tool | Input | Output | Scope |
| ---- | ----- | ------ | ----- |
| `anda_brain_memory` | `{ request: MemoryRequest }` | `MemoryResponse` | recall `read`; mutations `write` |
| `anda_brain_stage_memory_source` | `StageSourceInput` | `StagedSourceRef` | `write` |
| `anda_brain_memory_receipt` | `{ receipt_ref }` | `{ receipt, progress, result, warnings }` | `read` credential |
| `anda_brain_remember_conversation` | `FormationInput` shape (`messages`, `context`, `timestamp`) | `AgentOutput` | `write` |
| `anda_brain_recall_memory` | `RecallInput` shape (`query`, `context`, optional `budget`) | `AgentOutput` | `read` |
| `anda_brain_run_maintenance` | `MaintenanceInput` shape | `AgentOutput` | `write` |
| `anda_brain_get_space_info` | none | `SpaceInfo` | `read` |
| `anda_brain_get_formation_status` | none | `FormationStatus` | `read` |
| `anda_brain_execute_kip_readonly` | `{ command?, commands?, parameters?, dry_run? }` | `KipResponse` | `read` |
| `anda_brain_get_or_init_user` | `{ user, name? }` | `Concept` | `write` |
| `anda_brain_list_conversations` | `{ collection?, cursor?, limit? }` | `{ conversations, next_cursor }` | `read` |
| `anda_brain_get_conversation` | `{ conversation_id, collection?, delta?, messages_offset?, artifacts_offset? }` | `Conversation` or `ConversationDelta` | `read` |
| `anda_brain_get_attention` | `{ cursor?, limit? }` | `AttentionPage` | runtime: verified `read` + mapping |
| `anda_brain_respond_attention` | `{ id, response: AttentionResponse }` | `ResponseReceipt` | runtime: verified `write` + mapping |
| `anda_brain_get_runtime_status` | none | `RuntimeStatus` | runtime: verified `read` |
| `anda_brain_wiki_search` | wiki query, filters, and result limit | `WikiSearchOutput` | `read` |
| `anda_brain_wiki_read` | document id and selector | `WikiReadOutput` | `read` |
| `anda_brain_wiki_commit` | document fields and full Markdown content | `WikiCommitOutput` | `write` |
| `anda_brain_wiki_verify` | citation URI or explicit citation fields | `WikiVerifyOutput` | `read` |

The MCP read-only KIP tool uses `commands` and supplies independent execution for batches. The HTTP `/execute_kip_readonly` endpoint uses `operations` and requires an explicit `execution.mode` for batches. Wiki MCP tools are available when the `wiki` feature is enabled (always true for the service binary). The three runtime tools follow the [runtime endpoint](#authenticated-runtime-inbox-and-observations) rules; observer writes are never model tools. No tool schema uses `oneOf`, `anyOf` or `allOf`: alternatives are type lists or one object with an `enum` discriminator, and cross-field rules are checked when arguments are parsed.

When `ED25519_PUBKEYS` is set, configure the remote MCP client with an `Authorization` bearer token, or configure stdio with `MCP_AUTH_TOKEN` / `--mcp-auth-token`. `read` tools can also access public spaces without a token. For remote MCP behind a company domain or reverse proxy, set `MCP_HTTP_ALLOWED_HOSTS` to the accepted Host values (and `MCP_HTTP_ALLOWED_ORIGINS` for browser clients). Use `--mcp-auto-create-space` for local stdio development or `MCP_HTTP_AUTO_CREATE_SPACE=true` for remote development if the target space does not exist yet; remote auto-create requires `ED25519_PUBKEYS` plus a CWT with `write` scope for the target space before the missing space is created.

---

## 6) Error Semantics

- Authentication failure: HTTP `401`, response body is `RpcError`
- Invalid request/parameters: HTTP `400`, response body is `RpcError`; `RpcError.message` is plain human-readable text, not a quoted or debug rendering
- Forbidden access: HTTP `403`; missing space or wiki document: HTTP `404`
- Wiki commit conflicts: HTTP `409`, with the current version in `RpcError.data.current_version`; oversized wiki content: HTTP `413`
- LLM request load shedding: HTTP `429`; global HTTP load shedding: HTTP `503`. These middleware responses are plain text.
- Success: HTTP `200`, response body is usually `RpcResponse<T>`
- Handler errors are JSON even when `Accept` requests CBOR or Markdown. Unmatched routes and middleware errors may have a plain text or empty body. Only success bodies follow `Accept`.
- A KIP request can return HTTP `200` with `KipResponse.status` or an operation's `status` equal to `failed`; inspect both KIP levels.
- MCP tools mirror this classification: caller-fixable failures surface as JSON-RPC `invalid_params`/`invalid_request` (a wiki commit conflict carries the same `data.current_version` retry payload as the HTTP `409` body), and only true internal failures use `internal_error`

---

## 7) TypeScript Type Definitions

```ts
export type TokenScope = 'read' | 'write' | '*';

export interface RpcError {
  message: string;
  data?: unknown;
}

export interface RpcResponse<T> {
  result?: T;
  error?: RpcError;
  next_cursor?: string;
}

export interface InputContext {
  counterparty?: string;
  agent?: string;
  source?: string; // provenance thread/channel; not a submission/message deduplication key
  topic?: string;
}

export type MessageRole = 'system' | 'user' | 'assistant' | 'tool';

export type MessageContentPart =
  | string
  | {
      type: string;
      text?: string;
      [k: string]: unknown;
    };

export interface Message {
  role: MessageRole;
  content: string | MessageContentPart[];
  name?: string;  // user or tool name
  user?: string;  // user ID
  timestamp?: number; // Unix timestamp in milliseconds
}

export interface FormationInput {
  messages: Message[]; // must contain at least one non-empty message (400)
  context?: InputContext;
  timestamp?: string; // RFC 3339; canonicalized to UTC milliseconds; unparseable or sub-millisecond is 400; missing uses receipt time
}

export interface RecallInput {
  query: string; // must not be empty/blank (400)
  context?: InputContext;
  budget?: RecallBudget | null;
}

export interface RecallBudget {
  // Names the o200k encoding of the tiktoken-rs 0.12 line. The earlier
  // 'o200k_base@tiktoken-rs-0.12.0' is the same encoding and is accepted.
  tokenizer?: 'o200k_base@tiktoken-rs-0.12';
  max_tokens?: number; // 1–65536; default 4096 after explicit opt-in
  context_tokens?: number; // 1–131072; default 49152; cumulative normalized planner inputs
}

export interface MemoryPolicy {
  version?: number;
  memory_strength_decay_factor?: number; // deprecated; retained for stored-policy compatibility; inert
  recall_reinforcement?: number; // retained for stored-policy compatibility; inert
  correction_penalty?: number; // retained for stored-policy compatibility; inert
  decay_floor?: number; // deprecated; retained for stored-policy compatibility; inert
  stale_event_threshold_days?: number;
  unconsolidated_max_backlog?: number;
  orphan_max_count?: number;
  self_test_queries_per_cycle?: number;
  self_test_token_budget?: number;
  recall_search_threshold?: number; // declared but not consumed yet
  recall_max_rounds?: number;
  recall_budget?: RecallBudget | null;
  shadow_replay_sample?: number;
}

export interface MemoryCitation {
  entity: string;
  type?: string;
  name?: string;
  confidence?: number;
  source?: string;
  created_at?: string;
}

export interface RecallBudgetReceipt {
  tokenizer: string;
  token_limit: number;
  tokens: number;
  context_token_limit: number;
}

export interface RecallOutput {
  answer: string;
  found: boolean;
  uncertainty?: number;
  memories?: MemoryCitation[];
  conversation?: number;
  usage: Usage;
  failed_reason?: string;
  memory_budget?: RecallBudgetReceipt;
}

export interface ProbeInput {
  query: string;
  limit?: number;
}

export interface ProbeOutput {
  found: boolean;
  negative_cached: boolean;
  search_exhaustive?: boolean;
  hits?: MemoryCitation[];
}

export interface MemoryPinInput {
  entity: string; // graph element id such as C-7, P-3, or A-2
  pinned?: boolean; // default true
}

export interface MemoryPinOutput {
  entity: string;
  pinned: boolean;
  updated: number; // number of changed retention records
}

export interface MemoryForgetInput {
  entities: string[];
  dry_run?: boolean; // default false; inspect a dry run before deletion
}

export interface MemoryForgetEntity {
  entity: string;
  existed: boolean;
  error?: string;
}

export interface MemoryForgetReport {
  dry_run: boolean;
  deleted_concepts: number;
  deleted_propositions: number;
  deleted_assertions: number;
  deleted_evidence: number;
  deleted_activities: number;
  entities?: MemoryForgetEntity[];
}

export interface MemoryMetrics {
  recalls_completed: number;
  entities_recalled: number;
  probe_hits: number;
  probe_misses: number;
  negative_cache_hits: number;
  self_test_tested: number;
  self_test_grounded: number;
  reencode_tasks: number;
  corrections: number;
  uncertainty_reports: number;
  uncertainty_sum: number;
  forgotten_entities: number;
  updated_at: number;
}

export interface MemoryGraphCounters {
  concepts: number;
  propositions: number;
  unconsolidated?: number;
  orphans?: number;
  predicate_types?: number;
  as_of?: number;
}

export interface WatchSettlement {
  fired: number;
  conflicted: number;
  disarmed: number;
  deferred: number;
  error?: string;
}

export interface SkillSettlement {
  unsupported_reason?: string;
  graded: number;
  transitions: number;
  conflicted: number;
  error?: string;
}

export interface MemorySettlementReport {
  settled_at: number;
  revised_roots?: unknown[];
  exposures?: { element_id: string; retrieved: number; used: number; last_snapshot_seq: number }[]; // next bounded exposure-log batch (Spec §66.8): the reinforcement input
  exposures_truncated?: boolean;
  new_corrections: number;
  watches: WatchSettlement;
  commitments: { due: number; raised: number; error?: string }; // due Commitments raised as commitment_review Activities
  skills: SkillSettlement;
  correction_scan_error?: string;
  correction_scan_incomplete: boolean;
  correction_scan_through_seq: number;
  retention: { // records whose retention expired; claim validity is computed at read time
    archived: number;
    held: number;
    refused: number;
    remaining: number;
    error?: string;
  };
}

export interface SelfTestReport {
  tested_at: number;
  tested: number;
  grounded: number;
  reencode_tasks: number;
  usage: Usage;
}

export interface ShadowEvalInput {
  policy: MemoryPolicy;
  replay_sample?: number; // default from policy, at most 16
}

export interface ShadowReport {
  compared_at: number;
  replayed: number;
  baseline_wins: number;
  candidate_wins: number;
  ties: number;
  judge_errors: number;
  candidate_policy: MemoryPolicy;
  usage: Usage;
  samples?: { query: string; winner: 'baseline' | 'candidate' | 'tie' | 'error'; reason?: string }[];
}

export interface MemoryStatus {
  metrics: MemoryMetrics;
  groundability?: number;
  probe_hit_rate?: number;
  correction_rate?: number;
  avg_uncertainty?: number;
  maintenance_tokens_per_recall?: number;
  graph: MemoryGraphCounters;
  last_settlement?: MemorySettlementReport;
  last_self_test?: SelfTestReport;
  last_shadow?: ShadowReport;
  last_schema_audit?: { audited_at: number; predicates?: Record<string, number> };
}

export interface MaintenanceParameters {
  stale_event_threshold_days?: number; // [1, 365]
  memory_strength_decay_factor?: number; // deprecated and ignored; still checked in (0, 1]; alias: confidence_decay_factor
  unconsolidated_max_backlog?: number; // [1, 10000]; alias: unsorted_max_backlog
  orphan_max_count?: number; // [1, 10000]
}

export interface MaintenanceInput {
  trigger?: 'scheduled' | 'threshold' | 'on_demand';
  scope?: 'full' | 'quick' | 'daydream'; // defaults to 'daydream'
  timestamp?: string; // canonical UTC: YYYY-MM-DDTHH:mm:ss.SSSZ
  parameters?: MaintenanceParameters;
}

export interface AddSpaceTokenInput {
  scope: TokenScope; // minting "*" requires a "*"-scoped CWT
  name: string; // required, unique per space
  expires_at?: number; // Unix timestamp in milliseconds
  labels?: string[]; // wiki ACL labels; omitted = unrestricted, [] = unlabeled only
}

export interface RevokeSpaceTokenInput {
  token?: string; // full token value…
  name?: string; // …or the unique token name (one of the two is required)
}

export interface UpdateSpaceInput {
  name?: string;
  description?: string;
  public?: boolean;
  wiki_digest?: boolean; // enable WikiDigest graph extraction (default false)
  wiki_audit_reads?: boolean; // event external wiki reads (default false)
  wiki_acl_defaults?: Record<string, string>; // namespace -> default ACL label
  memory_policy?: MemoryPolicy; // replaces the space policy; omitted members use server defaults
}

export interface FormationRestartInput {
  conversation: number;
}

export interface CreateOrUpdateSpaceInput {
  user: string;
  space_id: string;
  tier: number;
}

export interface GetOrInitUserInput {
  user: string;
  name?: string;
}

// ── Wiki: versioned reference documents with verifiable citations ──────────

export type WikiDocStatus = 'active' | 'archived';
export type WikiSearchMode = 'chunks' | 'docs';

export interface WikiCommitInput {
  doc_id?: number; // omit to create a new document
  parent_version?: number; // required on update (CAS); stale value -> 409
  namespace?: string; // default "default"
  slug?: string; // display slug; derived from title when omitted
  title: string;
  content: string; // full Markdown document (not a diff); <= 1 MiB normalized
  tags?: string[]; // omit to keep stored tags on update
  acl_label?: string; // omit to keep/inherit namespace default; "" clears
  source_uri?: string; // omit to keep
  message?: string; // commit message
  metadata?: Record<string, unknown>; // omit to keep
}

export interface WikiDocInfo {
  id: number;
  namespace: string;
  slug: string;
  title: string;
  status: WikiDocStatus;
  current_version: number;
  current_checksum: string; // "sha3-256:..."
  tags: string[];
  acl_label?: string;
  source_uri?: string;
  metadata?: Record<string, unknown>;
  created_by: string;
  updated_by: string;
  created_at: number;
  updated_at: number;
}

export interface WikiVersionInfo {
  id: number;
  doc_id: number;
  parent_version?: number;
  checksum: string;
  size: number;
  author: string;
  message?: string;
  created_at: number;
}

export interface WikiCommitOutput {
  doc: WikiDocInfo;
  version: WikiVersionInfo;
  chunks: number;
  created: boolean;
  idempotent: boolean; // true when nothing changed (no new version written)
}

export interface WikiSearchInput {
  query: string; // BM25 keywords: exact terms, product names, error codes
  namespaces?: string[];
  doc_ids?: number[];
  tags?: string[];
  top_k?: number; // 1-50, default 8
  mode?: WikiSearchMode; // 'docs' = one best hit per document
  expand?: number; // 0-2 neighbor expansion; citations widen accordingly
}

export interface WikiCitation {
  uri: string; // wiki://{space}/{doc_id}@{version_id}#{start}-{end}
  doc_id: number;
  version_id: number;
  chunk_id: number;
  heading_path: string[];
  anchor: string; // stable section anchor for wiki_read
  byte_range: [number, number];
  checksum: string; // verifiable via /wiki/verify
  quote: string;
}

export interface WikiHit {
  text: string;
  doc_title: string;
  heading_path: string[];
  citation: WikiCitation;
}

export interface WikiSearchOutput {
  hits: WikiHit[];
  total_docs_matched: number;
}

export type WikiSelector =
  | { type: 'toc' }
  | { type: 'section'; anchor: string }
  | { type: 'range'; start: number; end: number }
  | { type: 'full' };

export interface WikiReadInput {
  doc_id: number;
  version?: number; // time-travel read of a historical version
  selector?: WikiSelector; // default { type: 'full' }
}

export interface WikiTocEntry {
  anchor: string;
  heading_path: string[];
  byte_start: number;
  byte_end: number;
}

export interface WikiReadOutput {
  doc_id: number;
  version_id: number;
  is_current: boolean;
  title: string;
  status: WikiDocStatus;
  checksum: string;
  size: number;
  toc?: WikiTocEntry[]; // for the 'toc' selector
  content?: string; // for section/range/full selectors
  byte_range?: [number, number];
  truncated: boolean; // full reads are bounded (256 KiB)
}

export interface WikiVerifyInput {
  uri?: string; // wiki:// citation URI, or pass the explicit fields below
  doc_id?: number;
  version_id?: number;
  byte_range?: [number, number];
  checksum?: string; // compared against the recomputed checksum when present
}

export type WikiVerifyStatus = 'valid' | 'superseded' | 'invalid' | 'not_found';

export interface WikiVerifyOutput {
  status: WikiVerifyStatus; // 'superseded' = intact but a newer version exists
  current_version?: number;
  checksum?: string; // recomputed from immutable content
  quote?: string;
}

export interface WikiBundleEntry {
  path: string; // bundle-relative path, e.g. "guides/setup.md"
  content: string;
}

export interface WikiImportInput {
  entries: WikiBundleEntry[]; // OKF v0.1 bundle files (Markdown + YAML frontmatter)
  namespace?: string; // default "default"; bundles round-trip per namespace
}

export type WikiImportStatus = 'created' | 'updated' | 'unchanged';

export interface WikiImportOutput {
  created: number;
  updated: number;
  unchanged: number; // checksum-idempotent: re-imports never grow versions
  docs: { path: string; doc_id: number; version_id: number; status: WikiImportStatus }[];
  skipped?: { path: string; reason: string }[];
}

export interface WikiExportOutput {
  namespace: string;
  entries: WikiBundleEntry[]; // concept .md files + index.md + manifest.json
  docs: number;
}

export interface WikiEventInfo {
  id: number;
  // DocCreated | VersionCommitted | DocArchived | DocRestored | OrphanSwept
  // | CitationVerifyFailed | ImportCompleted | ExportCompleted
  // | DigestExtracted | DigestFailed | WikiQueried | WikiRead | StaleReport | EventsPruned
  kind: string;
  doc_id?: number;
  version_id?: number;
  actor: string;
  detail?: Record<string, unknown>;
  created_at: number;
}

export interface WikiDigestReport {
  digested: number; // document generations processed by extraction or result reuse
  facts: number; // claims retained in the processed ledgers, not an exhaustive inventory
  superseded: number; // owned Assertions retracted after review or source withdrawal
  skipped: number; // archived, labeled or evaluation documents handled without extraction
  failed: number; // failed documents remain pending; another call retries them
  citations_checked: number;
  citations_invalid: number;
  usage: Usage;
}

export interface McpServerConfig {
  space_id: string;
  auth_token?: string;
  auto_create_space?: boolean;
  auto_create_tier?: number;
}

export interface McpHttpServerConfig {
  path_prefix?: string; // default "/mcp"; clients connect to {path_prefix}/{space_id}
  allowed_hosts?: string[]; // default loopback-only in rmcp; set company domains explicitly
  allowed_origins?: string[]; // for browser-based MCP clients
  auto_create_space?: boolean;
  auto_create_tier?: number;
}

export interface Concept {
  id: string; // engine-assigned element id, e.g. "C-7"
  kind: 'concept';
  space_id?: string;
  schema_ref?: string; // the exact type symbol, e.g. "kip://profiles/cognitive-memory@2.0.0/Person"
  key?: string; // immutable Space-local logical key — the caller's handle
  name?: string; // mutable display label; never identity
  canonical_id?: string;
  aliases?: string[];
  attributes?: Record<string, unknown>;
  facets?: Record<string, Record<string, unknown>>; // e.g. MnemonicState
  retention?: { retention_class?: string; expires_at?: string; legal_hold?: boolean };
  _system?: Record<string, unknown>; // engine truth: version, created_at, state, origin
}

export interface ModelConfig {
  family: string; // "gemini", "anthropic", "openai", "deepseek", "mimo" etc.
  model: string;
  api_base: string;
  api_key: string;
  disabled?: boolean;
  label?: string;
  effort?: 'minimal' | 'low' | 'medium' | 'high' | 'max';
  bearer_auth?: boolean;
  stream?: boolean;
  context_window?: number;
  max_output?: number;
}

export interface SpaceTier {
  tier: number;
  updated_at: number; // Unix timestamp in milliseconds
}

export interface SpaceToken {
  token: string; // full value only in the add_space_token response; redacted to a prefix elsewhere
  name: string; // required, unique per space; audit identity and revocation handle
  scope: TokenScope;
  usage: number;
  created_at: number; // Unix timestamp in milliseconds
  updated_at: number; // Unix timestamp in milliseconds
  expires_at?: number; // Unix timestamp in milliseconds
  labels?: string[]; // wiki ACL labels: [] sees unlabeled content only; omitted = unrestricted
}

export interface StorageStats {
  [k: string]: number | string | boolean | null;
}

export interface SpaceInfo {
  id: string;
  name?: string;
  description?: string;
  owner: string;
  db_stats: StorageStats;
  concepts: number;
  propositions: number;
  conversations: number;
  public: boolean;
  tier: SpaceTier;
  formation_usage: Usage;
  recall_usage: Usage;
  maintenance_usage: Usage;
  formation_processed_id: number;
  maintenance_processed_id: number;
  maintenance_at: MaintenanceAt;
  memory_interface?: MemoryDescriptor; // the Memory Interface this Space serves
  wiki_docs: number;
  wiki_chunks: number;
  wiki_versions: number;
  wiki_queries: number;
  wiki_digested: number; // observed version high-water mark, not processing coverage
  wiki_stale_docs: number; // from the last housekeeping stale scan
}

export interface FormationStatus {
  id: string;
  concepts: number;
  propositions: number;
  conversations: number;
  formation_processing: boolean;
  maintenance_processing: boolean;
  formation_processed_id: number;
  maintenance_processed_id: number;
  maintenance_at: MaintenanceAt;
}

export interface MaintenanceAt {
  daydream: number;
  full: number;
  quick: number;
  /** Start time of the latest maintenance task in unix milliseconds, 0 if none started. */
  start_at: number;
}

export interface Usage {
  /** Input tokens sent to the LLM. */
  input_tokens: number;
  /** Output tokens received from the LLM. */
  output_tokens: number;
  /** Cached tokens used in the execution. */
  cached_tokens: number;
  /** Number of requests made to models, agents, or tools. */
  requests: number;
}

export interface AgentOutput {
  content: string;
  conversation?: number;
  failed_reason?: string;
  usage?: Usage;
  model?: string;
  [k: string]: unknown;
}

export type ConversationStatus =
  | 'submitted'
  | 'working'
  | 'idle'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface Conversation {
  _id: number;
  user: string;
  thread?: string;
  label?: string;
  messages: Message[];
  resources: unknown[];
  artifacts: unknown[];
  status: ConversationStatus;
  failed_reason?: string | null;
  period: number;
  created_at: number;
  updated_at: number;
  usage: Usage;
  steering_messages?: string[];
  follow_up_messages?: string[];
  child?: number;
  extra?: unknown;
  ancestors?: number[];
}

export interface ConversationDelta {
  _id: number;
  messages: unknown[];
  artifacts: unknown[];
  status: ConversationStatus;
  usage: Usage;
  failed_reason?: string | null;
  updated_at: number;
  child?: number | null;
}

export interface ServiceInfo {
  name: string;
  version: string;
  sharding: number;
  description: string;
  memory_interface: MemoryDescriptor; // without default_space
}

export interface MemoryDescriptor {
  kip_memory: '2.0';
  bundles: ('memory_basic' | 'memory_experience' | 'memory_learning')[];
  default_scope?: { task_ref?: string; context_refs?: string[] };
  default_budget: { max_output_tokens: number; deadline_ms: number };
  tokenizer: string;
  minimum_response_tokens: number;
  default_space?: { id: string };
}

export type KipOperation = string | {
  op_id?: string;
  language?: 'KQL' | 'KML' | 'META'; // advisory; parsed command controls the read-only gate
  command?: string;
  ast?: unknown;
  parameters?: Record<string, unknown>;
  idempotency_key?: string;
  options?: { extensions?: Record<string, unknown> };
  extensions?: Record<string, unknown>;
};

export interface KipRequest {
  command?: string; // a single command; mutually exclusive with `operations`
  operations?: KipOperation[]; // several commands in one round-trip
  execution?: { mode: 'independent' | 'sequence' | 'atomic'; on_error?: 'stop' | 'continue'; isolation?: string; idempotency_key?: string; extensions?: Record<string, unknown> }; // required for more than one operation
  read?: { snapshot_token?: string; extensions?: Record<string, unknown> }; // bind every operation to one read coordinate
  parameters?: Record<string, unknown>; // values bound into `:placeholders`
  dry_run?: boolean; // validate and plan without committing
}

export interface KipError {
  code: string; // a registry name, e.g. "NotFoundOrNotVisible" — not a number
  message: string;
  category?: string;
  hint?: string;
  retry?: { class: string; after_ms?: number };
  details?: unknown;
}

export interface KipOperationResult<T> {
  op_id?: string;
  status: 'succeeded' | 'failed' | 'skipped' | 'rolled_back' | 'no_effect';
  result?: T;
  context?: unknown;
  error?: KipError;
  warnings?: unknown[];
  next_cursor?: string;
  receipt?: unknown;
  extensions?: Record<string, unknown>;
}

export interface KipResponse<T> {
  kip: '2.0';
  request_id?: string;
  status: 'succeeded' | 'failed' | 'partial' | 'outcome_unknown';
  results: KipOperationResult<T>[];
  execution?: unknown;
  context?: unknown;
  snapshot?: { space_id?: string; snapshot_seq: number; schema_environment_version?: number; snapshot_token?: string; extensions?: Record<string, unknown> };
  receipt?: unknown;
  warnings?: unknown[];
  next_cursor?: string;
  error?: KipError; // set only when the request failed before its operations
  extensions?: Record<string, unknown>;
}
```

> **KIP 2.0.** A failure lives at the operation level for an ordinary error and
> at the request level only for an envelope error, so a client must read both.
> `outcome_unknown` is neither success nor failure: a write may have committed,
> and the recovery is to look the transaction up — never to re-send it as if
> nothing had happened.

---

## 8) Frontend Call Example (TS)

```ts
async function rpcPost<TReq, TRes>(
  url: string,
  body: TReq,
  token?: string
): Promise<RpcResponse<TRes>> {
  const res = await fetch(url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json',
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    body: JSON.stringify(body),
  });

  const responseText = await res.text();
  if (!res.ok) {
    let message = responseText || `HTTP ${res.status}`;
    try {
      const error = JSON.parse(responseText) as RpcError;
      if (error.message) message = error.message;
    } catch { /* middleware errors can be plain text */ }
    throw new Error(`HTTP ${res.status}: ${message}`);
  }
  return JSON.parse(responseText) as RpcResponse<TRes>;
}

// Recall
const recall = await rpcPost<RecallInput, AgentOutput>(
  '/v1/my_space_001/recall',
  { query: 'What are this user\'s preferences?', context: { counterparty: 'user_1' } },
  'YOUR_TOKEN'
);

if (recall.error) {
  console.error(recall.error.message);
} else {
  console.log(recall.result?.content);
}
```

---

## 9) Rust Host APIs

These are trusted Rust interfaces for an embedding host. They add no HTTP or MCP routes and grant no model authority.

<a id="trusted-host-memory-product-contracts"></a>

### 9.1 Trusted host memory product contracts

The Rust `product` module provides Assertion-backed `MemoryRecord` projections, stable native revisions, explicit stance/lifecycle/storage state and typed Evidence source references. `Space::product_records`, `product_record` and `product_source` do not authenticate a user. The embedding host must enforce owner/source visibility before returning any record, preview, dependent identifier or source quote. A matching source digest is provenance, not proof that an inference is correct.

`Space::ingest_product` accepts a bounded, trusted `SourceIdentity` with parent conversation/session keys. Natural-language input cannot set that identity. `product_prepare`, `product_commit`, `product_change` and `product_discard` implement caller/operation-scoped immutable requests, fixed revision/preview digests and ten-minute previews. `ChangeKind` names which history a change writes (KIP Spec §14.2, Memory Interface §4):

- `Correct` — the caller's own claim was wrong. A new claim with a user-statement Evidence supersedes the old Assertion and keeps the world interval it covered (an absent start is materialized as `{latest: <original asserted_at>}`); a `belief_revision` Activity records it. The old Assertion reads `superseded`.
- `WorldChange` — the world moved on. One new claim from now; temporal succession ends the old value, which stays `active` and true for its time.

Both revisions keep the record's `context_refs` (exposed on `MemoryRecord`): supersession across context sets fails `SupersessionMismatch` (Spec §14.2), and a new value outside the old context set would start a second succession line instead of ending the old value (Spec §25.4).
- `Misrecorded` — the Brain recorded what the caller never said. That is recording repair, which runs through the Memory Interface `revise` intent with `change_kind: "misrecorded"` (see [Memory Interface](#memory-interface)); this value-based product API fails `unsupported_capability` and never writes it as a correction or a world change.

None of them rewrites a Concept label or creates an illegal cross-Proposition supersession. Undo is another conditional change.

`Suppress` archives and `Delete` purges the declared bounded closure: selected Proposition/Assertions, cited inputs and recorded referrers. Unknown sources, Concept cascades, retention holds and closures above 128 elements are rejected. A durable source exclusion and processing epoch are admitted before mutation; tracked native work survives a cancelled API waiter and resumes before a reloaded Space is exposed. Stale Formation/Maintenance/Notes writes are fenced. Managed changes clear processing Notes and miss caches, stop old processor histories from entering new contexts and restrict automatic KIP readers to current active state. Trusted owner audit APIs remain distinct. Recall rechecks its captured epoch before returning context.

Managed changes also exclude history from budgeted Recall and fence timeout/turn-limit outputs. Pagination started before the change must restart; newer continuations remain usable. Deletion clears copies of erased content from saved previews/corrections and discards affected uncommitted intents while retaining operation identities and digests. An independent correction whose claim and Evidence survive keeps its source text. Correction-source reads check that the Evidence still exists with the matching payload.

Removal does not erase an embedding application's original chat/files/logs/backups, other independent graph records, already delivered context or provider copies. Minimal source keys/digests survive for replay suppression. Replacing the database with an old backup without its current exclusions is not a supported deletion-preserving rollback. The host must also reset its own injected Notes and prevent excluded sources from being imported through later conversation chains.

`MemoryRuntime::{create_record_watch,record_watch,cancel_record_watch}` is a narrow recipient-owned subscription adapter using an opaque authenticated `RuntimeCaller`. Creation persists identity and arms only the initial generation. It provisions a cancellation grant restricted to that one Watch for the configured controller, with a durable no-regrant marker. Cancellation archives the Watch rather than forging its protected status; retries never re-arm it. Already delivered questions are separate work and remain visible. A revoked grant is not restored by subscription retry or bootstrap. These Rust methods are not new generic model tools or native HTTP product routes.

`RuntimeConfig::validate` performs static validation without loading a Space, running models, probing services or provisioning grants. The optional learning runtime reports `product_readiness` for installed isolated workflow bindings, including missing services, mismatched pins, missing reviewed calibration and approval gates. Ready is not business deployment authority; native per-item service/permission checks remain mandatory.

Anda Bot consumes these contracts from the published crate and keeps one shared DB/KIP/Core type identity. No empirical learning improvement or full cost accounting is implied by these mechanism tests.

### 9.2 Trusted learning runtime (`learning` feature)

With `learning`, `Space::learning()` exposes explicit registration, frozen
cohorts, bounded `drive` steps, metadata discovery after restart, and separately
authenticated Outcome ingestion. Native leases, current executable authority,
dependency validity and policy pins gate dispatch. These are host APIs and add
no HTTP/MCP routes or model mutation authority. Cohort completion does not adopt
a Skill. `settle(job_id)` recomputes the native ledger after the fixed cutoff
and atomically records the verdict/standing. `reviews()` and `enroll_review()`
provide recoverable monitoring with retained acquisition evidence;
`submit_safety_signal()` accepts separately authenticated safety withdrawal.
`bind_application_context()` accepts short-lived trusted host observations;
`procedure_status()` and Recall's internal `check_procedure_status` tool read
current eligibility without granting execution permission. Unverified conditions
or an expired review block recommendations. Configured learning Spaces cannot
copy their operational journals via fork/snapshot. See [runtime](README.md#native-learning-contracts)
and [lifecycle and recovery](README.md#native-learning-contracts).

### 9.3 Isolated experiments and the MIB host (`experiments` feature)

The sibling Anda Bot `mib` feature exposes separate loopback protocols at
`/mib-agent/v0.1` and `/mib-memory/v0.1`. These are not routes of this production
Brain API. They use `experiments` for isolated state, completion barriers,
monotonic business time and cleanup. See [integration](README.md#mib-integration).
The adapter does not advertise online learning; costs with missing provider or
observer telemetry remain incomplete.

The Rust `Experiment::create_with_recall_budget` factory persists a forced Recall
budget before exposing a run. `audit_procedures()` returns a bounded read-only
native inventory; truncated counts cannot prove absence. The Bot-only MIB
extension `learning_audit` exposes this inventory to the evaluator, never the
business model. It neither enables learning nor grants execution authority.
See [validation](README.md#mib-integration).

### 9.4 Instance configuration and offline regression

The former Rust `anda_brain::eval` API and `eval` CLI (including optimizer/miner
flags) have been retired. MIB supplies the public product-regression
profile; self-test, shadow diagnostics, probes, citations and ledgers remain
online instruments. Runtime policies use the Space's persisted `MemoryPolicy`.
Trusted Rust hosts can call `AppState::with_agent_prompts(AgentPrompts)` before
sharing/opening the host to supply immutable deployment sections; each section
starts with `# A.`, fits 128 KiB and retains the compiled reference prefix.
This configuration is not an HTTP/MCP prompt operation. See [migration](README.md#offline-regression-and-instance-configuration).

---

## 10) Execution and Resource Limits

Formation and Maintenance share one per-Space writer admission guard, including
Maintenance's deterministic settlement. Explicit Rust/HTTP/MCP Maintenance is
refused while Formation owns that guard; queued Formation resumes after Maintenance.
Formation's threshold trigger transfers the guard before starting the next worker.

`LLM_MAX_CONCURRENCY` bounds in-flight calls through Brain's model registry across
Spaces, including background Formation/Maintenance, compaction, WikiDigest,
self-test and shadow judges. HTTP/MCP request admission has a separate semaphore
with the same configured capacity: excess requests still receive HTTP 429 (or the
MCP busy error), while admitted model calls wait cancellably for a model slot.
Explicit semantic-Watch and learning-provider bindings keep their separate configured
budgets. This is a concurrency bound, not a total token or monetary budget.

Space shutdown cancels model waits, stops background admission and drains admitted
native settlement writes. A failed eviction retains its closing owner for retry;
access to that Space can temporarily fail until closing completes. Other Spaces
remain accessible while its database closes.

Self-test records an independent `last_self_test_at` and can retest unused memories
after 30 days. `self_test_token_budget` counts the actual host instructions/prompt
with the pinned Recall tokenizer and reserves one quarter (at most 4096 tokens)
for the requested output cap. Candidates that do not fit are not sent. Provider
framing/tokenization may differ; unsupported output caps fail the diagnostic,
and missing provider usage remains unknown. This is not a provider billing guarantee.
