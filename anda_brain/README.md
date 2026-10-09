# Anda Brain — Technical Reference

`anda_brain` is the Rust library and service binary behind
[Anda Brain](../README.md): a dedicated, LLM-driven memory service that maintains a
persistent **Cognitive Nexus** on behalf of business agents through
[KIP 2.0](https://github.com/ldclabs/KIP). Business agents talk to it in natural
language over REST or MCP; they never write KIP.

| Document | Use it for |
| :--- | :--- |
| [API.md](API.md) · [API_cn.md](API_cn.md) | Every request/response shape, with TypeScript types |
| [SKILL.md](SKILL.md) | Integration instructions for agents (served at `GET /SKILL.md`) |
| [RUNTIME.md](RUNTIME.md) · [RUNTIME_cn.md](RUNTIME_cn.md) | Runtime API, Watch scheduling, action callbacks, recovery, execution limits |
| [SEMANTIC_WATCH_RUNTIME.md](SEMANTIC_WATCH_RUNTIME.md), [LEARNING_RUNTIME.md](LEARNING_RUNTIME.md), [UTILITY_RUNTIME.md](UTILITY_RUNTIME.md), [TRUST_RUNTIME.md](TRUST_RUNTIME.md) | Optional runtimes (each with a `_cn.md` edition) |
| [../CHANGELOG.md](../CHANGELOG.md) | Release notes and known limits |

Version 0.13.4 tracks KIP `11a82ec` and `kip://profiles/cognitive-memory@2.0.0`
(content digest `sha256:734aa0fd…`), on the 0.14 AndaDB / KIP / Cognitive Nexus
stack and `anda_engine` 0.16. `Cargo.lock` records the resolved versions.

## Contents

- [Architecture](#architecture)
- [Agents](#agents)
- [Memory model](#memory-model)
- [Memory Interface](#memory-interface)
- [HTTP API](#http-api)
- [MCP server](#mcp-server)
- [Diagnostics and observability](#diagnostics-and-observability)
- [Wiki](#wiki-wiki-feature)
- [Optional runtimes and host contracts](#optional-runtimes-and-host-contracts)
- [Space lifecycle](#space-lifecycle)
- [Configuration](#configuration)
- [Cargo features](#cargo-features)
- [Running](#running)
- [Embedding the library](#embedding-the-library)
- [Prompts and the KIP reference](#prompts-and-the-kip-reference)
- [Testing](#testing)
- [Known limits](#known-limits)

---

## Architecture

```
┌─────────────────────┐
│   Business agent    │  natural language, REST, MCP, Memory Interface
└─────────┬───────────┘
          ▼
┌─────────────────────────────────────────────────────────────┐
│ anda_brain                                                  │
│  HTTP / MCP handlers ─ auth (CWT, Space tokens) ─ payloads  │
│  Formation · Recall · Maintenance agents (LLM + KIP tools)  │
│  Host gates · settlement · self-test · attention scheduler  │
└─────────┬───────────────────────────────────────────────────┘
          ▼ KIP 2.0 (KQL / KML / META)
┌─────────────────────────────────────────────────────────────┐
│ Cognitive Nexus on AndaDB — one database per Space          │
│ object store: local filesystem · AWS S3 · in-memory         │
└─────────────────────────────────────────────────────────────┘
```

| Module | Responsibility |
| :--- | :--- |
| `src/agents/` | Formation, Recall and Maintenance agents and their prompts |
| `src/space.rs`, `src/space/` | Space loading, lifecycle, flushing/eviction, settlement hooks, self-test, shadow evaluation, attention, vocabulary |
| `src/handler.rs`, `src/handler/` | HTTP route handlers |
| `src/mcp.rs` | MCP server (stdio and Streamable HTTP) |
| `src/payload.rs` | JSON / CBOR / Markdown negotiation |
| `src/authz.rs` | Endpoint admission (public read, credentialed, management) |
| `src/types.rs` | API inputs/outputs and persisted configuration |
| `src/kip.rs` | KIP envelope builders, read-only gate, literal helpers |
| `src/kip_reference.rs` | The read-only `kip_reference` tool over the compiled reference |
| `src/memory_interface/` | KIP Memory Interface (`memory_basic`) |
| `src/vocabulary.rs` | Draft vocabulary and the Formation `DEFINE` gate |
| `src/settlement/` | Deterministic settlement: corrections, revised roots, Watch advancement, Commitments, retention |
| `src/assess/` | Recall traces, citations, probe |
| `src/recall_budget/` | Budgeted Recall packets and the pinned tokenizer |
| `src/attention/`, `src/action/`, `src/runtime_api/` | Watch scheduling, action gate and dispatch, authenticated runtime API |
| `src/consequence/`, `src/recall_receipt.rs` | Outcomes, Recall receipts, utility and trust |
| `src/product.rs`, `src/product/` | Trusted-host memory product contracts |
| `src/learning/` | Trusted paired-trial runtime (`learning` feature) |
| `src/wiki/` | Versioned wiki (`wiki` feature) |
| `src/legacy_upgrade.rs` | In-place upgrade of KIP 1.x Spaces |

---

## Agents

Every model call carries the complete, version-pinned KIP 2.0 syntax, the Cognitive
Memory Profile, the applicable role cards and the deployment contract in its system
prompt (see [Prompts and the KIP reference](#prompts-and-the-kip-reference)). The
model writes KIP; the host decides what it is allowed to write.

### Formation — encoding (`formation_memory`)

`POST /v1/{space_id}/formation` and the Memory Interface `observe` / `revise` /
`feedback` intents queue a Formation conversation and return at once.

1. The input (messages, optional `context`, optional `timestamp`) becomes a tracked
   conversation: `submitted` → `working` → `completed` | `failed`. The host captures
   each message as Evidence before the model runs; the model cites it as `:msg1`,
   `:msg2`, ….
2. The model decides what is worth keeping, using the Profile's products: Evidence
   for what was observed, Proposition + Assertion for a claim and whose stance it is,
   Event for what happened, Experience for a process that can teach future behavior,
   Commitment for an obligation, Insight and SelfModel candidates. Writing nothing is
   a valid answer.
3. It grounds against existing memory (`SEARCH` before `CREATE`) and writes through
   `execute_kip`.

The Formation gate accepts KQL and META in full and only the cognition subset of KML.
Bulk administration — `UPDATE`, `SET RETENTION`, `PURGE`, `MERGE CONCEPT`, and
`TRANSITION` to `archived` or `tombstoned` — is refused, because the whole input is an
untrusted conversation. Other host rules:

- **World time.** An Assertion that cites a captured message must carry that message's
  observation time (`at` or `valid.from`); the Brain's own inferences are exempt. A
  message's `timestamp` (Unix ms) is its observation time; otherwise the request
  `timestamp` (RFC 3339, canonicalized to UTC milliseconds; unparseable values are
  rejected with 400), otherwise the conversation's creation time.
- **Draft vocabulary.** A command naming an undefined type or predicate fails with
  `SchemaSymbolNotFound`. Formation then sends `DEFINE CONCEPT TYPE` /
  `DEFINE PREDICATE` with literal names in a request of its own (at most eight).
  The host checks the name's shape, caps the Space at 512 symbols of its own, and
  queues one `review_schema` SleepTask per new symbol. The `declare_memory_symbols`
  tool remains as a deprecated shortcut that drafts bare names the same way.
- **Strength writes** must carry `last_metabolized_at` and the host-bound
  `strength_policy` pin (`kip:strength-half-life-30d`).
- **Scope.** A Memory Interface source with a task/context scope makes every claim
  carry that context set; the gate refuses one that does not.
- **Large inputs** (≥ 10,000 estimated tokens) get one focused self-review within the
  same turn and time budgets. It checks for material omissions and misrepresentation;
  "no changes" is a valid result. This is not a completeness guarantee.

Formation processes a Space's queue sequentially and resumes after Maintenance. A
conversation that fails three rounds over at least 30 minutes is given up so the
queue can move on. Tools: `execute_kip`, `note`, `declare_memory_symbols`,
`memory_runtime`, `kip_reference`.

### Recall — retrieval (`recall_memory`)

`POST /v1/{space_id}/recall`, `/recall_structured`, the MCP recall tool and the
Memory Interface `recall` intent run the Recall agent.

1. It loads a fresh Primer, analyzes the question (entity lookup, relationship
   traversal, event recall, pattern, …) and grounds names to graph nodes.
2. It reads with KQL and META. Belief questions go through `BELIEF` projection rather
   than raw `FIND`, because a Proposition existing is not the Proposition being true.
3. It deepens iteratively, up to the Space's `recall_max_rounds` (default 7).
4. It answers with what memory supports: contested as contested, insufficient as
   insufficient — never "no" for "no basis".

Recall is strictly read-only. `execute_kip_readonly` enforces KQL/META on what each
command *parses to*, and reading never reinforces memory. Tools:
`execute_kip_readonly`, `kip_reference`, plus `wiki_search` / `wiki_read` with the
`wiki` feature and `check_procedure_status` with the `learning` feature.

`recall_structured` adds trace-derived citations, a `found` flag and the model's
self-reported uncertainty. An optional `budget` (or an enforced
`memory_policy.recall_budget`) switches Recall to a host-packed JSON memory packet
counted with the pinned `o200k_base@tiktoken-rs-0.12` tokenizer: required
constraints and warnings first, explicit coverage, fixed failure codes. See the
[Recall budget contract](API.md#recall-budget-contract).

### Maintenance — metabolism (`maintenance_memory`)

Maintenance runs asynchronously, single-flight per Space, and returns a conversation
id immediately. It runs at three scopes:

| Scope | Automatic trigger | Work |
| :--- | :--- | :--- |
| `daydream` (default) | every 21 formed conversations | Salience scoring and micro-consolidation |
| `quick` | every 42 formed conversations | Assessment plus urgent SleepTasks |
| `full` | every 168 formed conversations, and when 24 hours have passed since the last cycle of a Space that has formed anything | All phases, plus the predicate census and retention expiry |

The clock trigger fires from the background flush pass for resident Spaces and on load
for evicted ones. `POST /v1/{space_id}/maintenance` runs one on demand.

**Settlement first.** Before the model runs, the host settles deterministically:
newly superseded Assertions are recorded as corrections and tallied per asserting
actor (`source_reliability`); each revised root is handed over with its bounded
`LIST DEPENDENTS` (`assessment.revised_roots`); structured Watches advance under
generation/version/coverage checks; each due `pending`/`blocked` Commitment without a
Watch is raised once per `due_at` with a `commitment_review` Activity; full cycles
refresh the predicate census and archive what passed `retention.expires_at`. The
result reaches the model as its `assessment` block, which the host overwrites —
a request body cannot tell the Brain what its own graph looks like. The last report is
kept in the `memory_settlement` extension.

**Model phases** (full scope): assessment; SleepTask processing (`consolidate`,
`review_conflict`, `review_skill`, `resolve_identity`, `review_retention`,
`review_derived`, `refresh_self_model`, `inspect_quarantine`, `review_schema`);
semantic consolidation with Activity lineage; procedural consolidation into unproven
Skill candidates with immutable `SkillRevision`s; identity review and non-destructive
`MERGE CONCEPT`; contradiction and derivation review; mnemonic metabolism from
explicit signals; Commitment and Watch review; SelfModel / WorkingState refresh;
retention review with `SET RETENTION`.

What Maintenance cannot do: `PURGE` / `PURGE PAYLOAD`, `DEFINE`, promoting vocabulary,
writing WatchState / LeaseState, or writing trial, evaluation, attempt, outcome,
grading or lineage records. SleepTask work requires a five-minute lease from the
`memory_runtime` tool (`lease_task`) and commits terminal state and outputs in one
version-guarded `MUTATE`. A Watch is created `disarmed` and armed with
`memory_runtime`'s `arm_watch`, which captures an authorization view and starts a new
generation; a fired Watch is attention, never permission. Tools: `execute_kip`,
`note`, `memory_runtime`, `kip_reference`.

**Self-test.** After a cycle, the host samples recent memories with no usage
evidence, generates one probe query per memory (one model call, bounded by
`self_test_queries_per_cycle` and `self_test_token_budget`) and checks whether search
surfaces them. Unfindable ones become `review` SleepTasks that the next full cycle
re-encodes with aliases and richer descriptions. Self-test retrievals are counted
separately and never reinforce. The report lives in the `memory_self_test` extension.

---

## Memory model

| Element | Id | What it is |
| :--- | :--- | :--- |
| **Concept** | `C-*` | A referable entity: `schema_ref`, immutable `key`, mutable `name`, attributes |
| **Proposition** | `P-*` | A truth-neutral `(subject, predicate, object)` tuple |
| **Assertion** | `A-*` | One actor's stance on a Proposition: `asserted_by`, `mode`, `confidence`, valid time, Evidence |
| **Evidence** | `E-*` | An observed artifact: a message, a tool result, a document passage |
| **Activity** | `X-*` | Provenance of a process: formation, consolidation, revision, import |

- **Belief is projected**, never stored: `BELIEF` evaluates Assertions under a named
  policy. `insufficient` is never reported as "no".
- **Three histories.** A changed world is one new Assertion from the change; temporal
  succession ends the old value, which stays true for its time. A wrong claim is
  superseded by a new Assertion from the same actor. A misrecording is repaired by
  invalidating the extraction (Memory Interface `revise` with
  `change_kind: "misrecorded"`). Nothing rewrites an Assertion, and different actors'
  disagreement coexists.
- **Mnemonic state is not confidence.** `MnemonicState.memory_strength` is a base;
  the engine derives `effective_strength` at read time from the base, its anchor
  (`last_metabolized_at`) and the pinned strength policy. A missing input is unknown,
  never 0.5. There is no decay sweep, and Assertion confidence never decays.
- **Reading never reinforces.** Recall records what it surfaced in an off-graph usage
  ledger and the Nexus exposure log; neither raises strength. Maintenance may
  reinforce from explicit signals in bounded, version-guarded batches.
- **Schema is not graph state.** Types and predicates resolve from immutable,
  versioned Schema Packages. Each Space activates the Cognitive Memory Profile and
  its own draft package `kip://local/draft@0.0.0` (KIP §20.16). Drafts are additive
  and never change; `GET /v1/{space_id}/schema/drafts` lists them, and the owner
  promotes one onto an installed symbol with `POST /v1/{space_id}/schema/promote`.
  Spaces that grew vocabulary earlier keep their read-only `kip://anda-brain/memory`
  package. The Profile has no `Preference` type: an option is a Concept typed by its
  kind (`ColorScheme`, `Editor`).
- **Two expiry clocks.** An Assertion whose `valid_time.until` has passed is
  `expired` for later reads — computed, never swept, still visible to `FOR TIME` in
  the past. An element whose `retention.expires_at` has passed is archived by a full
  settlement (out of ordinary recall, still readable). Legal holds stop the sweep;
  the report counts held and refused elements.
- **Pinning and forgetting.** `POST /v1/{space_id}/memory/pin` gives an element the
  `pinned` retention class. `POST /v1/{space_id}/memory/forget` physically purges
  explicit `C-*`, `P-*`, `A-*`, `E-*` and `X-*` ids (dry run first), subject to legal
  holds and reference checks, and scrubs saved previews that referred to them.
  Conversations, wiki documents and external copies have their own lifecycles.
  Governed, plan-based erasure is the Memory Interface `forget` intent.

---

## Memory Interface

Every Space serves the KIP 2.0 Memory Interface at the `memory_basic` level: one
request shape, five intents, one intent per request. It is the recommended path for a
business agent that needs receipts and barriers.

| Intent | Does |
| :--- | :--- |
| `observe` | Runs Formation over a staged source within the request's scope |
| `recall` | Returns a `Briefing`: final belief per item, seven coverage channels (`constraints`, `commitments`, `failures`, `experiences`, `skills`, `dependencies`, `evidence`), `action_eligible`, and a retained `basis_ref` that can be expanded later. `mode: "attention"` pages raised attention without a model call |
| `revise` | Writes the right history: `correction`, `world_change`, `misrecorded` (recording repair) or `unspecified` |
| `feedback` | Captures a self-report or a person's statement as Evidence; it grades nothing |
| `forget` | Runs an ErasurePlan: `payload_only`, or `semantic` (owner CWT) erasure of a claim and its closure, including the host's own copies |

Mutations take an `idempotency_key` scoped to `(caller, Space, operation)` and return
an immutable receipt whose progress moves `recorded` → `available` (with a
disposition of `formed`, `evidence_only`, `skipped` or `erased`) or `failed`. A
recall's `after` list waits for the caller's receipts until `budget.deadline_ms`.
Staged sources, keys, receipts, retained bases and plans belong to the caller and
survive restart. `memory_experience`, `memory_learning` and `durable_brain_runtime`
are not advertised; requiring them fails with `UnsupportedCapability`.

The descriptor appears on `GET /info` and `GET /v1/{space_id}/info` as
`memory_interface`. Full semantics: [API.md › Memory Interface](API.md#memory-interface);
host-side receipts and barriers: [RUNTIME.md](RUNTIME.md#memory-interface-receipts-and-barriers).

---

## HTTP API

### Conventions

- **Envelope.** Most endpoints return `{"result": …, "error": null}`.
  `POST /v1/{space_id}/memory` returns a Memory Interface `Response` instead and
  reports failures inside it with HTTP 200.
- **Content negotiation.** Request bodies: `Content-Type: application/json`
  (default), `application/cbor` or `text/markdown` (raw text for Formation/Recall).
  Success bodies follow `Accept` with the same three types. Handler errors are always
  JSON; load shedding (`429` / `503`) and unmatched routes may return plain text.
- **Sharding.** Send `Shard-Id: <n>` (or `X-Shard`) matching the instance's
  `SHARDING_IDX`; the default is `0`.
- **Admission.** Model-driving routes (formation, recall, recall_structured,
  maintenance, shadow_eval, wiki digest) share `LLM_MAX_CONCURRENCY` and answer `429`
  when it is exhausted; every route shares `HTTP_MAX_CONCURRENCY` and answers `503`.

### Authentication

Credentials are sent as `Authorization: Bearer <token>`:

- **CWT** — a COSE Sign1 token signed by one of the `ED25519_PUBKEYS`, with claims
  `sub` (principal), `aud` (Space id or `*`) and `scope` (`read`, `write` or `*`).
  `anda-cli cwt` creates one.
- **Space token** — minted by the Space's management endpoints, with scope `read`,
  `write` or `*`, an optional expiry, and optional wiki ACL `labels`. A
  label-restricted token cannot use agentic Recall or read conversations, because
  those span all labels.

Scopes don't nest: `*` satisfies every requirement, while `read` and `write` satisfy
only endpoints that require exactly that scope. An agent that writes and recalls a
private Space needs a `*` credential or one of each.

Endpoints admit callers in one of four ways: public read (a public Space is readable
anonymously), credentialed (`read` / `write` / `*` via CWT or Space token), management
(CWT only), and admin (a CWT whose subject is in `MANAGERS`). When `ED25519_PUBKEYS` is
empty, authentication is disabled for these endpoints — never run like that in
production. The runtime endpoints always require verified credentials and explicit
mappings. The per-endpoint rules are in [API.md › Authentication](API.md#2-authentication).

### Endpoints

Public:

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/info` | Service name, version, sharding and the Memory Interface descriptor |
| `GET` | `/SKILL.md` | Integration skill (Markdown) |
| `GET` | `/favicon.ico`, `/apple-touch-icon.webp` | Icons |

Memory (`/v1/{space_id}`):

| Method | Path | Auth | Description |
| :--- | :--- | :--- | :--- |
| `POST` | `/formation` | `write` | Queue messages for encoding; returns the conversation id |
| `POST` | `/recall` | `read`* | Natural-language answer |
| `POST` | `/recall_structured` | `read`* | Answer with citations, `found`, uncertainty (or a budget packet) |
| `POST` | `/maintenance` | `write` | Run a maintenance cycle (`trigger`, `scope`, `parameters`) |
| `POST` | `/probe` | `read`* | Model-free "do I know anything about this?" |
| `POST` | `/memory/pin` | `write` | Pin or unpin an element |
| `POST` | `/memory/forget` | `write` | Purge explicit elements (`dry_run` supported) |
| `GET` | `/memory/attention` | `read`* | Fired Watches and due Commitments after a kept cursor |
| `GET` | `/schema/drafts` | `read`* | The Space's draft vocabulary |
| `POST` | `/schema/promote` | management CWT | Promote a draft onto an installed symbol |
| `GET` | `/memory_status` | `read`* | Counters, rates, graph counts, latest settlement/self-test/shadow reports |
| `POST` | `/execute_kip_readonly` | `read`* | Read-only KIP (KQL and META) |
| `POST` | `/get_or_init_user` | `write` | Get or create a counterparty Concept |
| `GET` | `/info`, `/status` | `read`* | Space information and statistics |
| `GET` | `/formation_status` | `read`* | Lightweight formation/maintenance progress |
| `GET` | `/conversations` | `read`* | List formation / recall / maintenance conversations (cursor paging) |
| `GET` | `/conversations/{id}` | `read`* | One conversation |
| `GET` | `/conversations/{id}/delta` | `read`* | Messages and artifacts after given offsets |

\* Public Spaces allow anonymous reads; some endpoints additionally reject
label-restricted tokens or anonymous reads of recall transcripts. See [API.md](API.md).

Memory Interface (`/v1/{space_id}`):

| Method | Path | Auth | Description |
| :--- | :--- | :--- | :--- |
| `POST` | `/memory` | recall `read`*; mutations `write`; `semantic` forget owner CWT | One intent per request |
| `POST` | `/memory/sources` | `write` | Stage observed messages; returns a `source_ref` |
| `GET` | `/memory/sources/{source_ref}` | `read` credential | A staged source, for the caller that staged it |
| `GET` | `/memory/receipts/{receipt_ref}` | `read` credential | Receipt progress and result |
| `GET` | `/memory/plans/{plan_ref}` | `read` credential | A forget's ErasurePlan and host surfaces |

Wiki (`/v1/{space_id}/wiki`, `wiki` feature):

| Method | Path | Auth | Description |
| :--- | :--- | :--- | :--- |
| `POST` / `GET` | `/docs` | `write` / `read`* | Commit a document version / list documents |
| `GET` | `/docs/{doc_id}`, `/docs/{doc_id}/content`, `/docs/{doc_id}/versions` | `read`* | Metadata, content (TOC, section, range), history |
| `POST` | `/docs/{doc_id}/archive`, `/docs/{doc_id}/restore` | `write` | Archive or restore |
| `POST` | `/search`, `/verify` | `read`* | BM25 search with citations; verify a citation |
| `GET` | `/events` | `read` | Wiki audit events |
| `POST` / `GET` | `/import`, `/export` | `*` | OKF import / export |
| `POST` | `/digest` | `write` | Run WikiDigest graph extraction |

Management (`/v1/{space_id}/management`, CWT only):

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/space_tokens` | List Space tokens |
| `POST` | `/add_space_token` | Mint a token (`*` needs a `*`-scoped CWT) |
| `POST` | `/revoke_space_token` | Revoke a token by value or name |
| `PATCH` | `/update_space` | Name, description, visibility, `memory_policy`, wiki settings |
| `PATCH` | `/restart_formation` | Restart a stuck formation |
| `GET` / `PATCH` | `/space_byok` | Read / set the Space's own model configuration |
| `POST` | `/shadow_eval` | Compare a candidate `MemoryPolicy` on forked copies |

Admin (CWT from a principal in `MANAGERS`):

| Method | Path | Description |
| :--- | :--- | :--- |
| `POST` | `/admin/create_space` | Create a Space: `{user, space_id, tier}` |
| `POST` | `/admin/{space_id}/update_space_tier` | Change a Space's tier |

Runtime (`/v1/{space_id}`, requires `BRAIN_RUNTIME_CONFIG` mappings; see
[RUNTIME.md](RUNTIME.md)):

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/attention` | The caller's durable attention inbox (reading claims nothing) |
| `POST` | `/attention/{id}/responses` | Answer a clarification or record an agent statement |
| `POST` | `/outcomes` | Independent outcome from a signed, registered observer |
| `GET` | `/runtime/status` | Configured runtime capabilities and a bounded visible inventory |

Typical calls:

```bash
# Formation
curl -sX POST "$BRAIN/v1/my_space/formation" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"messages": [{"role": "user", "content": "I prefer dark mode.", "name": "Alice"}],
       "context": {"counterparty": "alice", "agent": "settings_bot"},
       "timestamp": "2026-03-09T10:30:00.000Z"}'

# Recall
curl -sX POST "$BRAIN/v1/my_space/recall" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "What are Alice'\''s preferences?", "context": {"counterparty": "alice"}}'

# Maintenance
curl -sX POST "$BRAIN/v1/my_space/maintenance" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '{"trigger": "on_demand", "scope": "full"}'

# Read-only KIP
curl -sX POST "$BRAIN/v1/my_space/execute_kip_readonly" -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" -d '"DESCRIBE PRIMER"'
```

Business agents can register Recall as a function call; `RecallAgent::definition()`
is its definition.

---

## MCP server

With the `mcp` feature, the HTTP service mounts a Streamable HTTP MCP endpoint at
`{MCP_HTTP_PATH_PREFIX}/{space_id}` (default `/mcp/{space_id}`). Clients send the same
CWT or Space token as REST in `Authorization: Bearer …`. For local clients, run a
stdio server:

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" ./anda_brain mcp --space-id my_space local --db ./data
```

Both transports share the HTTP service's model, auth and storage configuration and
the process-wide model concurrency budget. The storage subcommand after `mcp` is
optional (in memory when omitted).

| Tool | Purpose | Scope |
| :--- | :--- | :--- |
| `anda_brain_memory` | Memory Interface request (`{request}`) | recall `read`; mutations `write` |
| `anda_brain_stage_memory_source` | Stage observed messages; returns a `source_ref` | `write` |
| `anda_brain_memory_receipt` | Read a receipt's progress | `read` |
| `anda_brain_remember_conversation` | Encode conversation messages (Formation) | `write` |
| `anda_brain_recall_memory` | Ask memory a question (Recall, optional `budget`) | `read` |
| `anda_brain_run_maintenance` | Run a maintenance cycle | `write` |
| `anda_brain_get_space_info` | Space statistics and metadata | `read` |
| `anda_brain_get_formation_status` | Formation/maintenance progress | `read` |
| `anda_brain_execute_kip_readonly` | Read-only KIP (KQL/META, 15-second bound) | `read` |
| `anda_brain_get_or_init_user` | Get or create a counterparty Concept | `write` |
| `anda_brain_list_conversations` | Page through tracked conversations | `read` |
| `anda_brain_get_conversation` | One conversation or its delta | `read` |
| `anda_brain_get_attention` | The caller's runtime attention inbox | verified `read` + mapping |
| `anda_brain_respond_attention` | Answer a clarification or record a statement | verified `write` + mapping |
| `anda_brain_get_runtime_status` | Configured runtime capabilities | verified `read` |
| `anda_brain_wiki_search` | Wiki search with verifiable citations | `read` |
| `anda_brain_wiki_read` | Progressive wiki reads (TOC, section, range) | `read` |
| `anda_brain_wiki_commit` | Commit a wiki document version | `write` |
| `anda_brain_wiki_verify` | Verify a wiki citation | `read` |

The wiki tools join when the `wiki` feature is on (always, for the service binary).
Read tools work on public Spaces without a token. Remote MCP checks the `Host` header
against `MCP_HTTP_ALLOWED_HOSTS` and browser origins against
`MCP_HTTP_ALLOWED_ORIGINS`; set them when serving behind a domain or proxy.
`MCP_HTTP_AUTO_CREATE_SPACE` creates a missing Space on first use, which requires
`ED25519_PUBKEYS` and a `write` CWT for that Space; `--mcp-auto-create-space` does the
same for stdio. No MCP tool schema uses `oneOf`, `anyOf` or `allOf`.

---

## Diagnostics and observability

- **Probe.** `POST /v1/{space_id}/probe` (`{"query", "limit"}`) answers "do I know
  anything about this?" with pure search — no model call. Explicitly exhaustive misses
  enter a negative cache (cleared when Formation completes, 1-hour TTL). `found: false`
  is a retrieval result, not a rejected belief. Probe first; pay for Recall when
  `found` is true.
- **Memory status.** `GET /v1/{space_id}/memory_status` returns counters maintained at
  write time (recalls, probe hits/misses, self-test groundability, corrections,
  forgets), derived rates (probe hit rate, correction rate, mean self-reported
  uncertainty, maintenance tokens per recall), graph counts including
  `predicate_types`, and the latest settlement, self-test and shadow reports. Reading
  it never runs heavy queries. Correction discovery reports
  `correction_scan_incomplete`, `correction_scan_through_seq` and
  `correction_scan_error`.
- **Shadow evaluation.** `POST /v1/{space_id}/management/shadow_eval` forks the Space
  twice into isolated in-memory stores (current vs candidate `MemoryPolicy`), settles
  both, replays recent real recall queries on each and has a judge compare the answers
  blind, alternating A/B order. The live Space is only read. Set an independent judge
  with `JUDGE_MODEL_*`; otherwise the Space's own model judges. The report is stored in
  the `shadow_report` extension; promoting the candidate stays a human
  `update_space` decision.
- **Conversations.** Formation, Recall and Maintenance runs are kept as conversations
  (`collection=formation|recall|maintenance`), readable in full or as deltas.

---

## Wiki (`wiki` feature)

The wiki is a Space's versioned reference memory — policies, manuals, SOPs, API docs.

- Writes are immutable Markdown versions with compare-and-swap on `parent_version`;
  a conflict returns 409 with the version to rebase on. Content is capped at 1 MiB.
- Documents may carry an `acl_label`. Label-restricted tokens see unlabeled content
  plus their labels; anonymous readers of public Spaces see unlabeled content only.
  ACL checks and content selection use one document snapshot.
- The TOC follows ATX headings independently of retrieval chunks; reads are capped at
  256 KiB and continue by byte range. Citations are verifiable `wiki://` URIs.
- OKF import/export round-trips unknown YAML values.
- **WikiDigest** (disabled by default) extracts graph claims from documents. It keeps
  per-document pending state, skips unchanged bodies, and withdraws an old claim only
  after an explicit absence review over every body batch or withdrawal of the source —
  an omission is not negative evidence.
- Recall uses `wiki_search` / `wiki_read` alongside the graph.

See [API.md › Wiki](API.md#43-wiki-endpoints-v1space_idwiki).

---

## Optional runtimes and host contracts

None of these change behavior until a host configures them. Model output never
installs an executor, observer, evaluator or authority.

| Capability | What it adds | Guide |
| :--- | :--- | :--- |
| **Watch scheduling** | A persistent scheduler advances structured Watches across registered Spaces, independently of Maintenance, resuming after eviction or restart. Firing commits the transition, a `watch_fire` Activity and a protected wake atomically. | [RUNTIME.md](RUNTIME.md) |
| **Action callbacks** | Trusted Rust hosts install `ActionBindings` for a four-way decision gate, clarification and fenced dispatch. None are installed by default. | [RUNTIME.md](RUNTIME.md#action-callbacks-and-authority) |
| **Runtime API** | `BRAIN_RUNTIME_CONFIG` selects a compiled inbox adapter and explicit identity/observer mappings for `/attention`, `/outcomes` and `/runtime/status`. | [RUNTIME.md](RUNTIME.md#startup), [runtime.example.json](runtime.example.json) |
| **Semantic Watches** | A pinned evaluator judges prose and mixed Watch conditions over immutable pages; unknown, missing or truncated judgments never advance coverage. | [SEMANTIC_WATCH_RUNTIME.md](SEMANTIC_WATCH_RUNTIME.md), [example](semantic.runtime.example.json) |
| **Learning** (`learning` feature) | Frozen paired-trial contracts, a Nexus evaluator and a persistent trial/settlement/review runtime with a `workflow_http_v1` adapter. Requires a real executor, independently authenticated observers and reviewed calibration. | [Contracts](#native-learning-contracts), [LEARNING_RUNTIME.md](LEARNING_RUNTIME.md), [example](learning.runtime.example.json) |
| **Memory utility** | Verifiable off-graph Recall receipts, independent contribution attribution, bounded utility calibration and optional ranking within existing Recall priorities. | [UTILITY_RUNTIME.md](UTILITY_RUNTIME.md), [example](utility.runtime.example.json) |
| **Contextual trust** | Reviewable source-trust proposals from independently verified facts, applied only through current `manage_trust` authority. | [TRUST_RUNTIME.md](TRUST_RUNTIME.md), [example](trust.runtime.example.json) |
| **Experiments** (`experiments` feature) | Isolated host runs, immutable snapshots, completion waits, business time, cost receipts with explicit unknowns, forced Recall budgets and evaluator-only procedure audits. | [Below](#isolated-experiments-and-mib-integration), [src/space/experiments.rs](src/space/experiments.rs) |
| **Memory product contracts** | The Rust `product` module: Assertion-backed `MemoryRecord` views, source-backed ingestion, reviewed `Correct` / `WorldChange` changes, `Suppress` / `Delete` with source fences, and recipient-owned record Watches. The embedding host must enforce visibility. | [API.md › Trusted host contracts](API.md#trusted-host-memory-product-contracts) |

Procedures stay **unproven** without the learning pipeline:
`Skill.current_revision` points at an immutable `SkillRevision`, shared `task_family`
only discovers candidate controls, and `skills.unsupported_reason` in the settlement
report says why no verdict ran. Mechanism tests use deterministic fixtures; they are
not evidence of empirical improvement.

### Native learning contracts

With the `learning` feature, `anda_brain::learning` provides `PairedTrialPlan`,
`ExecutionContract`, `PairedRule` and `register_paired_rule`. A trusted host freezes
the plan as an artifact before the baseline runs, uses its pin for both arms'
`AttemptRecord.selection_policy` and `TrialRecord.parameters`, and registers the rule
once per Nexus instance (again after every restart). `attempt_context()` and
`comparability()` build the pinned KIP fields.

The `anda-brain:paired-bounded-v2` rule compares one candidate revision with a stable
task policy under the same factual memory, model, tools, budget and task/state/seed
pairs. It needs the whole predeclared cohort, a one-sided Hoeffding lower bound above
the practical-improvement margin and an absolute candidate failure-rate ceiling.
Missing treatment outcomes count as failures; missing or unknown control outcomes
leave the comparison insufficient. Duplicate pairs or observations, changed pins and
extra applied revisions are rejected. A monitoring trial keeps the original
acquisition through `AdoptionBasis`. Multi-Skill bundles, adaptive stopping and
windowed ledgers are not supported, and no parameter has a production default.

`ExecutionContract` pins tool/time/token ceilings, the cutoff and the review deadline;
call `validate_settlement()` before a final verdict. `workflow_contract()` supplies
the first resettable task-family contract
([workflow-contract-v1.json](assets/learning/workflow-contract-v1.json)).

`Space::learning()` is the persistent host runtime: explicit registration, frozen
enrollment, bounded `drive` steps, recovery after restart, separately authenticated
Outcome ingestion, fixed-cutoff `settle`, persistent `reviews` / `enroll_review`,
`submit_safety_signal` for independent revocation, and `procedure_status` /
`bind_application_context` for read-time eligibility. Native leases, executable
authority and dependency validity gate dispatch; finishing a cohort adopts nothing,
and a recommendation never grants execution permission. Configured learning Spaces
cannot be forked with their operational journal. Background advancement, the
`workflow_http_v1` adapter and archival are configured as described in
[LEARNING_RUNTIME.md](LEARNING_RUNTIME.md). Run the mechanism tests with:

```bash
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib --features learning learning::
```

<a id="mib-integration"></a>
<a id="isolated-experiments"></a>

### Isolated experiments and MIB integration

With the `experiments` feature, the host-only `Experiment` owner supplies isolated
stores, quiescent immutable snapshots, per-conversation completion waits, monotonic
business time (`advance_to`), session boundaries and cost receipts that keep unknown
values unknown. It enables the Nexus `simulation` API for lifecycle expiry only;
authentication, lease and audit clocks stay real, and no HTTP/MCP clock override
exists. Agent Notes persist under each Space's `engine/` prefix and are part of
snapshots. `Experiment::create_with_recall_budget` pins a forced Recall budget before a
run is exposed, across forks and session boundaries; `audit_procedures()` returns a
bounded evaluator-only native inventory (256 items per kind, 4 MiB) whose truncated
counts cannot prove absence. Neither enables learning or grants execution authority.

The sibling [Anda Bot](https://github.com/ldclabs/anda-bot) `mib` feature builds on
this for an isolated loopback host: `/mib-agent/v0.1` for the agent and
`/mib-memory/v0.1` for the memory backend. These are not routes of this service. See
the [Bot host contract](https://github.com/ldclabs/anda-bot/blob/main/docs/mib-integration.md),
the [MIB memory backend contract](https://github.com/ldclabs/MIB/blob/main/docs/harness/MIB-Memory-Backend.md)
and the [longitudinal harness](https://github.com/ldclabs/MIB/blob/main/docs/harness/MIB-Learning-Longitudinal.md).
The Bot currently provides persistent and no-memory modes; native normal/ungated
learning bindings remain pending.

---

## Space lifecycle

- **Creation** (`POST /admin/create_space`) creates an AndaDB database for the Space,
  initializes the Cognitive Nexus, activates the Cognitive Memory Profile and records
  the owner. Drafted vocabulary is Space state the engine keeps across activations.
- **Tiers.** Formation refuses new input once a Space holds more Concepts (or
  conversations) than its tier allows: 10^(tier+2) — 100 at tier 0, 1,000 at tier 1.
- **Loading.** Spaces load lazily on first access and stay cached.
- **Background work.** Every 5 minutes the service flushes active Spaces and checks
  the maintenance clock; Spaces idle for 9 minutes are evicted, unless pinned,
  processing or still referenced. Owned native writes survive cancelled requests and
  drain before a database closes.
- **Shutdown** closes every Space so AndaDB flushes collections and metadata.
- **One writer per Space.** The usage ledger, settlement, self-test and cache guards
  are in-process. Sharding assigns each Space to exactly one process; never point two
  instances at the same Space's storage.

<a id="upgrading-a-space-written-by-a-kip-1x-build"></a>

### Upgrading KIP 1.x Spaces

A Space written by a KIP 1.x build migrates in place when first opened. Stop the old
writer, take a consistent backup and rehearse on a copy first; rollback needs that
backup. The migration checkpoints extraction and vocabulary so it can resume, keeps
the original rows in `kip_legacy_v1`, and maps conservatively:

| v1 data | v2 representation |
| :--- | :--- |
| `(type, name)` identity | Immutable Concept `key`; standard Person / Event / Insight / Commitment / SleepTask fields normalized; a 1.x `Preference` keeps an open legacy type |
| A recorded claim | Proposition + `mode: "imported"` Assertion, preserving confidence and resolvable author |
| Retracted or superseded claim | Native lifecycle where the same-actor revision is reconstructible; otherwise archived, never revived as current belief |
| `valid_from` / `valid_until`; `expires_at` | Assertion valid time; record retention |
| `pinned`; mnemonic values | Pinned retention class; `MnemonicState`, never copied from confidence |
| Unsupported learning/runtime artifacts | Distinct `Legacy*` types under `kip://legacy/nexus@1.1.0`, without standing or leases |

Old-id usage rows, miss caches, derived metrics and scan cursors reset once;
conversations, tokens, policies and wiki records are kept. Malformed identifiers can
stop the migration — inspect the reported row and retry on the backup rather than
treating an unopened Space as empty.

Spaces activated under the KIP 2.1.0 draft are not migrated by the service. Use the
standalone [`tools/migrate-draft-space`](../tools/migrate-draft-space/README.md).

---

## Configuration

### Service options

Every option is a flag and an environment variable; a `.env` file is read at startup.

| Environment variable | Flag | Default | Description |
| :--- | :--- | :--- | :--- |
| `LISTEN_ADDR` | `--addr` | `127.0.0.1:8042` | Listen address |
| `ED25519_PUBKEYS` | `--ed25519-pubkeys` | — | Comma-separated Base64 Ed25519 public keys for CWT verification; empty disables authentication |
| `MANAGERS` | `--managers` | — | Comma-separated principals allowed to use `/admin` endpoints |
| `MODEL_FAMILY` | `--model-family` | `anthropic` | Provider protocol: `anthropic`, `openai`, `gemini`, … |
| `MODEL_API_BASE` | `--model-api-base` | `https://api.deepseek.com/anthropic` | Provider base URL |
| `MODEL_NAME` | `--model-name` | `deepseek-v4-pro` | Model for all three agents |
| `MODEL_API_KEY` | `--model-api-key` | — | Provider key; empty disables the default model |
| `MODEL_CONTEXT_WINDOW` | `--model-context-window` | `400000` | Context window (tokens) |
| `MODEL_MAX_OUTPUT` | `--model-max-output` | `384000` | Max output (tokens); use ≤ 128000 for Claude |
| `HTTPS_PROXY` | `--https-proxy` | — | Proxy for outbound model requests |
| `SHARDING_IDX` | `--sharding-idx` | `0` | Shard index served by this instance |
| `CORS_ORIGINS` | `--cors-origins` | — | Empty = disabled, `*` = any origin, or a comma-separated list |
| `HTTP_MAX_CONCURRENCY` | `--http-max-concurrency` | `1024` | In-flight request cap; excess requests get 503 |
| `LLM_MAX_CONCURRENCY` | `--llm-max-concurrency` | `64` | Model-call cap across Spaces (background work included); excess model-driving requests get 429 |
| `BRAIN_RUNTIME_CONFIG` | `--runtime-config` | — | Path to a versioned JSON runtime configuration (≤ 1 MiB) |
| `MCP_HTTP_ENABLED` | `--mcp-http-enabled` | `true` | Mount Streamable HTTP MCP |
| `MCP_HTTP_PATH_PREFIX` | `--mcp-http-path-prefix` | `/mcp` | Clients connect to `{prefix}/{space_id}` |
| `MCP_HTTP_ALLOWED_HOSTS` | `--mcp-http-allowed-hosts` | — | Allowed `Host` values for remote MCP (`*` = any) |
| `MCP_HTTP_ALLOWED_ORIGINS` | `--mcp-http-allowed-origins` | — | Allowed browser `Origin` values for remote MCP |
| `MCP_HTTP_AUTO_CREATE_SPACE` | `--mcp-http-auto-create-space` | `false` | Create missing Spaces on first remote MCP use |
| `MCP_HTTP_AUTO_CREATE_TIER` | `--mcp-http-auto-create-tier` | `1` | Tier for those Spaces |
| `MCP_SPACE_ID` | `mcp --space-id` | — | Space served by the stdio MCP server |
| `MCP_AUTH_TOKEN` | `mcp --mcp-auth-token` | — | CWT or Space token used by stdio MCP tools |
| `MCP_AUTO_CREATE_SPACE` | `mcp --mcp-auto-create-space` | `false` | Create the stdio Space if missing |
| `MCP_AUTO_CREATE_TIER` | `mcp --mcp-auto-create-tier` | `1` | Tier for that Space |
| `JUDGE_MODEL_API_KEY`, `JUDGE_MODEL_FAMILY`, `JUDGE_MODEL_NAME`, `JUDGE_MODEL_API_BASE` | — | family `openai` | Independent judge for shadow evaluation |
| `LOG_LEVEL` (or `RUST_LOG`) | — | — | Log level for structured JSON logs |

Logs go to stdout for the HTTP service and to stderr in stdio MCP mode.

### Storage

| Subcommand | Storage | Settings |
| :--- | :--- | :--- |
| *(none)* | In memory — lost on exit; a warning is logged | — |
| `local` | Local filesystem | `--db` / `LOCAL_DB_PATH` (default `./db`) |
| `aws` | AWS S3 | `--bucket` / `AWS_BUCKET`, `--region` / `AWS_REGION`, plus `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` |

`mcp` takes the same `local` / `aws` subcommands after its own options.

### Memory policy

Each Space carries an optional `MemoryPolicy` in its `memory_policy` extension, set
with `PATCH /v1/{space_id}/management/update_space`. An absent policy means the
compiled defaults. Explicit maintenance `parameters` override it for one cycle.

| Field | Default | Effect |
| :--- | :--- | :--- |
| `stale_event_threshold_days` | `7` | Events older than this are consolidation candidates |
| `unconsolidated_max_backlog` | `20` | Target for Events/Experiences not yet consolidated (alias `unsorted_max_backlog`) |
| `orphan_max_count` | `20` | Target for orphan Concepts |
| `self_test_queries_per_cycle` | `4` | Self-test probes per cycle; `0` disables |
| `self_test_token_budget` | `20000` | Token budget of one self-test pass |
| `recall_max_rounds` | `7` | Recall model turns, 1–50 |
| `recall_budget` | `null` | Enforced Recall budget ceilings; a request may tighten, never raise them |
| `shadow_replay_sample` | `4` | Recall queries replayed by shadow evaluation (cap 16) |
| `memory_strength_decay_factor`, `decay_floor`, `recall_reinforcement`, `correction_penalty`, `recall_search_threshold` | — | Accepted for stored-policy compatibility; nothing reads them |

### Per-Space models (BYOK)

`PATCH /v1/{space_id}/management/space_byok` gives a Space its own model
configuration; `GET` returns it (including credentials) to a management CWT.

---

## Cargo features

The library defaults to memory only — Formation, Recall, Maintenance and their HTTP
routes.

| Feature | Adds |
| :--- | :--- |
| `wiki` | The wiki (documents, versions, ACL-scoped reads, OKF import/export), the `wiki_search` / `wiki_read` / `wiki_commit` agent tools, WikiDigest, the `/v1/{space_id}/wiki/*` routes and the `wiki_*` fields of `SpaceInfo` / `UpdateSpaceInput` |
| `mcp` | The MCP channel (stdio and Streamable HTTP); with `wiki`, the wiki tools join it |
| `experiments` | Isolated host runs, snapshots, business time, cost receipts, forced Recall budgets, procedure audits; enables the Nexus `simulation` feature |
| `learning` | Paired-trial contracts and evaluator, native records, persistent host trial runtime and the read-only `check_procedure_status` Recall tool |

The `anda_brain` binary is the full product and declares
`required-features = ["mcp", "wiki"]`; Cargo silently skips it without them.

---

## Running

```bash
# In memory (development)
cargo run -p anda_brain --features mcp,wiki

# Local filesystem
cargo run -p anda_brain --features mcp,wiki -- local --db ./data

# AWS S3
cargo run -p anda_brain --features mcp,wiki -- aws --bucket my-bucket --region us-east-1

# Remote MCP behind a domain
MCP_HTTP_ALLOWED_HOSTS="brain.example.com" \
  cargo run -p anda_brain --features mcp,wiki -- local --db ./data

# stdio MCP
MCP_AUTH_TOKEN="$SPACE_TOKEN" \
  cargo run -p anda_brain --features mcp,wiki -- mcp --space-id my_space local --db ./data
```

Docker (the image runs as UID/GID `10001`):

```bash
docker run --rm -p 8042:8042 \
  -e LISTEN_ADDR=0.0.0.0:8042 -e MODEL_API_KEY=your_key \
  -v "$(pwd)/data:/app/db" \
  ghcr.io/ldclabs/anda_brain_amd64:latest local --db /app/db
```

A systemd unit is in [`deploy/systemd`](../deploy/systemd), and the full walkthrough
is [deploy/quick_start.md](../deploy/quick_start.md).

---

## Embedding the library

```toml
# Memory only
anda_brain = "0.13"

# The full surface
anda_brain = { version = "0.13", features = ["mcp", "wiki"] }
```

`AppState` owns the Spaces, model configuration and background tasks; the binary in
[`src/bin/main.rs`](src/bin/main.rs) is the reference wiring (router, CORS,
concurrency limits, MCP mount, graceful shutdown). Builders configure it before it is
shared: `with_agent_prompts`, `with_judge_model`, `with_llm_concurrency`,
`with_runtime_config` and, for an embedded single-Space host, `with_audience_free_cwt`.

Trusted hosts may replace the deployment section of a prompt:

```rust
use anda_brain::agents::prompts::{AgentPrompts, PromptTarget};

let prompts = AgentPrompts::default().with_deployment_section(
    PromptTarget::Recall,
    "# A. Deployment contract\nYour reviewed deployment instructions here.",
)?;
let app = app.with_agent_prompts(prompts)?;
```

Only section A can be replaced (it must start with `# A.`, at most 128 KiB); the
compiled KIP reference prefix stays verbatim. The configuration is immutable, inherited
by agent instances and forks, and must be supplied again at every start. There is no
HTTP or MCP endpoint for prompts and no process-global override.

---

## Prompts and the KIP reference

Each `assets/Brain{Formation,Recall,Maintenance}.md` has two halves. Everything above
`# A.` is the KIP 2.0 reference policy vendored from `anda_kip`; `# A.` and below is
this deployment's contract. The system prompt of every model call assembles the full
`anda_kip` syntax (about 40 KiB), the Cognitive Memory Profile, the role cards and
that prompt. Budgeted Recall counts the whole system prompt toward its input budget
and reports `recall_context_budget_exhausted` rather than dropping the syntax.

All three agents can page through additional compiled-in documentation with the
read-only `kip_reference` tool (`document=index` lists documents, `section=index`
lists headings, pages are at most 8 KiB). Reference pages grant no capability and never
count as retrieved memory.

Do not hand-edit the reference halves or `assets/kip-reference/`; edit section A by
hand and regenerate the rest from the repository root:

```bash
node scripts/sync-kip-assets.mjs
pnpm --filter @ldclabs/anda-brain-worker run codegen:prompts
node scripts/sync-kip-reference.mjs --check
```

`sync-kip-assets.mjs` re-copies the reference halves of the Rust and Worker prompts
from a sibling `anda-db` checkout by default; set `ANDA_KIP_SOURCE` to a downloaded
`anda_kip` crate directory to sync from a published version. `sync-kip-reference.mjs`
regenerates `assets/kip-reference/` (or verifies it with `--check`) and refuses a
source that differs from the `anda_kip` in `Cargo.lock`.

---

## Testing

```bash
cargo fmt --check
cargo clippy -p anda_brain --all-targets --all-features -- -D warnings
RUST_MIN_STACK=16777216 cargo test -p anda_brain --all-features

# The lean builds must keep compiling
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib --features wiki
RUST_MIN_STACK=16777216 cargo test -p anda_brain --lib --features mcp
```

Debug builds need the larger test stack; release code fits the default 2 MiB worker
stack. Tests use local or in-memory storage and never call a model provider; one bin
test binds an ephemeral localhost port.

- **KIP conformance**: [conformance/](conformance/README.md) drives a real process
  through the Memory Interface with a controlled model.
- **Wiki retrieval corpus**: [evals/wiki/](evals/wiki).

<a id="offline-regression-and-instance-configuration"></a>

### Offline regression

The Rust `anda_brain::eval` API and the `anda_brain eval` CLI (including `--optimize`,
`--mine` and their fixture profiles) have been retired. Product regressions live in
the sibling [MIB](https://github.com/ldclabs/MIB) project; Brain keeps its online
instruments — self-test, probe, citations, usage and correction ledgers, shadow
evaluation — and the native learning tests. From a MIB checkout:

```bash
# Public regression contracts, without a model
python scripts/check-brain-product-regression.py --output-dir /tmp/brain-product-regression

# A configured business agent through the normal submission path
python -m mib_runner benchmark \
  --profile profiles/MIB-Brain-Product-Regression-0.1-Dev.json \
  --schema schemas/mib-scenario.schema.json \
  --submission /absolute/path/agent.json \
  --output-report /tmp/product-real.report.json
python -m mib_runner verify-score /tmp/product-real.report.json
```

The contract fixture verifies the harness and its oracle, not model quality. The
[MIB migration guide](https://github.com/ldclabs/MIB/blob/main/docs/harness/MIB-Brain-Legacy-Migration.md)
lists the removed Rust APIs. Instance configuration stays per Space
([memory policy](#memory-policy)) or host-owned ([deployment prompts](#embedding-the-library));
there is no process-global override.

---

## Known limits

- **Full-scan ceiling.** Correction discovery and self-test sampling use full-scan KQL
  that the engine caps at 65,536 solutions. Past that they fail loudly
  (`correction_scan_error` in the settlement report and `memory_status`);
  predicate-sharded scans are not implemented.
- **Memory Interface.** `memory_experience` and `memory_learning` are not advertised;
  `resume` has no maintained WorkingState; a Formation interrupted by a close is
  reported `failed` with an unknown outcome and is not re-run.
- **Erasure scope.** Forgetting removes graph records and the host's own copies; it
  cannot erase an embedding application's original data, backups or provider copies.
- **Learning claims.** Skills stay unproven without configured trials; runtime status
  and fixtures are not calibration or improvement evidence. Missing measurements and
  provider costs stay unknown, never zero.

## Dependencies

| Crate | Role |
| :--- | :--- |
| `anda_core`, `anda_engine` | Agent traits, engine, model adapters, notes |
| `anda_db`, `anda_object_store` | Embedded database and object-store layer |
| `anda_kip` | KIP 2.0 parser, envelopes, errors, syntax and reference documents |
| `anda_cognitive_nexus` | The Cognitive Nexus graph |
| `axum`, `rmcp` | HTTP and MCP servers |
| `tiktoken-rs` | The pinned Recall tokenizer (0.12.x) |

## License

Copyright © LDC Labs. Licensed under the Apache License, Version 2.0.
