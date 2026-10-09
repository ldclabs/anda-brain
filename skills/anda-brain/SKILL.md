---
name: anda-brain
description: |
  Use a self-hosted Anda Brain service to remember conversations, recall facts and
  procedures, maintain structured memory, and read or answer configured attention
  inboxes. Apply when an agent needs persistent memory across sessions or the user
  asks to use Anda Brain. Observer ingestion, runtime installation and trust
  governance require separate trusted-host authority.
metadata:
  version: 0.13.4
  url: https://github.com/ldclabs/anda-brain/blob/main/skills/anda-brain/SKILL.md
  keywords:
    - long-term memory
    - agent memory
    - knowledge graph
    - cognitive nexus
    - memory formation
    - memory recall
    - memory maintenance
    - KIP
    - persistent memory
---

# 🧠 Anda Brain

This skill targets the Rust **Anda Brain 0.13.4** service with KIP 2.0 (`11a82ec`),
`cognitive-memory@2.0.0`, Nexus 0.14 and KIP 0.14. The Cloudflare Worker uses a
separate engine and does not acquire these Rust runtime capabilities automatically.
Check the deployed service and its configuration before choosing an optional path.

Formation and Maintenance share a per-Space writer guard. If explicit Maintenance reports busy, let the active task finish before retrying. A successful Formation submission only acknowledges queued work; its background model calls keep using the host concurrency budget until they finish. A Space closing for eviction can temporarily be unavailable; retry after it finishes.

Prefer the **Memory Interface** (below): stage what you observed, then send one
intent — `observe`, `recall`, `revise`, `feedback` or `forget` — to
`POST /v1/{space_id}/memory`. It is served at the `memory_basic` level only. The
older Formation, Recall and Maintenance endpoints remain available. A Proposition's
existence is not belief; `insufficient` is not false. Corrections append new claims,
and reading never increases confidence or utility. A plain Formation conversation
ID is not a processing receipt; a Memory Interface receipt is, and a recall that
passes it in `after` waits for it.

For reminders or business follow-up, read the authenticated runtime status and inbox
as described under **Attention and independent outcomes** below. A fired Watch is
attention, not proof of delivery or permission. An answer is data, and an executor
ACK is not an independently measured outcome. Procedures remain unproven without
qualifying native trials/evaluations and current applicability checks.

Host bindings, independent observation, learning, semantic evaluation, utility and
trust have separate configuration and authority requirements. A model message,
ordinary write token, a reported score or a calibration digest supplies none of
those permissions. Native attention/learning identities cannot be copied by forks.
A local purge acknowledgement is not proof that external exports or backups were
erased. KIP timestamps use `YYYY-MM-DDTHH:mm:ss.SSSZ`.

