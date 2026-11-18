# Anda CLI

A command-line tool for interacting with the [Anda Brain](https://brain.anda.ai) memory service API.

## Install

```bash
go install github.com/ldclabs/anda-brain/anda-cli@latest
```

Or build from source:

```bash
cd anda-cli
go build -o anda-cli .
```

## Configuration

Configuration can be provided via flags or environment variables:

| Flag         | Env Variable    | Description                                       | Default                 |
| ------------ | --------------- | ------------------------------------------------- | ----------------------- |
| `--base-url` | `ANDA_BASE_URL` | API base URL                                      | `http://127.0.0.1:8042` |
| `--space-id` | `ANDA_SPACE_ID` | Space ID                                          |                         |
| `--token`    | `ANDA_TOKEN`    | Auth token                                        |                         |
| `--shard`    | `ANDA_SHARD`    | Shard index (`Shard-Id` header) for sharded setup | `0`                     |
| `--timeout`  | `ANDA_TIMEOUT`  | HTTP request timeout in seconds                   | `120`                   |

`--timeout` must be positive and `--shard` non-negative. Space commands require
`--space-id` or `ANDA_SPACE_ID`; `help`, shell completion, `keygen`, `cwt`, `status`
and `admin` do not. Secret environment values are resolved when the command runs and
are never displayed as help defaults; explicit flags take precedence.

At startup the CLI also loads a `.env` file from the current directory. It never
overrides a variable already set in the environment, but it can set any one that is
missing, including `ANDA_BASE_URL`: a `.env` in an untrusted directory could point
the CLI, and the `ANDA_TOKEN` from your shell, at another server. Set the variables
you rely on explicitly, or run the CLI from a directory you trust.

Commands exit nonzero for HTTP/RPC errors and reported execution failures, including
Recall `failed_reason`, per-entity forget errors, WikiDigest `failed` documents, a
failed Memory Interface intent and a `revoke-token` that matched no token. The JSON
result remains on stdout when it contains a business failure; diagnostics go to
stderr. A successful Recall with `found: false` still exits zero. RPC results are
printed as the server sent them, including members this CLI does not model yet, and
JSON numbers in KIP parameters/results, tool content and metadata retain their
integer precision. HTTP errors show the server's message and its structured `data`.

**CWT command flags:**

| Flag         | Env Variable        | Description                                                         | Default |
| ------------ | ------------------- | ------------------------------------------------------------------- | ------- |
| `--key`      | `ANDA_CWT_KEY`      | Ed25519 private key (base64/base64url CBOR, or file path / `@file`) |         |
| `--subject`  | `ANDA_CWT_SUBJECT`  | Subject claim - user/principal ID                                   |         |
| `--audience` | `ANDA_CWT_AUDIENCE` | Audience claim - space ID or `*`                                    |         |
| `--scope`    | `ANDA_CWT_SCOPE`    | Scope claim: read, write or `*`                                     | `read`  |
| `--issuer`   | `ANDA_CWT_ISSUER`   | Issuer claim                                                        |         |

## Commands

### Key Generation & CWT

```bash
# Generate a new Ed25519 key pair (base64url-encoded COSE Key)
anda-cli keygen
anda-cli keygen --json

# Create a CWT (CBOR Web Token) signed with Ed25519 private key
anda-cli cwt --key <base64url_private_key> --subject <user_id> --audience <space_id> --scope write

# Create a CWT using a key stored in a file (base64 or base64url content)
anda-cli cwt --key ./private_key.txt --subject <user_id> --audience <space_id> --scope write
anda-cli cwt --key @./private_key.txt --subject <user_id> --audience <space_id> --scope write

# Create a CWT with wildcard audience and 2-hour expiration
anda-cli cwt --key <base64url_private_key> --subject <user_id> --audience "*" --scope "*" --expiration 7200
```

### Service

```bash
# Get service information (name, version, sharding)
anda-cli status
```

### Memory Operations

```bash
# Submit memory formation
anda-cli --space-id my_space --token $TOKEN formation \
  --messages '[{"role":"user","content":"Hello"},{"role":"assistant","content":"Hi!"}]'

# Submit memory formation with plain text (--messages)
anda-cli --space-id my_space --token $TOKEN formation \
  --messages 'Hello, this is a plain text memory.'

# Submit memory formation from file (JSON or plain text)
anda-cli --space-id my_space --token $TOKEN formation \
  --file ./message.txt

# Say when the conversation happened; formed claims use it as asserted_at
# (omit it and the server uses its receipt time)
anda-cli --space-id my_space --token $TOKEN formation \
  --file ./message.txt --timestamp 2026-09-22T08:15:00+08:00

# Or pipe from stdin
echo '[{"role":"user","content":"Hello"}]' | \
  anda-cli --space-id my_space --token $TOKEN formation

# Or pipe plain text from stdin
echo 'Hello from stdin plain text' | \
  anda-cli --space-id my_space --token $TOKEN formation

# Batch submit files by exact filename (recursive).
# Hidden entries (dot-prefixed, e.g. .git) and the checklist file are skipped.
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs \
  --batch-file-name Skill.md

# Batch submit files by extension (recursive)
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs \
  --batch-ext .md

# Resume pending/changed files and retry previously failed submissions
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs \
  --batch-ext .md \
  --batch-retry-failed

# Use a custom checklist path
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs \
  --batch-file-name Skill.md \
  --batch-report ./tmp/formation-batch-checklist.json

# Dry run: scan and print eligible files without submissions or checklist writes
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs \
  --batch-ext .md \
  --batch-dry-run

# Explicitly resubmit all matched files, including unchanged submissions
anda-cli --space-id my_space --token $TOKEN formation \
  --batch-dir ./docs --batch-ext .md --batch-force

# Recall memory
anda-cli --space-id my_space --token $TOKEN recall "What are the user's preferences?"

# Recall with context
anda-cli --space-id my_space --token $TOKEN recall \
  --context-counterparty u1 "What happened in the last meeting?"

# Return structured provenance, or opt into a bounded memory packet
anda-cli --space-id my_space --token $TOKEN recall --structured "What happened?"
anda-cli --space-id my_space --token $TOKEN recall --budget \
  --budget-max-tokens 2048 --budget-context-tokens 16000 "What should I know?"

# Model-free retrieval check and memory observability
anda-cli --space-id my_space --token $TOKEN probe --limit 8 "project name"
anda-cli --space-id my_space --token $TOKEN memory-status

# Pin/unpin an entity, then inspect a deletion before applying it
anda-cli --space-id my_space --token $TOKEN memory pin C-7
anda-cli --space-id my_space --token $TOKEN memory pin --pinned=false C-7
anda-cli --space-id my_space --token $TOKEN memory forget --dry-run C-7
anda-cli --space-id my_space --token $TOKEN memory forget C-7

# Memory Interface (KIP 2.0, memory_basic): stage what you observed, then send
# one intent at a time. --session keeps outstanding receipts and the attention
# cursor and task/context scope; recall adds outstanding receipts to `after` automatically.
# A receipt leaves the session once a recall reports it available, or failed: a
# failed receipt never becomes available, and that recall's briefing explains it.
# Repeating an --after receipt uses one slot. A session with 128 outstanding
# receipts needs a recall before another mutation can be submitted with that session.
# --dry-run is refused with --mode: the Memory Interface has no erasure preview.
# Stage from --text, --file or stdin. For ordered sources supply --stream
# and --event together. --ordinal and predecessor --after belong to that stream.
anda-cli --space-id my_space --token $TOKEN memory sources stage \
  --file chat.json --observed-at 2026-09-02T08:00:00.000Z --key chat-42:msg-7
anda-cli --space-id my_space --token $TOKEN memory observe --source src-… \
  --task relocation --session ./brain-session.json
anda-cli --space-id my_space --token $TOKEN memory recall "Where does the user live now?" \
  --mode action --task relocation --session ./brain-session.json
# Poll attention (due Commitments, fired Watches) from the kept cursor
anda-cli --space-id my_space --token $TOKEN memory recall --mode attention --session ./brain-session.json
# Expand a result's retained basis and element versions
anda-cli --space-id my_space --token $TOKEN memory recall --target basis-… --detail evidence
# Revise with the right history: correction | world_change | misrecorded | unspecified
anda-cli --space-id my_space --token $TOKEN memory revise --source src-… --kind misrecorded --target A-12
anda-cli --space-id my_space --token $TOKEN memory feedback --source src-…
# Governed forgetting (semantic needs the owner's CWT); read the plan it reports
anda-cli --space-id my_space --token $CWT memory forget --mode semantic A-12
anda-cli --space-id my_space --token $TOKEN memory receipt rcpt-…
anda-cli --space-id my_space --token $TOKEN memory plan plan-…

# Trigger maintenance
anda-cli --space-id my_space --token $TOKEN maintenance
anda-cli --space-id my_space --token $TOKEN maintenance --trigger on_demand --scope full
anda-cli --space-id my_space --token $TOKEN maintenance \
  --unconsolidated-max-backlog 50

# Execute a single read-only KIP command (a bare command or {"command": ...})
anda-cli --space-id my_space --token $TOKEN execute-kip-readonly \
  --request 'DESCRIBE PRIMER'
anda-cli --space-id my_space --token $TOKEN execute-kip-readonly \
  --request '{"command":"DESCRIBE PRIMER"}'

# Execute a KIP 2.0 batch (HTTP uses operations; MCP tools use commands)
anda-cli --space-id my_space --token $TOKEN execute-kip-readonly \
  --request '{"operations":["DESCRIBE PRIMER","DESCRIBE SCHEMA ENVIRONMENT"],"execution":{"mode":"independent"}}'

# Execute read-only KIP request from file
anda-cli --space-id my_space --token $TOKEN execute-kip-readonly --file ./kip_request.json

# Execute read-only KIP request from stdin
cat ./kip_request.json | anda-cli --space-id my_space --token $TOKEN execute-kip-readonly
```

Batch Formation visits regular files in deterministic directory order. The checklist
binds its root and selector to the API endpoint, Space ID and shard. Use a separate
`--batch-report` when changing that target. Historical checklists with attempted
submissions but no recorded target are refused; retain them for reference and choose
a new report instead of assuming their files reached the current Space. Unless
`--context-source` is given, each file is submitted with its path relative to
`--batch-dir` as its source, the same key the checklist uses.

Unchanged submitted files are skipped; SHA-256 content changes become new submissions.
Changing context alone requires `--batch-force`, which deliberately resubmits every
matched file and can create duplicate conversations. `submitted` means the server
returned a conversation ID, not that background memory formation completed. Inspect
that conversation with `conversations get` for its processing result. Failed or
interrupted submissions remain visible in `unresolved`/`failed` counts and produce a
nonzero exit status. `--batch-retry-failed` retries failed submissions; an interrupted
`working` entry requires reconciliation and explicit `--batch-force` before resubmission.
Transport failures can also occur after server acceptance, so inspect receipts before
retrying them.

During a run, `<report>.jsonl` appends individual status changes; it is replayed on
restart and folded into the JSON checklist on normal return. Keep it with the report
if a run was interrupted. Run only one batch at a time per report. Batch-only flags
require `--batch-dir`. Formation size/token limits are enforced by the server.

### Space Info & Conversations

```bash
# Get space information and statistics
anda-cli --space-id my_space --token $TOKEN info

# Get formation processing status
anda-cli --space-id my_space --token $TOKEN formation-status

# Get or initialize a user concept
anda-cli --space-id my_space --token $TOKEN get-or-init-user principal_123 --name Alice

# List conversations
anda-cli --space-id my_space --token $TOKEN conversations list --limit 10

# Get a specific conversation
anda-cli --space-id my_space --token $TOKEN conversations get 42

# Get incremental updates for a conversation
anda-cli --space-id my_space --token $TOKEN conversations delta 42 \
  --messages-offset 10 \
  --artifacts-offset 2
```

### Space Management (requires CWT auth)

```bash
# List space tokens
anda-cli --space-id my_space --token $CWT_TOKEN management list-tokens

# Add a space token (--name is required; full token value is only shown once)
anda-cli --space-id my_space --token $CWT_TOKEN management add-token --name writer --scope write

# Add a label-restricted wiki viewer token (labels require --scope read;
# the token sees unlabeled content plus the listed ACL labels)
anda-cli --space-id my_space --token $CWT_TOKEN management add-token \
  --name hr-viewer --scope read --labels hr,finance

# Read only unlabeled wiki documents; distinct from an unrestricted token
anda-cli --space-id my_space --token $CWT_TOKEN management add-token \
  --name public-viewer --scope read --unlabeled-only

# Label-restricted tokens cannot use agentic recall or read conversation history.

# Revoke a space token by full value
anda-cli --space-id my_space --token $CWT_TOKEN management revoke-token ST_xxx

# Revoke by unique token name (list-tokens only echoes a token prefix, so use
# --name when the full value was not saved at mint time). Both forms exit nonzero
# when no token matched.
anda-cli --space-id my_space --token $CWT_TOKEN management revoke-token --name hr-viewer

# Update space info
anda-cli --space-id my_space --token $CWT_TOKEN management update-space --name "My Space" --public

# Update wiki settings: enable WikiDigest extraction, audit external wiki
# reads, and set namespace default ACL labels (namespace=label pairs; the
# map is replaced as a whole — pass --wiki-acl-defaults "" to clear it)
anda-cli --space-id my_space --token $CWT_TOKEN management update-space \
  --wiki-digest \
  --wiki-audit-reads \
  --wiki-acl-defaults internal=staff,hr=hr

# Replace the space memory policy (omitted members use server defaults)
anda-cli --space-id my_space --token $CWT_TOKEN management update-space \
  --memory-policy @./memory-policy.json

# Compare a candidate policy before promoting it
anda-cli --space-id my_space --token $CWT_TOKEN management shadow-eval \
  --policy @./memory-policy.json --replay-sample 4

# Restart formation for a conversation
anda-cli --space-id my_space --token $CWT_TOKEN management restart-formation --conversation 42

# Get BYOK configuration
anda-cli --space-id my_space --token $CWT_TOKEN management get-byok

# Update BYOK configuration. --api-key accepts a literal value, @file/path
# (recommended: keeps the key out of shell history and `ps`), or the
# ANDA_BYOK_API_KEY environment variable as its default.
anda-cli --space-id my_space --token $CWT_TOKEN management update-byok \
  --family anthropic \
  --model claude-opus-4-6 \
  --api-base https://api.anthropic.com/v1 \
  --api-key @./api_key.txt \
  --stream \
  --context-window 200000 \
  --max-output 8192

# Reasoning effort can be set to minimal, low, medium, high, or max
anda-cli --space-id my_space --token $CWT_TOKEN management update-byok \
  --family anthropic --model claude-opus-4-6 \
  --api-base https://api.anthropic.com/v1 --api-key @./api_key.txt --effort high

# Or via environment variable:
export ANDA_BYOK_API_KEY=sk-xxx
anda-cli --space-id my_space --token $CWT_TOKEN management update-byok \
  --family anthropic --model claude-opus-4-6 --api-base https://api.anthropic.com/v1
```

### Wiki

The `wiki` command covers the versioned reference document API. A write token is needed for commits, archive, restore, and digest; import/export require `*` scope. Read commands follow the space's public and ACL rules; `events` requires an unrestricted read credential. Citation verification does not require a model call.

```bash
# Commit a Markdown document; use --doc-id and --parent-version for a CAS update
anda-cli --space-id my_space --token $TOKEN wiki commit \
  --file ./guide.md --title "Guide" --namespace docs --tags onboarding
anda-cli --space-id my_space --token $TOKEN wiki commit --input @./commit.json

# --input is a complete request and cannot be mixed with --file, --acl-label,
# --doc-id, --parent-version, or other document fields; put them in the JSON.

# In a commit JSON update, tags:[] clears tags and metadata:{} clears metadata

anda-cli --space-id my_space --token $TOKEN wiki list --namespace docs --limit 20
anda-cli --space-id my_space --token $TOKEN wiki get 7
anda-cli --space-id my_space --token $TOKEN wiki read 7 --anchor setup
anda-cli --space-id my_space --token $TOKEN wiki versions 7
anda-cli --space-id my_space --token $TOKEN wiki search "installation" --namespaces docs
anda-cli --space-id my_space --token $TOKEN wiki verify --uri 'wiki://my_space/7@12#0-100'
anda-cli --space-id my_space --token $TOKEN wiki events --doc-id 7
anda-cli --space-id my_space --token $TOKEN wiki archive 7
anda-cli --space-id my_space --token $TOKEN wiki restore 7
anda-cli --space-id my_space --token $TOKEN wiki digest

# OKF bundle JSON has an entries array of {path, content} objects
anda-cli --space-id my_space --token $FULL_TOKEN wiki import --input @./bundle.json
anda-cli --space-id my_space --token $FULL_TOKEN wiki export --namespace docs

# Exported bundles can be imported directly, including their docs summary
anda-cli --space-id my_space --token $FULL_TOKEN wiki export --namespace docs > bundle.json
anda-cli --space-id my_space --token $FULL_TOKEN wiki import --input @bundle.json
```

### Admin (requires platform admin auth)

```bash
# Create a space
anda-cli --token $ADMIN_TOKEN admin create-space --user owner_id --space-id new_space --tier 1

# Update space tier
anda-cli --token $ADMIN_TOKEN admin update-tier --user owner_id --space-id my_space --tier 2
```
