# 🧠 Anda Brain — Long-Term Graph Memory for AI Agents

> Burn electricity to train large models, and you get a neural network ontology; burn tokens to train a memory graph, and you get a symbolic network ontology.
>
> Combine the two, and you get **Neural-Symbolic AI** — and Brain is the cognitive organ that keeps the symbolic network growing.

**[English](./README.md) | [中文](./README_cn.md)**

Anda Brain is a self-hosted memory service for LLM agents. Agents send it what they
observed and ask it questions in natural language; Brain turns those interactions
into a versioned knowledge graph — the **Cognitive Nexus**, stored in
[AndaDB](https://github.com/ldclabs/anda-db) — and keeps that graph healthy with
background "sleep" cycles. Internally everything is expressed in
[KIP 2.0](https://github.com/ldclabs/KIP) (Knowledge Interaction Protocol), but
business agents never have to write KIP.

- **Formation** encodes conversations into structured memory.
- **Recall** answers natural-language questions from that memory.
- **Maintenance** consolidates, reviews and retires memory while the agent is idle.

Current release: **0.13.4**, tracking KIP `11a82ec` and the Cognitive Memory Profile
`kip://profiles/cognitive-memory@2.0.0`. See the [CHANGELOG](./CHANGELOG.md).

---

## Why another memory system?

Your assistant remembers every word you've said. Then you ask it for a restaurant
and it cheerfully suggests a Brazilian steakhouse — even though you told it last
month that you became vegetarian.

That is not a retrieval failure. It retrieved "I love BBQ" from two years ago *and*
"I'm vegetarian now" from last month. It just had no way to know that one replaced
the other: in its store they were two equal points with no timeline, no source and
no relationship.

| Approach | What goes wrong |
| :--- | :--- |
| **Vector RAG** | Fragments are independent points. Nothing says two of them describe the same preference of the same person, or that one ended the other. |
| **Markdown memos** | Every clean-up re-reads the whole file. The longer it grows, the more each pass costs and the less accurate it becomes. |
| **Key-value stores** | `alice.diet = "omnivore"` overwrites `"vegetarian"`; the history is gone. |
| **Graph databases + LLM-written queries** | The right structure, but asking a model to write Cypher against a rigid schema is error-prone and hard to integrate. |

The operations memory actually needs — merging fragments about one topic, noticing
that something changed, keeping a timeline, weighing evidence — are operations on a
network. Anda Brain keeps memory as a graph and gives the work of maintaining it to
dedicated agents, so your business agents don't have to.

---

## How it works

### Three layers

```
┌──────────────────────────────────────────┐
│ Support agent · Sales agent · Dev agent  │  ← Business agents
│   natural language, REST or MCP          │    no graph or KIP knowledge needed
└────────────────┬─────────────────────────┘
                 │ Formation / Recall / Memory Interface
                 ▼
┌──────────────────────────────────────────┐
│               Anda Brain                 │  ← Formation · Recall · Maintenance agents
│   the only layer that speaks KIP         │    plus host gates, settlement, scheduling
└────────────────┬─────────────────────────┘
                 │ KIP 2.0 (KQL / KML / META)
                 ▼
┌──────────────────────────────────────────┐
│   Cognitive Nexus on AndaDB              │  ← Persistent, versioned, auditable graph
│   Concepts · Propositions · Assertions   │    one database per memory Space
│   Evidence · Activities                  │
└──────────────────────────────────────────┘
```

Each **Space** is an isolated memory: its own database, graph, conversation history,
tokens and policy. One Brain instance serves many Spaces, and many agents can share
one Space — what the support agent learns, the sales agent can recall.

### Three agents

| Agent | What it does | Brain analogy |
| :--- | :--- | :--- |
| **Formation** | Reads a conversation, grounds it against existing memory and writes what is worth keeping: Evidence, claims, events, experiences, commitments. Runs asynchronously and processes a Space's queue in order. | Encoding new experiences |
| **Recall** | Plans read-only graph queries for a question, follows relationships across hops, and answers with what memory actually supports — including "contested" and "insufficient". | Remembering |
| **Maintenance** | Consolidates events into knowledge, reviews identity and contradictions, re-checks what depended on a revised claim, reviews retention, commitments and new vocabulary. | Sleep |

### What memory looks like (KIP 2.0)

KIP 2.0 separates meaning, belief, evidence and provenance. The rule everything else
follows from: **a statement existing is not the statement being true.**

| Element | What it is |
| :--- | :--- |
| **Concept** | A referable thing: a person, a project, an option, an event. |
| **Proposition** | A truth-neutral `(subject, predicate, object)` statement. |
| **Assertion** | One actor's stance on a Proposition: who said it, how confidently, from when, citing what. |
| **Evidence** | What was observed: a message, a tool result, a document passage. |
| **Activity** | How something came to be: a formation pass, a consolidation, a revision. |

What is *currently believed* is projected from Assertions at read time, not stored.
That gives Brain a few properties that are hard to get any other way:

- **Disagreement coexists.** "Alice says X, Bob says not-X" stays two Assertions;
  nothing silently picks a winner.
- **Three kinds of change, three histories.**
  - *The world changed* ("I'm vegetarian now"): one new Assertion from the time of the
    change. Temporal succession ends the old value, which still answers questions
    about its own time.
  - *A claim was wrong*: a new Assertion supersedes the old one by the same actor.
  - *Brain misrecorded something*: recording repair invalidates the bad extraction;
    nobody's stance changes.
- **Every claim is traceable** to the actor, the Evidence and the observation time.
  A claim's `asserted_at` is when its source was observed, not when Formation ran.
- **Forgetting is about accessibility, never truth.** Memory strength decays with
  disuse and is computed at read time; an Assertion's confidence never decays. A fact
  nobody asked about for a month is no less true.
- **Reading never reinforces.** Recall is read-only; only an explicit signal (a
  decision that used a memory, a correction) changes strength.
- **"I don't know" is not "no".** An insufficient basis is reported as insufficient.
- **New vocabulary is a draft.** When Formation needs a type or predicate the Profile
  lacks, it drafts it into the Space's own draft package. The host checks the name,
  caps each Space at 512 symbols, and queues a review; only the Space owner can
  promote a draft onto an installed symbol.

### Sleep: the maintenance cycle

Maintenance runs at three depths:

| Scope | Runs | Work |
| :--- | :--- | :--- |
| `daydream` | every 21 formed conversations, or on demand (default) | Salience scoring and micro-consolidation of recent material. |
| `quick` | every 42 formed conversations | Assessment plus urgent SleepTasks. |
| `full` | every 168 formed conversations, and at least once every 24 hours for a Space that has formed anything | Everything, including the predicate census and retention expiry. |

Each cycle starts with a deterministic **settlement** — the host, not the model,
records new corrections, walks what depended on revised claims, raises due
Commitments, archives what passed its retention date and hands the model a factual
`assessment`. The Maintenance agent then does the cognitive work:

- **Consolidation**: clusters of events and experiences become derived claims with
  lineage back to their sources ("mentioned salmon, sea urchin and sushi in three
  conversations" → "prefers Japanese cuisine").
- **Identity**: suspected duplicates are reviewed and merged non-destructively.
- **Revision**: claims that depended on a corrected premise are flagged stale.
- **Procedures**: repeated successes and failures can be compiled into a Skill
  candidate, which stays *unproven* until an independent trial pipeline says otherwise.
- **Commitments, Watches, retention and new vocabulary** are reviewed.
- **Self-test**: after a cycle, Brain probes recent memories it has never been asked
  about and queues re-encoding for the ones search can't find.

Maintenance never purges memory, never defines vocabulary and never grades a Skill;
those belong to explicit, authorized paths.

---

## Features

**Integration**

- **REST API** with JSON, CBOR or Markdown request/response bodies.
- **KIP Memory Interface** (`memory_basic`): one endpoint, five intents — `observe`,
  `recall`, `revise`, `feedback`, `forget` — over staged sources, with idempotent
  receipts, `after` barriers and scoped recall briefings.
- **MCP server** over Streamable HTTP (`/mcp/{space_id}`) and stdio, with the same
  tools for memory, attention and wiki.
- **[SKILL.md](./skills/anda-brain/SKILL.md)** for agent frameworks, also served by
  every deployment at `/SKILL.md`.
- **[anda-cli](./anda-cli/README.md)**, a command-line client for keys, tokens,
  formation (including batch file import), recall and the Memory Interface.

**Memory**

- Natural-language Formation and Recall; structured Recall with citations and a
  `found` flag; optional hard token **budgets** for Recall packets.
- Model-free **probe** ("do I know anything about this?") with a negative cache.
- **Attention recall**: fired Watches and due Commitments, ordered and cursor-paged.
- **Pinning**, explicit **forgetting** with dry runs, and governed erasure plans.
- Read-only **KIP endpoint** for advanced inspection.
- **Memory observability**: usage, probe, self-test and correction counters, graph
  counts and the latest settlement report.

**Operations**

- Multi-Space isolation, Ed25519-signed CWT authentication, per-Space tokens
  (`read` / `write` / `*`, optional wiki label restrictions), managers and tiers.
- Storage on the local filesystem, AWS S3, or in memory for development.
- Per-Space `MemoryPolicy` and per-Space BYOK model configuration.
- Request shedding, a process-wide model concurrency cap, background flushing and
  idle-Space eviction, graceful shutdown.
- Automatic upgrade of Spaces written by KIP 1.x builds.

**Optional** (compile-time features or explicit runtime bindings — off by default)

- **Wiki**: versioned Markdown reference documents with ACL-scoped reads, verifiable
  citations, OKF import/export and optional graph extraction (WikiDigest).
- **Runtime API**: authenticated attention inbox, responses and independent outcomes;
  structured Watch scheduling across Spaces; action callbacks with fenced dispatch.
- **Semantic Watches**, **trusted learning trials**, **memory utility** and
  **contextual source trust** runtimes, each with its own guide and disabled-by-default
  configuration.
- **Experiments**: isolated host runs, snapshots and business time for evaluation.

---

## How it compares

| Capability | Vector RAG | Markdown memos | KV store | Graph DB + LLM queries | **Anda Brain** |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Structure** | Chunks | Semi-structured text | Fixed fields | Fixed graph schema | **Graph with drafted, reviewable vocabulary** |
| **Integration** | Simple | Simple | Simple | Heavy | **Natural language, REST or MCP** |
| **Digestion** | None | Full re-read per pass | Overwrite | Rarely automated | **Scheduled consolidation cycles** |
| **Change over time** | Both versions coexist, unordered | Up to the model | Old value lost | Custom logic | **Temporal succession; history kept** |
| **Wrong claims** | Stay | Edited in place | Overwritten | Edited in place | **Superseded by a new Assertion** |
| **Disagreement** | Indistinguishable | Up to the model | Last write wins | Custom logic | **Per-actor Assertions coexist** |
| **Provenance** | Source chunk at best | None | None | Depends | **Actor, Evidence and time on every claim** |
| **Multi-hop questions** | Weak | Weak | None | Good | **Graph traversal by the Recall agent** |

---

## Quick start

> The hosted service (`brain.anda.ai`) has been discontinued. Anda Brain is
> [open source](https://github.com/ldclabs/anda-brain) and meant to be self-hosted.
> The step-by-step guide is [deploy/quick_start.md](./deploy/quick_start.md).

### 1. Run the service

Download `anda_brain` from [Releases](https://github.com/ldclabs/anda-brain/releases),
use the Docker image, or build it:

```bash
cargo build -p anda_brain --release --features mcp,wiki
```

```bash
docker pull ghcr.io/ldclabs/anda_brain_amd64:latest
```

Pick a storage backend:

```bash
# In memory — everything is lost on exit; for trying things out
./anda_brain

# Local filesystem
./anda_brain local --db ./data

# AWS S3 (credentials from the standard AWS_* variables)
./anda_brain aws --bucket my-bucket --region us-east-1
```

### 2. Configure a model and authentication

Settings come from flags, environment variables or a `.env` file:

```bash
MODEL_FAMILY='anthropic'            # anthropic | openai | gemini | …
MODEL_API_BASE='https://api.deepseek.com/anthropic'
MODEL_NAME='deepseek-v4-pro'
MODEL_API_KEY='…'
ED25519_PUBKEYS='…'                 # empty = authentication disabled (development only)
MANAGERS='…'                        # principals allowed to create Spaces
```

The defaults target DeepSeek's Anthropic-compatible endpoint. On a Claude model, set
`MODEL_MAX_OUTPUT` to 128000 or less. Every option is listed in the
[technical documentation](./anda_brain/README.md#configuration).

### 3. Create a Space and a token

With [anda-cli](./anda-cli/README.md) and a manager CWT:

```bash
# 1. Generate an Ed25519 keypair and put the public key in ED25519_PUBKEYS / MANAGERS
anda-cli keygen --json > keys.json

# 2. Mint a wildcard management CWT signed with the private key
PRIVKEY=$(jq -r .private_key keys.json)
MANAGER_CWT=$(anda-cli cwt --key "$PRIVKEY" --subject "$OWNER" --audience '*' --scope '*')

# 3. Create a Space and mint an agent token
anda-cli --token "$MANAGER_CWT" admin create-space --user "$OWNER" --space-id my_space --tier 2
anda-cli --token "$MANAGER_CWT" --space-id my_space management add-token --scope '*' --name support_bot
```

Scopes don't nest: a `*` token passes every endpoint, while `read` and `write` tokens
pass only endpoints that require exactly that scope. An agent that both writes and
recalls a private Space needs `*` (minting one takes a `*`-scoped CWT) or one token
of each. The same operations are `POST /admin/create_space` and
`POST /v1/{space_id}/management/add_space_token`. The tier caps the graph:
Formation refuses new input once a tier-`n` Space holds more than 10^(n+2) Concepts
or conversations (tier 0: 100, tier 2: 10,000).

### 4. Write and read memory

Send a conversation to Formation (returns immediately; encoding runs in the
background):

```bash
curl -sX POST https://brain.example.com/v1/my_space/formation \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [
      {"role": "user", "content": "I work at Acme Corp as a senior engineer."},
      {"role": "assistant", "content": "Noted: senior engineer at Acme Corp."}
    ],
    "context": {"counterparty": "user_123", "agent": "onboarding_bot"},
    "timestamp": "2026-03-09T10:30:00.000Z"
  }'
```

Ask Recall before answering the user:

```bash
curl -sX POST https://brain.example.com/v1/my_space/recall \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "Where does this user work?", "context": {"counterparty": "user_123"}}'
```

Agents that need receipts, barriers and scoped briefings use the Memory Interface:
stage what was observed, then send one intent per request.

```bash
# Stage a source → {"result": {"source_ref": "src-…", …}}
curl -sX POST https://brain.example.com/v1/my_space/memory/sources \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"messages": [{"role": "user", "content": "I moved to Berlin last week."}],
       "observed_at": "2026-09-02T08:00:00.000Z", "idempotency_key": "chat-42:msg-7"}'

# Observe it → a receipt that moves from "recorded" to "available"
curl -sX POST https://brain.example.com/v1/my_space/memory \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"kip_memory": "2.0", "operation": "observe", "idempotency_key": "observe:chat-42:msg-7",
       "input": {"source_ref": "src-…"}}'

# Recall, waiting for that receipt first
curl -sX POST https://brain.example.com/v1/my_space/memory \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"kip_memory": "2.0", "operation": "recall",
       "input": {"query": "Where does the user live now?", "after": ["rcpt-…"]}}'
```

### 5. Or connect over MCP

The HTTP service mounts a Streamable HTTP MCP endpoint at
`https://brain.example.com/mcp/{space_id}`; send the same token as
`Authorization: Bearer …`. For a local client, run Brain as a stdio server:

```bash
MCP_AUTH_TOKEN="$SPACE_TOKEN" ./anda_brain mcp --space-id my_space local --db ./data
```

Tools include `anda_brain_memory`, `anda_brain_stage_memory_source`,
`anda_brain_remember_conversation`, `anda_brain_recall_memory` and
`anda_brain_wiki_search`. The full list is in the
[technical documentation](./anda_brain/README.md#mcp-server).

---

## Two ways to deploy

| | **Rust service** (`anda_brain`) | **Cloudflare Worker** ([`anda-brain-worker`](./anda-brain-worker/README.md)) |
| :--- | :--- | :--- |
| Engine | Cognitive Nexus on AndaDB | `@ldclabs/kip-do`, an independent KIP 2.0 engine |
| Storage | Local filesystem, S3 or memory | One SQLite Durable Object per Space |
| Models | Any configured provider, per-Space BYOK | Workers AI |
| Formation / Recall / Maintenance | ✓ (Formation queued in the background) | ✓ (Formation runs inside the request) |
| Memory Interface (`memory_basic`), attention recall | ✓ | ✓ |
| Scheduled maintenance | Built in | Call it yourself or from a Cron Trigger |
| MCP, wiki, budgeted Recall, CBOR/Markdown, scoped tokens | ✓ | — |
| Atomic batches, retention-expiry sweep | ✓ | — |
| Runtime API, learning, utility, trust runtimes | Optional | — |

The Worker targets small agents and edge deployments; its README is the authority on
what it does and does not build.

---

## Use cases

**Personal agents.** Local agents outgrow Markdown files and SQLite once memory spans
years of relationships, preferences and projects. [Anda Bot](https://github.com/ldclabs/anda-bot)
is an open-source agent built on Anda Brain as its long-term memory.

**Enterprise "corporate brains."** A sales agent records "the customer needs 5,000
units before Q3"; the procurement agent later recalls that the supplier of a core
material was late three times in six months, from whom, and on what evidence.
Customer-service history, decision rationales and lessons from failures accumulate in
one Space that every agent — and every newly connected agent — can query. Deploy it
on-premises to keep that memory under your control.

---

## Two kinds of training

| | Large-model training | Memory-graph training |
| :--- | :--- | :--- |
| **Spends** | Electricity (compute) | Tokens (inference) |
| **Learns from** | Public corpora | Your dialogues and events |
| **Produces** | Neural network ontology (weights) | Symbolic network ontology (graph) |
| **Gives AI** | General reasoning | Identity, experience and facts |
| **Character** | Probabilistic, black-box, general | Deterministic, white-box, personal |

Models are interchangeable; the memory is not. Switch from one model provider to
another and the graph — your agents' accumulated experience — stays.

### Why "Brain"?

Because it behaves like one: it **encodes** experience during the day,
**consolidates** it while idle, and **recalls** from a better-organized structure
afterwards. The longer that loop runs, the more an agent knows about its world.

**It's time to let your AI sleep.**

---

## Documentation

| Document | Contents |
| :--- | :--- |
| [anda_brain/README.md](./anda_brain/README.md) | Technical reference: agents, endpoints, MCP tools, configuration, features, lifecycle |
| [API.md](./anda_brain/API.md) · [API_cn.md](./anda_brain/API_cn.md) | Full HTTP and MCP API with TypeScript types |
| [SKILL.md](./skills/anda-brain/SKILL.md) | Integration instructions for agents |
| [deploy/quick_start.md](./deploy/quick_start.md) · [中文](./deploy/quick_start_cn.md) | From zero to a working Space |
| [RUNTIME.md](./anda_brain/RUNTIME.md) · [中文](./anda_brain/RUNTIME_cn.md) | Runtime API, Watch scheduling, action callbacks, recovery, execution limits |
| [Semantic Watch](./anda_brain/SEMANTIC_WATCH_RUNTIME.md) · [Learning](./anda_brain/LEARNING_RUNTIME.md) · [Utility](./anda_brain/UTILITY_RUNTIME.md) · [Trust](./anda_brain/TRUST_RUNTIME.md) | Optional runtime guides (each has a `_cn.md` edition) |
| [anda-cli/README.md](./anda-cli/README.md) | Command-line client |
| [anda-brain-worker/README.md](./anda-brain-worker/README.md) | Cloudflare Worker edition |
| [anda_brain/conformance](./anda_brain/conformance/README.md) | KIP conformance harness adapter |
| [tools/migrate-draft-space](./tools/migrate-draft-space/README.md) | Moving a KIP 2.1.0-draft Space onto a fresh 2.0.0 Nexus |
| [CHANGELOG.md](./CHANGELOG.md) | Release notes and known limits |

## Repository layout

```
anda_brain/              Rust library and service binary
  src/agents/            Formation, Recall and Maintenance agents
  src/memory_interface/  KIP Memory Interface (memory_basic)
  src/space*.rs, space/  Space lifecycle, settlement, self-test, attention
  src/wiki/              Versioned wiki (feature "wiki")
  assets/                Agent prompts and generated KIP reference
anda-brain-worker/       Cloudflare Worker edition
anda-cli/                Go command-line client
skills/anda-brain/       Packaged agent skill
deploy/                  Quick start and systemd unit
tools/migrate-draft-space/  Standalone 2.1.0-draft → 2.0.0 migration
posts/                   Essays on the design
```

## Development

```bash
cargo fmt --check
cargo clippy -p anda_brain --all-targets --all-features -- -D warnings
RUST_MIN_STACK=16777216 cargo test -p anda_brain --all-features
CI=true pnpm --filter @ldclabs/anda-brain-worker check
```

Tests use local storage and never call a model provider. See
[AGENTS.md](./AGENTS.md) for the repository's conventions and protocol invariants.

## Status and limits

- **KIP versions.** KIP 1.x Spaces upgrade automatically on first open (back up
  first; the upgrade is one-way). Spaces activated under the 2.1.0 draft are not
  migrated automatically; use [`tools/migrate-draft-space`](./tools/migrate-draft-space/README.md).
- **One writer per Space.** Several locks are in-process; never point two instances
  at the same Space's storage.
- **Scale ceiling.** Correction discovery and self-test sampling use full scans that
  the engine caps at 65,536 solutions; past that they report an error instead of
  completing.
- **No learned-improvement claims.** Skill candidates stay unproven unless a trusted,
  independently observed trial pipeline is configured. Learning, utility and trust
  runtimes ship disabled; their mechanism tests are not evidence of real-world gains.

## Further reading

- [AI Memory Must Sleep — And Only Knowledge Graphs Can Make That Happen](./posts/AI_Memory_Must_Sleep.md)
- [A Deep Dive into Claude Code's Memory System: How Does AI "Remember" You?](./posts/Claude_Code_Memory_Research.md)
- [When AI Learns Ontology Modeling: Anda Brain Lets Enterprises "Grow" Their Own Intelligent Brains](./posts/Enterprise_AI_Brain.md)
- [The Second Training of AI: Forging Memory Graphs with Tokens](./posts/Tokens_Anda_Brain.md)
- [Building a Company as an Intelligence Requires a "Brain"](./posts/Company_Built_As_Intelligence.md)
- [From "Compiling Knowledge" to "Forging the Brain" — Anda Brain Responds to Karpathy's "LLM Knowledge Bases"](./posts/LLM_Knowledge_Bases.md)

## License

Copyright © LDC Labs. Licensed under the Apache License, Version 2.0.