Persistent long-term memory service for LLM agents, powered by a Knowledge Graph (Cognitive Nexus) and KIP (Knowledge Interaction Protocol). Anda Brain is [open-source software](https://github.com/ldclabs/anda-brain) designed to be **self-hosted** — deploy your own instance with the [Quick Start guide](https://github.com/ldclabs/anda-brain/blob/main/deploy/quick_start.md).

> **Note:** The hosted cloud service (`brain.anda.ai`) and its console (`anda.ai/brain`) have been discontinued. All examples below assume your own deployment.

For a complete, ready-to-run agent built on Anda Brain, see [Anda Bot](https://github.com/ldclabs/anda-bot).

Business agents interact entirely through **natural language** and a simple REST API — no KIP knowledge required.

```
Business Agent  ──natural language──▶  Brain  ──KIP──▶  Cognitive Nexus
 (your agent)                         (this service)          (knowledge graph)
```

---

The Cloudflare Worker is a separate compact adapter: it now supports reviewed memory changes through trusted host RPC and `/memory/forget`, but not budgeted Recall, Wiki, or the learning/inbox runtime. Use its own README and PRODUCT.md contracts; a nonempty Recall `budget` is rejected there. Worker callers should inspect `operation_results`, preserve nullable `usage` and known subtotals, and handle 409 processing conflicts, 502 model failures and 504 shared-deadline timeouts. These Worker-specific response contracts do not change the Rust endpoints below.

## What You Get

Three operational modes cover the full memory lifecycle:

| Mode | Endpoint | Purpose | Auth |
|------|----------|---------|------|
| **Formation** | `POST /v1/{space_id}/formation` | Encode conversations into structured memory | `write` (CWT or space token) |
| **Recall** | `POST /v1/{space_id}/recall` | Query memory with natural language | `read` (CWT or space token) |
| **Maintenance** | `POST /v1/{space_id}/maintenance` | Trigger memory consolidation & pruning cycle | `write` (CWT or space token) |

Supporting endpoints:

| Method | Endpoint | Purpose | Auth |
|--------|----------|---------|------|
| `GET` | `/` | Anda Brain website | — |
| `GET` | `/info` | Service info (name, version, sharding) | — |
| `GET` | `/SKILL.md` | This skill description | — |
| `GET` | `/v1/{space_id}/info` | Space status and statistics | `read` (CWT or space token) |
| `GET` | `/v1/{space_id}/formation_status` | Formation progress (lightweight monitoring) | `read` (CWT or space token) |
| `POST` | `/v1/{space_id}/execute_kip_readonly` | Execute a read-only KIP request | `read` (CWT or space token) |
| `GET` | `/v1/{space_id}/conversations/{conversation_id}` | Get one conversation detail | `read` (CWT or space token) |
| `GET` | `/v1/{space_id}/conversations/{conversation_id}/delta` | Get incremental conversation updates | `read` (CWT or space token) |
| `GET` | `/v1/{space_id}/conversations` | List conversations (cursor pagination) | `read` (CWT or space token) |
| `GET` | `/v1/{space_id}/management/space_tokens` | List space tokens | `write` (CWT) |
| `POST` | `/v1/{space_id}/management/add_space_token` | Add a space token | `write` (CWT) |
| `POST` | `/v1/{space_id}/management/revoke_space_token` | Revoke a space token | `write` (CWT) |
| `PATCH` | `/v1/{space_id}/management/update_space` | Update space information (name, description, public/private) | `write` (CWT) |
| `PATCH` | `/v1/{space_id}/management/restart_formation` | Restart formation for a conversation (re-encode with updated model/config) | `write` (CWT) |
| `GET` | `/v1/{space_id}/management/space_byok` | Get BYOK configuration for the space | `write` (CWT) |
| `PATCH` | `/v1/{space_id}/management/space_byok` | Update BYOK configuration for the space | `write` (CWT) |
| `POST` | `/admin/{space_id}/update_space_tier` | Update a space tier | manager (CWT) |
| `POST` | `/admin/create_space` | Create a new memory space | manager (CWT) |
> Auth scopes in tables apply when authentication is enabled (`ED25519_PUBKEYS` is set).

---

## When to Use This Service

Use Anda Brain when your agent needs to:

- **Persist knowledge across sessions** — user preferences, facts, decisions, relationships, events
- **Recall previous context** — what happened before, what the user said, what decisions were made
- **Share memory across agents** — multiple agents can read/write to the same space
- **Maintain memory health** — consolidate old events, deduplicate facts, review stale knowledge

The service handles all the complexity of knowledge graph management. Your agent just sends messages and asks questions in natural language.

## When NOT to Use

- Temporary conversation context that only matters in the current session
- Large file storage (use object storage instead)
- Real-time data streaming
- Secrets, passwords, or API keys (the service is not a vault)

---

## Concepts

### Memory Space

Each space is an isolated environment with its own knowledge graph, conversation history, and database. Spaces are identified by a `space_id` string.

### Memory Types

The Formation agent extracts three types of memory from conversations:

1. **Episodic Memory** (Events) — What happened, when, who participated, outcome
2. **Semantic Memory** (Stable Knowledge) — Facts, preferences, relationships, domain knowledge
3. **Cognitive Memory** (Patterns) — Behavioral patterns, decision criteria, communication style

### Cognitive Nexus

The underlying knowledge graph consists of:

- **Concept Nodes** — Entities with a type and name (e.g., `{type: "Person", name: "Alice"}`, `{type: "ColorScheme", name: "dark_mode"}`; an option is typed by its kind)
- **Proposition Links** — Directed relationships between concepts (e.g., `(Alice, "prefers", dark_mode)`)

---

## Authentication

If `ED25519_PUBKEYS` is configured, protected endpoints require a Bearer token in the `Authorization` header.

If `ED25519_PUBKEYS` is empty/not provided, legacy memory endpoints allow local
unauthenticated access. Runtime inbox/status/responses still require verified
credentials and explicit native identity/audience mappings. Independent HTTP
outcomes require a signed observer-mapped CWT; they remain disabled without a
configured verifier. An ordinary Space token cannot become an observer.

```
Authorization: Bearer <base64_encoded_cose_sign1_token>
```

Management endpoints (`/v1/{space_id}/management/*`) and admin endpoints (`/admin/*`) still follow their role/scope checks when auth is enabled.

---

## API Reference

For complete endpoint and TypeScript schema details, see:
- `https://github.com/ldclabs/anda-brain/blob/main/anda_brain/API.md` (English)
- `https://github.com/ldclabs/anda-brain/blob/main/anda_brain/API_cn.md` (中文)

### Content Negotiation

The API supports triple serialization. Set `Content-Type` and `Accept` headers accordingly:

- `application/json` — JSON (default)
- `application/cbor` — CBOR (binary, more compact)
- `text/markdown` — Markdown (raw text or formatted Markdown)

All responses are wrapped in an RPC envelope when using JSON or CBOR:

```json
{
  "result": { ... },
  "error": null
}
```

When `Accept: text/markdown` is used, the response is returned as raw text or a Markdown formatted string.

On error:

```json
{
  "result": null,
  "error": {
    "message": "error description",
    "data": { ... }
  }
}
```

#### Markdown Serialization Sample

If `Accept: text/markdown` is specified, the `result` field's content will be directly serialized as the response body.

**Request:**
```http
POST /v1/my_space_001/recall
Accept: text/markdown

What are Alice's preferences?
```

**Response (HTTP 200):**
```markdown
Alice has the following known preferences:
- **Dark mode** in all applications (confidence: 0.9, since 2025-01-15)
- **Email communication** preferred over phone calls (confidence: 0.8, since 2025-01-10)

Alice is currently working on **Project Aurora** and was last seen on 2025-01-15 discussing settings preferences.

Gaps:
- No information found about Alice's language preferences.
```

---

### Create Space

Create a new isolated memory space.

```
POST /admin/create_space
Authorization: Bearer <token>
Content-Type: application/json
```

**Request:**

```json
{
  "user": "<owner_principal_id>",
  "space_id": "my_space_001",
  "tier": 0
}
```

**Response:**

```json
{
  "result": { ... }
}
```

---

### Formation — Encode Conversations into Memory

Send conversation messages to be analyzed and encoded into the knowledge graph. The service extracts facts, preferences, relationships, events, and patterns, then stores them as structured knowledge.

Processing is asynchronous — the endpoint returns immediately with a conversation ID while encoding continues in the background. New submissions are queued and processed sequentially.

```
POST /v1/{space_id}/formation
Authorization: Bearer <token>
Content-Type: application/json
```

**Request:**

```json
{
  "messages": [
    {
      "role": "user",
      "content": "I prefer dark mode for all my apps. My timezone is UTC+8.",
      "name": "Alice"
    },
    {
      "role": "assistant",
      "content": "Got it! I've noted your preference for dark mode and UTC+8 timezone."
    }
  ],
  "context": {
    "counterparty": "alice_principal_id",
    "agent": "customer_bot_001",
    "source": "source_123",
    "topic": "settings"
  },
  "timestamp": "2026-03-09T10:30:00.000Z"
}
```

**Response:**

```json
{
  "result": { "conversation": 1, ... }
}
```

**Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `messages` | `Message[]` | Yes | Conversation messages (role: `user` / `assistant` / `system`) |
| `context` | `InputContext` | No | Contextual metadata to help with encoding |
| `context.counterparty` | `string` | No | User identifier |
| `context.agent` | `string` | No | Calling agent identifier |
| `context.source` | `string` | No | Identifier of the source of the current interaction content |
| `context.topic` | `string` | No | Conversation topic |
| `timestamp` | `string` | No (recommended) | RFC 3339 instant, canonicalized to UTC milliseconds; unparseable or sub-millisecond is 400; missing uses receipt time |

Send the time the conversation actually happened: formed claims take it as their
`asserted_at`, the start key of temporal succession. A message's own `timestamp`
(Unix ms) overrides it for that message. Offsets and up to millisecond precision
are canonicalized; an unparseable value is rejected rather than replaced, and a
missing one uses the durable conversation creation time. In Markdown
mode, raw text is captured verbatim as one user message with the usual `:msg1`
Evidence binding. Resolving an existing counterparty without `name` preserves its
display name; an explicitly supplied name still updates it.

`POST /v1/{space_id}/memory/forget` accepts explicit `C-*`, `P-*`, `A-*`, `E-*`
and `X-*` IDs with `write` credentials. Use `E-*` to erase captured message
Evidence. Inspect per-entity errors and the `deleted_concepts`,
`deleted_propositions`, `deleted_assertions`, `deleted_evidence` and
`deleted_activities` counts; legal holds and native reference checks still apply.
Graph erasure does not scrub stored conversations, wiki documents or external copies.

`context.source` identifies a thread/channel for provenance. Reusing it for later
messages is supported; it is not an idempotency key. Rust retries of a persisted
formation keep that conversation's Evidence identity. A new submission is a new
conversation; use the stored conversation/restart path when retrying existing work.

**Tips for best results:**

- Include the `context` field whenever possible — it helps the encoder associate knowledge correctly
- Send complete conversation segments, not individual messages
- Include timestamps (the time things were said, not the time you submit) so a late submission never overrides newer memory
- The `name` field in messages helps distinguish between multiple users in the same conversation

---

### Recall — Query Memory

Ask a natural language question and receive a synthesized answer drawn from the knowledge graph and conversation history.

```
POST /v1/{space_id}/recall
Authorization: Bearer <token>
Content-Type: application/json
```

**Request:**

```json
{
  "query": "What are Alice's preferences?",
  "context": {
    "counterparty": "alice_principal_id",
    "topic": "settings"
  }
}
```

**Response:**

```json
{
  "result": {
    "content": "Alice prefers dark mode for all applications and operates in the UTC+8 timezone.",
    ...
  }
}
```

Note: `result.content` is the primary contract. Additional fields may vary by model/runtime.

**Optional budget mode (Rust service):** add `budget` with the fixed tokenizer
`o200k_base@tiktoken-rs-0.12` (the earlier `…-0.12.0` spelling is accepted),
`max_tokens` (1–65536), and `context_tokens` (1–131072). An explicit empty object
defaults to 4096 output / 49152 cumulative normalized planning-input tokens. `memory_policy.recall_budget` can enforce
ceilings; omitting the request budget cannot disable that policy.

In this mode `content` is one compact JSON memory packet, not a synthesized
answer. Its complete text, including coverage/escaping, fits `max_tokens`.
Transport wrappers, MCP replicas and provider billing templates are separate.
The host preserves necessary constraints/warnings and clears history, thoughts,
tool calls and artifacts from the response. `recall_structured.answer` carries
the same packet and `memory_budget` reports its count; extra trace citations are
not copied outside the packet. Treat `budget_insufficient` or literal `null`
with `failed_reason` as unusable/incomplete. `semantic_complete` and
`action_ready` remain false; this packet is not a Memory Interface briefing.
Each query has host-side concept discovery before selection. Compact views
carry `recall_detail` references for omitted fields; fetch projected attributes
when more detail is needed. Pending/blocked commitments and warnings remain
mandatory. A `bounded` packet can be partial: inspect coverage and warning
items, including `recall_context_budget_exhausted` when only host-read candidates
could be delivered. It never establishes semantic completeness or execution
permission. Required reads/constraints that cannot fit still fail closed.
See [public contract](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/API.md#recall-budget-contract) for the exact scope and failure codes.

**Query examples:**

| Intent | Example query |
|--------|--------------|
| Entity lookup | "Who is Alice?" |
| Relationship | "Who does Alice work with?" |
| Attribute | "What are Alice's preferences?" |
| Event recall | "What happened in our last meeting?" |
| Domain exploration | "What do we know about Project Aurora?" |
| Pattern detection | "Does Alice prefer email or chat?" |
| Existence check | "Have we discussed the pricing strategy?" |

---

### Space Status

Get statistics and health information for a memory space.

```
GET /v1/{space_id}/info
Authorization: Bearer <token>
```

**Response:**

```json
{
  "result": {
    "space_id": "my_space_001",
    "owner": "principal_id",
    "db_stats": {
      "total_items": 150,
      "total_bytes": 524288
    },
    "concepts": 85,
    "propositions": 120,
    "conversations": 12,
    ...
  }
}
```

---

### Wiki — Versioned Reference Documents with Citations

The wiki is the space's reference memory: policies, manuals, SOPs, API docs and FAQs stored as immutable Markdown commits (git-like), retrieved by BM25 keyword search, and quoted through verifiable `wiki://` citations. Search is deterministic and LLM-free; compose answers yourself and cite the URIs.

**Commit (create or update):**

```
POST /v1/{space_id}/wiki/docs
Authorization: Bearer <token with write scope>
```

```json
{
  "title": "Deployment Guide",
  "content": "# Deployment Guide\n\n## Rollback\n\nUse the previous snapshot...",
  "namespace": "engineering",
  "tags": ["sop"],
  "message": "initial import"
}
```

- Create: omit `doc_id`. Update: pass `doc_id` **and** `parent_version` (the `current_version` you read). A stale `parent_version` returns `409` with the current version in `error.data` — re-read, merge, retry.
- Committing identical content is a no-op (`"idempotent": true`); safe to retry and re-import.
- Content is whole-document Markdown (not a diff), at most 1 MiB after normalization (`413` beyond).

**Search with citations:**

```
POST /v1/{space_id}/wiki/search
```

```json
{ "query": "rollback checksum", "namespaces": ["engineering"], "top_k": 8, "mode": "chunks", "expand": 1 }
```

Each hit carries the matching text and a citation: `{ "uri": "wiki://{space}/{doc_id}@{version_id}#{start}-{end}", "checksum": "sha3-256:...", "anchor": "...", "quote": "..." }`. `mode: "docs"` returns one best hit per document. `expand` (0-2, default 0) widens each hit with adjacent passages; each expansion uses immutable source text and widens its citation range; nearby hits may overlap. BM25 favors exact terms (product names, error codes); reformulate keywords rather than sending full sentences.

**Read progressively:**

```
GET /v1/{space_id}/wiki/docs/{doc_id}                      → metadata + table of contents
GET /v1/{space_id}/wiki/docs/{doc_id}/content?anchor=...   → one section
GET /v1/{space_id}/wiki/docs/{doc_id}/content?start=&end=  → byte range
GET /v1/{space_id}/wiki/docs/{doc_id}/content              → full text (bounded)
GET /v1/{space_id}/wiki/docs/{doc_id}/content?version=...  → historical version
```

Prefer TOC → section over full reads for long documents. The TOC follows ATX headings (`#`–`######`) independently of retrieval chunks; sections include their descendants. All text selectors return at most 256 KiB. Use `truncated` and the returned `byte_range` to continue reading. Historical reads and verification accept only published versions.

**Manage and audit:**

```
GET  /v1/{space_id}/wiki/docs?namespace=&tag=&status=&cursor=&limit=
GET  /v1/{space_id}/wiki/docs/{doc_id}/versions
POST /v1/{space_id}/wiki/docs/{doc_id}/archive     (hidden from search, still readable)
POST /v1/{space_id}/wiki/docs/{doc_id}/restore
POST /v1/{space_id}/wiki/verify                    {"uri": "wiki://...", "checksum": "sha3-256:..."}
GET  /v1/{space_id}/wiki/events?kind=&doc_id=
```

`verify` answers `valid`, `superseded` (a newer version exists — it names it), or `invalid`. Versions are immutable, so citations never rot.

**Access control (ACL labels):**

Documents may carry an `acl_label` (set via commit, or inherited from a per-namespace default configured with `update_space {"wiki_acl_defaults": {"hr": "hr-internal"}}`). Space tokens may carry `labels`: a token with labels sees only unlabeled documents plus its granted labels — prefiltered in the query and then checked against the authoritative document version, status and ACL. Content reads authorize and select a version from the same document snapshot. Tokens without labels and CWT holders are unrestricted; anonymous readers of public spaces see unlabeled content only. Denials surface as 404 (existence does not leak); the audit log, agentic recall, and the conversations endpoints (which persist full recall runner history) all require an unrestricted token and answer `403` to a labeled one. Note: OKF bundles do not carry ACL labels (the exchange format cannot express enterprise ACLs) — imported documents inherit namespace defaults.

```
POST /v1/{space_id}/management/add_space_token
{"scope": "read", "name": "analyst", "labels": ["engineering"]}
```

**Read auditing and housekeeping:**

`update_space {"wiki_audit_reads": true}` events every external search/read (`WikiQueried` / `WikiRead`, with the real actor); agent reads stay covered by recall conversation logs. Housekeeping runs automatically after maintenance cycles and on startup: the audit log is pruned to its retention cap (the prune itself is evented), and a stale-document report is refreshed. `GET /v1/{space_id}/info` exposes `wiki_docs / wiki_chunks / wiki_versions / wiki_queries / wiki_digested / wiki_stale_docs`.

**Graph bridge (WikiDigest, opt-in):**

```
PATCH /v1/{space_id}/management/update_space   {"wiki_digest": true}
POST  /v1/{space_id}/wiki/digest
```

When enabled, WikiDigest processes durable pending document generations, up to 20 per run. Commit, archive, restore and ACL changes enqueue work; failed documents remain pending and increment `WikiDigestReport.failed`. Retry by calling the endpoint again. Startup and post-maintenance hooks also process pending work. The `wiki_digested` version high-water mark is diagnostic, not a coverage guarantee.

The model proposes facts; the host writes attributed Assertions with versioned citation Evidence. An unchanged body checksum reuses the existing ledger without a model call. **Omission from a bounded extraction never proves withdrawal.** The model must explicitly review each old claim against every source batch; only all-`absent` reviews authorize retracting that source's Assertion. Missing, duplicate or `unknown` reviews retain it, and the ledger retains ownership so later reconciliation can still withdraw it. Archive or labeling withdraws the document's claims during a subsequent digest; this graph synchronization is asynchronous. Concurrent edits fence out stale extraction results. Propositions and other sources' Assertions are preserved. Digest remains off by default, and mechanism tests do not establish real-model extraction quality.

**OKF interchange (requires full-scope token):**

```
POST /v1/{space_id}/wiki/import    {"entries": [{"path": "guides/setup.md", "content": "---\ntype: Guide\n---\n\n# ..."}], "namespace": "kb"}
GET  /v1/{space_id}/wiki/export?namespace=kb
```

Bundles follow the OKF v0.1 convention (Markdown + YAML frontmatter; concept paths become hierarchical slugs). Unknown frontmatter key/value pairs survive structurally, including after title/tag edits; YAML comments, key order and scalar formatting are canonicalized. Re-importing unchanged values is a no-op. Import replaces the file-owned fields, so deleting tags, resource, type or unknown frontmatter fields clears them, while preserving existing ACLs and unrelated host metadata. Export adds `x_anda_doc_id` / `x_anda_version_id` / `x_anda_checksum` provenance keys plus a root `index.md` and `manifest.json`, so a wiki snapshot can be reviewed with git diff and replayed into an empty space. Reserved files (`index.md`, `log.md`, non-Markdown) are skipped on import.

---

## Memory Interface (recommended)

Five intents, one request shape (`kip_memory: "2.0"`), one intent per request. The
host keeps the retry identities and handles; you never write KIP, re-type observed
bytes or invent scores. Check `GET /v1/{space_id}/info` → `memory_interface` for the
advertised levels (`memory_basic` only) and budgets.

**1. Stage what you observed** and keep the `source_ref`:

```bash
curl -sX POST https://your-brain-host/v1/my_space_001/memory/sources \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"messages": [{"role": "user", "content": "I moved to Shanghai on 2026-09-01."}],
       "observed_at": "2026-09-02T08:00:00.000Z", "idempotency_key": "chat-42:msg-7"}'
```

**2. observe** it (a mutation: always send an `idempotency_key` you can retry with):

```bash
curl -sX POST https://your-brain-host/v1/my_space_001/memory \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"kip_memory": "2.0", "operation": "observe", "idempotency_key": "observe:chat-42:msg-7",
       "scope": {"task_ref": "relocation"}, "input": {"source_ref": "src-…"}}'
```

The Response carries a `receipt` and `progress`: `recorded` means durable but not
yet formed (`status: "pending"`); `available` means recall can include it, with an
honest `disposition` (`formed`, `evidence_only` or `skipped`). Keep every receipt you
have not yet seen available.

**3. recall** before you act, passing those receipts in `after`:

```json
{"kip_memory": "2.0", "operation": "recall", "scope": {"task_ref": "relocation"},
 "budget": {"max_output_tokens": 1200, "deadline_ms": 30000},
 "input": {"query": "Where does the user live now?", "mode": "action", "after": ["rcpt-…"]}}
```

Read the `Briefing`: `items[].epistemic_status` is final belief (`insufficient` is
not "no"), `coverage` names seven channels and any `pending_receipts`, and
`action_eligible` says whether memory is sufficient — never whether you may act.
Required constraints (and open commitments) always come back. Expand an item or the
whole result with `{"target_ref": "<basis_ref or item ref>", "detail": "evidence"}`.
Poll `{"mode": "attention", "attention_cursor": "<last cursor>"}` to receive due
Commitments and fired Watches; keep the returned cursor. An item grants nothing.

**4. revise** with the right history: `correction` (they were wrong),
`world_change` (it was true, now it changed), `misrecorded` (the Brain recorded
something they never said — name the wrong Assertion in `target_ref`; it is repaired,
never recorded as their retraction), or `unspecified` (recorded as new claims only).

**5. feedback** preserves a report with its actual origin (an assistant's own report
is `agent_statement`); it never grades or promotes anything.

**6. forget** `{"target_ref": "A-…" | "E-…" | "src-…", "mode": "payload_only" |
"semantic"}` runs an ErasurePlan; `semantic` needs the owner's CWT. Report forgetting
as done only when `result.status` is `completed`. Missing source ownership traces
or unfinished source processing are reported as partial, not completed.

Retrying a mutation with the same key replays the original receipt; the same key
with different content fails `IdempotencyConflict`. Receipt progress is also at
`GET /v1/{space_id}/memory/receipts/{receipt_ref}`. MCP clients use
`anda_brain_stage_memory_source`, `anda_brain_memory` (`{request}`) and
`anda_brain_memory_receipt`.

## Integration Pattern

The older endpoints remain available for existing integrations (replace `your-brain-host` with your deployment address, e.g. `localhost:8042`):

### 1. Remember: Send conversations for memory encoding

After each meaningful conversation with a user, send the messages to Formation:

```bash
curl -sX POST https://your-brain-host/v1/my_space_001/formation \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [
      {"role": "user", "content": "I work at Acme Corp as a senior engineer."},
      {"role": "assistant", "content": "Nice to meet you! Noted that you are a senior engineer at Acme Corp."}
    ],
    "context": {"counterparty": "user_123", "agent": "onboarding_bot"},
    "timestamp": "2026-03-09T10:30:00.000Z"
  }'
```

### 2. Recall: Query memory before responding

Before generating a response, check if relevant memory exists:

```bash
curl -sX POST https://your-brain-host/v1/my_space_001/recall \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "query": "Where does this user work and what is their role?",
    "context": {"counterparty": "user_123"}
  }'
```

---

## MCP Integration

If your agent has an MCP client, connect to the HTTP service's Streamable HTTP MCP endpoint:

```text
https://your-brain-host/mcp/my_space_001
```

Use the same `spaceId` and `spaceToken` you would use for REST. Pass the token as `Authorization: Bearer <token>`. This is the preferred setup for company or team deployments where each employee's agent is assigned a dedicated Brain space.

For local desktop or development clients, run Anda Brain as a stdio MCP server and register it with that client:

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" \
  anda_brain mcp --space-id my_space_001 local --db ./data
```

Core MCP tools:

| Tool | Purpose |
|------|---------|
| `anda_brain_memory` | The Memory Interface: one observe/recall/revise/feedback/forget request |
| `anda_brain_stage_memory_source` | Stage observed messages; returns the `source_ref` |
| `anda_brain_memory_receipt` | Read a Memory Interface receipt's progress |
| `anda_brain_remember_conversation` | Encode conversation messages into long-term memory |
| `anda_brain_recall_memory` | Query memory with natural language |
| `anda_brain_run_maintenance` | Trigger consolidation and pruning |
| `anda_brain_get_space_info` | Inspect space statistics and metadata |
| `anda_brain_get_formation_status` | Check formation/maintenance progress |
| `anda_brain_execute_kip_readonly` | Run read-only KIP for advanced graph inspection |
| `anda_brain_get_attention` | Read only the caller-visible bounded inbox page |
| `anda_brain_respond_attention` | Submit the recipient's clarification or attributed statement |
| `anda_brain_get_runtime_status` | Inspect current bindings, switches and recovery state |

If `ED25519_PUBKEYS` is empty, core local MCP memory tools can omit tokens; runtime tools still require verified credentials and mappings. Add `--mcp-auto-create-space` for stdio development or `MCP_HTTP_AUTO_CREATE_SPACE=true` for remote development when the target space does not exist yet; remote auto-create requires `ED25519_PUBKEYS` plus a CWT with `write` scope for the target space before creating the missing space. Set `MCP_HTTP_ALLOWED_HOSTS` when remote MCP is exposed behind a company domain or reverse proxy.

---

## Attention and independent outcomes

1. Call `anda_brain_get_runtime_status` or `GET /v1/{space_id}/runtime/status`.
   Inspect `configured`, `attention_enabled`, `actions_enabled`, observation
   authentication and `blocked_reasons`. `learning`, `semantic_attention`, `utility`
   and `trust` report separate optional capabilities. Counts can be partial, and
   audit-only fields can be null; neither implies an empty queue or completed work.
2. Read `anda_brain_get_attention` with `{cursor,limit}` or
   `GET /v1/{space_id}/attention?limit=20`. Follow `next_cursor` until `complete`.
   Limits are 1–50, visible output is bounded, and cursors expire after five minutes
   and are bound to caller/instance/configuration. Restart pagination after expiry;
   reading never claims a lease. Keep the returned `id` and native references.
3. Reply through `anda_brain_respond_attention` with `{id,response}`, or
   `POST /v1/{space_id}/attention/{id}/responses` with the response object. Use
   `id`, not the slash-containing `wake_ref`, in the URL. A clarification response
   is `{kind:"clarification",event_key,answer}`. It must come from the intended
   recipient before the committed deadline; a fresh gate still checks permission.
   A self-report uses `{kind:"agent_statement",event_key,statement}` and produces
   attributed Evidence, never an independently graded Outcome.
4. Retry unresolved writes with the same event key/body and fresh authentication.
   Changed content under the same key conflicts. A deadline timeout is not consent;
   unknown delivery requires target reconciliation, not blind resending.

Attention recall is a separate, read-only view: `GET /v1/{space_id}/memory/attention`
with `attention_cursor` (optional) and `limit` (1–50) returns fired Watches
(`watch_fired`) and due Commitments (`commitment_due`) ordered by
`(raised_seq, ref)`, plus a new opaque `attention_cursor`. Keep that cursor after
taking the items and pass it next time; it never expires, a page may stop inside
one commit, and reading changes nothing. An item grants nothing.

`GET /v1/{space_id}/schema/drafts` lists the vocabulary Formation drafted
(`kip://local/draft@0.0.0`). Only the Space's owner, with the management token,
promotes a draft onto an installed symbol: `POST /v1/{space_id}/schema/promote`
with `{kind, from, to}`.

These routes use structured JSON/CBOR POST bodies and JSON/CBOR/Markdown responses.
Runtime credentials are mandatory even on public/local Spaces. Configuration is
installed before loading Spaces via `BRAIN_RUNTIME_CONFIG` or trusted Rust builders.
The compiled `attention_inbox_v1` adapter persists a delivery; it does not prove
that a human read it or that the business task succeeded.

`POST /v1/{space_id}/outcomes` belongs to separately registered, signed observers
with current native `record_outcome` authority and the exact measurement contract.
Ordinary agents do not submit self-grades through it; MCP exposes no observer writer.
Receipts distinguish native persistence, learning eligibility and unresolved safety
work. Registered learning Attempts use their own controller exclusively, including
baselines. Late/corrected evidence is additive and does not rewrite prior grades.

Text/mixed Watches require the installed semantic evaluator; missing, unknown,
timed-out or truncated judgments cannot prove silence. Inspect `semantic_attention`
for configuration/recovery and leave evaluator changes and reviewed re-arming to
operators. A maintenance model completion does not advance coverage.

Utility uses actual delivery receipts plus independently qualified use/contribution.
Trust uses separately verified facts within one exact actor/predicate/context; action
success/failure, usage and correction counters are not source-reliability samples.
Trust application additionally requires current `manage_trust`, calibrated parameters
and an explicit governor. No ordinary token, semantic actor or MCP tool installs
bindings, sets scores or grants governance. Automatic changes default off.

Read only the relevant reference when integrating a host:

- [Runtime setup, identity mapping and recovery](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/RUNTIME.md)
- [Exact endpoint and receipt types](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/API.md#authenticated-runtime-inbox-and-observations)
- [Semantic Watch contracts](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/SEMANTIC_WATCH_RUNTIME.md)
- [Scoped trust governance](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/TRUST_RUNTIME.md)

## OpenClaw Integration

The `anda-brain` OpenClaw plugin (installed from the OpenClaw plugin registry; its source is no longer in this repository) integrates Anda Brain into [OpenClaw](https://openclaw.ai/) agents, providing automatic memory encoding and a `recall_memory` tool — no manual API calls needed.

### Prerequisites: Deploy Anda Brain and Create a Space

Before installing the plugin, you need a running Anda Brain deployment plus a `spaceId` and `spaceToken`:

1. **Deploy Anda Brain** — see the [Quick Start guide](https://github.com/ldclabs/anda-brain/blob/main/deploy/quick_start.md) (binary or Docker, a few minutes).
2. **Create a brain space** via `POST /admin/create_space` — the `space_id` you choose is your `spaceId`.
3. **Create an API key** via `POST /v1/{space_id}/management/add_space_token` — the returned token is your `spaceToken`.

### Install

1. Install the plugin package:
```bash
openclaw plugins install anda-brain
```

2. Update anda-brain configuration in `openclaw.json` with the `spaceId` and `spaceToken` created above:
```json
{
  "plugins": {
    "entries": {
      "anda-brain": {
        "enabled": true,
        "config": {
          "spaceId": "my_space_001",
          "spaceToken": "STxxxxx",
          "baseUrl": "http://localhost:8042" // your Anda Brain deployment URL
        }
      }
    }
  }
}
```

3. Restart OpenClaw Gateway.
```sh
openclaw gateway restart
```

Required fields:

- `spaceId`: your Brain space ID (created via `POST /admin/create_space`)
- `spaceToken`: your space API Key (created via `POST /v1/{space_id}/management/add_space_token`)
- `baseUrl`: your Anda Brain deployment URL (e.g. `http://localhost:8042`) — always set this; the legacy default `https://brain.anda.ai` has been discontinued

### What It Does

| Feature | Mechanism | Description |
|---------|-----------|-------------|
| **Memory encoding** | `agent_end` hook | After each agent turn, conversation messages are automatically sent to `POST /v1/{space_id}/formation` (fire-and-forget). |
| **Memory recall** | `recall_memory` tool | Registered as an agent tool; the LLM can call it with a natural language query to retrieve knowledge via `POST /v1/{space_id}/recall`. |

### Configuration Options

| Option | Type | Required | Default | Description |
|--------|------|----------|---------|-------------|
| `spaceId` | `string` | Yes | — | Memory space ID |
| `spaceToken` | `string` | Yes | — | Space token for API authentication |
| `baseUrl` | `string` | Yes (in practice) | `https://brain.anda.ai` (discontinued) | Your Anda Brain deployment URL — always set this |
| `defaultContext` | `InputContext` | No | — | Default context included with every request (`counterparty`, `agent`, `source`, `topic`) |
| `formationTimeoutMs` | `number` | No | `30000` | Formation request timeout (ms) |
| `recallTimeoutMs` | `number` | No | `120000` | Recall request timeout (ms) — recall may take 10–100s |

### `recall_memory` Tool Parameters

The plugin registers a `recall_memory` tool that the LLM can invoke:

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | `string` | Yes | Natural language question (e.g. "What are Alice's preferences?") |
| `budget` | object/null | No | Optional fixed-codec memory packet and cumulative planning-input limits; Space policy may enforce tighter caps |
| `context.counterparty` | `string` | No | Current user identifier |
| `context.agent` | `string` | No | Calling agent identifier |
| `context.topic` | `string` | No | Topic hint for disambiguation |

---

## Anda Bot

[**Anda Bot**](https://github.com/ldclabs/anda-bot) is a complete, open-source AI agent built on Anda Brain, using Brain as its long-term memory and cognitive backbone. Use it directly, or as a reference implementation for integrating Anda Brain into your own agent.

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `401 Unauthorized` | If auth is enabled (`ED25519_PUBKEYS` set), check Bearer token signature, `aud` (space ID), and required `scope` (`read`/`write`) |
| `404 Not Found` on space endpoints | Verify the `space_id` exists and the token `aud` matches the target space |
| Formation returns but nothing in memory | Formation is async — check space status after a few seconds; look at the conversation status |
| Recall seems empty or insufficient | Memory may not be encoded yet, or the query is too narrow; try broader phrasing and include `context` |
| Maintenance rejected | Only one maintenance cycle can run at a time per space; wait for the current one to finish |
| Empty recall for new space | Expected — a new space has no memory yet; send conversations via Formation first |

---

## Configuration Reference

The service is configured via CLI arguments and environment variables:

| Env Variable | Default | Description |
|--------------|---------|-------------|
| `LISTEN_ADDR` | `127.0.0.1:8042` | Listen address |
| `ED25519_PUBKEYS` | — | Comma-separated Base64-encoded Ed25519 public keys; if empty, API authentication is disabled |
| `MODEL_FAMILY` | `anthropic` | Model family to use for encoding and recall (e.g., `gemini`, `anthropic`, `openai`) |
| `MODEL_API_KEY` | — | API key for the configured model provider |
| `MODEL_API_BASE` | `https://api.deepseek.com/anthropic` | Model API base URL |
| `MODEL_NAME` | `deepseek-v4-pro` | LLM model for agents |
| `HTTPS_PROXY` | — | HTTPS proxy URL |
| `SHARDING_IDX` | `0` | Shard index for this instance |
| `MANAGERS` | — | Comma-separated manager principal IDs |
| `CORS_ORIGINS` | — | CORS allowed origins: empty = disabled, `*` = allow all, or comma-separated origins |
| `MCP_HTTP_ENABLED` | `true` | Mount Streamable HTTP MCP with the HTTP service |
| `MCP_HTTP_PATH_PREFIX` | `/mcp` | Remote MCP prefix; clients connect to `{prefix}/{space_id}` |
| `MCP_HTTP_ALLOWED_HOSTS` | — | Comma-separated Host allowlist for remote MCP |
| `MCP_HTTP_ALLOWED_ORIGINS` | — | Comma-separated browser Origin allowlist for remote MCP |
| `MCP_HTTP_AUTO_CREATE_SPACE` | `false` | Create remote MCP spaces on first use after a valid `write` CWT |
| `MCP_HTTP_AUTO_CREATE_TIER` | `1` | Tier used for remote MCP auto-created spaces |
| `MCP_SPACE_ID` | — | Space exposed by the MCP stdio server |
| `MCP_AUTH_TOKEN` | — | CWT or space token used by MCP tools |
| `MCP_AUTO_CREATE_SPACE` | `false` | Create the MCP space if it does not exist |
| `MCP_AUTO_CREATE_TIER` | `1` | Tier used for MCP auto-created spaces |

**Storage backends:**

| Backend | Command | Key Config |
|---------|---------|------------|
| In-memory (dev) | `cargo run -p anda_brain --features mcp,wiki` | — |
| Local filesystem | `cargo run -p anda_brain --features mcp,wiki -- local` | `LOCAL_DB_PATH` (default `./db`) |
| AWS S3 | `cargo run -p anda_brain --features mcp,wiki -- aws` | `AWS_BUCKET`, `AWS_REGION` |
| MCP HTTP | `cargo run -p anda_brain --features mcp,wiki -- local` then connect `/mcp/{space_id}` | `MCP_HTTP_ALLOWED_HOSTS`, bearer token |
| MCP stdio | `cargo run -p anda_brain --features mcp,wiki -- mcp --space-id my_space_001 local` | `MCP_AUTH_TOKEN`, `LOCAL_DB_PATH` |

Maintenance parameters override the space policy for that run.
`memory_strength_decay_factor` is accepted but ignored: decay is computed at read
time from the pinned strength policy. Inspect `GET /v1/{space_id}/memory_status`:
`last_settlement.correction_scan_incomplete` means the bounded discovery scan
will continue, and `correction_scan_through_seq` identifies fully read progress.
A completed discovery page does not prove model processing or Watch coverage.

For KIP v1 upgrades, stop the old writer, back up and test a copy before cutover.
Migration is per-space and one-way. It preserves source records, conservatively
maps native fields/lifecycle/time, and leaves unsupported learning/runtime state
as Legacy records. Old-id usage and derived caches reset once; conversations,
policies, tokens and wiki records remain.

## Completion and experiment boundaries

Formation/maintenance acceptance and high-water marks do not prove that a
particular conversation completed. Retain the returned conversation ID and
inspect that record; Rust hosts can use `Space::wait_for_processing`. Timeout
requires reconciliation of the same ID, not another submission.

A `/probe` miss is retrieval information, not rejected belief. Optional
`search_exhaustive` is search-window coverage; omission means unknown.
The Rust `experiments` feature is a trusted host interface for isolated runs,
without a production HTTP/MCP clock override. The sibling Anda Bot optional
`mib` feature supplies separate loopback Agent and memory-backend protocols over
this host interface. Its run/request IDs, terminal waits, memory controls and
cost receipts are evaluation contracts, not production learning capability. Notes now persist
in the Space's `engine/` storage namespace; experiment session boundaries retain
or discard them according to the explicit memory condition.


The optional Rust `learning` feature adds `Space::learning()` for trusted host
registration, persistent paired-trial dispatch and authenticated observer
measurements. This is not an HTTP/MCP operation. Model messages cannot supply
observer authority, lift a revision to executable, or mark a Skill adopted.
`ReadyForEvaluation` only means the fixed cohort has terminal records. The host
calls `settle` after its predeclared cutoff to atomically commit a replayed native
verdict and standing. `reviews`/`enroll_review` retain acquisition evidence in new
monitoring trials; independent safety signals can revoke without a sample quota.
Recall's internal `check_procedure_status` tool checks the exact revision,
current policy/dependencies, review expiry and short-lived host application
context. A failed/unavailable check cannot support a validated recommendation.
Recall is not actual use, and recommendation/adoption never grant execution
permission. Production executor, observer and current-context bindings remain
explicit host responsibilities; see [public contract](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/README.md#native-learning-contracts).
Configured learning Spaces cannot be forked with their operational journal;
prepare the immutable factual baseline before configuring the controller.

Isolated experiment hosts can use `Experiment::create_with_recall_budget` to
force Recall-budget limits and `audit_procedures()` for evaluator-only native inventories.
The sibling Bot's `learning_audit` extension must never be fed back through
Observe or used as an execution permit. Incomplete audits cannot prove absence.
MIB requires explicit normal/no-memory/ungated capability declarations; Bot's
current persistent mode is not a configured learning condition. See
[public contract](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/README.md#mib-integration) for the remaining host bindings.

The offline Rust `anda_brain::eval` API and `eval` CLI, optimizer and miner are
retired. Use MIB's public product regression profile and normal submission
protocol. Online diagnostics, self-test, shadow reports, probes and ledgers are
retained; none substitutes for native learning evidence. Runtime policies are
per-Space. Trusted Rust hosts may configure immutable `AgentPrompts` before
sharing an AppState or opening a Space; only section A is replaceable and the
compiled KIP reference stays intact. This is not a model-facing prompt tool.
See [public contract](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/README.md#offline-regression-and-instance-configuration) for the migration and removed APIs.

The learning runtime provides explicitly configured background trial/review advancement and terminal
archival. Read `learning` in the runtime status to distinguish compiled, registered,
bound and automatic states. A host installs the registered workflow executor,
independent observer, frozen plan factory and reviewed calibration through
`BRAIN_RUNTIME_CONFIG`; ordinary agents cannot authorize these through memory text.
Do not describe a scheduler count, archival receipt or compiled adapter as measured
learning improvement. See [the learning runtime guide](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/LEARNING_RUNTIME.md).

Structured Recall includes an optional `recall_receipt` handle. Treat it as
delivery provenance, not proof of actual use, truth or permission. Trusted host
decisions select actual `used_refs`; only qualified independent contribution
witnesses or native frozen paired comparisons can support utility calibration.
Frequency, citations, likes and model self-reports never earn objective credit.
Configured utility can only order peers inside existing Recall priorities; it
cannot override constraints, uncertainty or revoked procedure status. See the
[utility guide](https://github.com/ldclabs/anda-brain/blob/main/anda_brain/UTILITY_RUNTIME.md).

## Host-managed memory changes

Trusted Rust product management is separate from the model tool surface. Do not invent source identities, operation approvals or caller authority from a prompt. A stopped source must not be reintroduced through processor history, historical KIP reads or copied Notes. After a managed change, automatic KIP reads reject historical/inactive queries and stale processing contexts must be rebuilt. Restart pagination from the first page if its cursor predates the change; newer cursors remain usable. Budgeted Recall also excludes old processing history. Natural-language corrections submit new information; they are not confirmation of a reviewed native correction or deletion. Ordinary memory requires neither inbox configuration nor MIB.
