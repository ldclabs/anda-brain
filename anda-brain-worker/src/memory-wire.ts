/**
 * The KIP 2.0 Memory Interface wire (`KIP-2.0-Memory-Interface.md`, shapes in
 * `kip-memory.schema.json`): one intent per request, the envelope rules the
 * schema states, and the response shapes this Worker returns. Mirrors
 * `anda_kip::memory::binding` in the Rust Brain.
 */
import { isJsonMap } from '@ldclabs/kip-do'
import { validateValue } from '@ldclabs/kip-do/schema'

export const KIP_MEMORY = '2.0'
export const MAX_AFTER = 128
/** Same pinned name the Rust Brain advertises; see {@link fitsBudget}. */
export const TOKENIZER = 'o200k_base@tiktoken-rs-0.12'
/** The name advertised through Brain 0.13.1, for the same encoding. */
export const LEGACY_TOKENIZER = 'o200k_base@tiktoken-rs-0.12.0'
export const DEFAULT_OUTPUT_TOKENS = 4096
export const DEFAULT_DEADLINE_MS = 30_000
export const MAX_DEADLINE_MS = 120_000
export const MINIMUM_RESPONSE_TOKENS = 256

export type Operation = 'observe' | 'recall' | 'revise' | 'feedback' | 'forget'
export type Bundle = 'memory_basic' | 'memory_experience' | 'memory_learning'
export type Phase = 'recorded' | 'processed' | 'available' | 'failed'
export type Disposition = 'formed' | 'evidence_only' | 'skipped' | 'erased'
export type Status = 'succeeded' | 'pending' | 'partial' | 'failed'
export type ChangeKind = 'correction' | 'world_change' | 'misrecorded' | 'unspecified'
export type ChannelState = 'complete' | 'incomplete' | 'not_applicable'
export type Role = 'fact' | 'constraint' | 'experience' | 'procedure' | 'warning' | 'source'
export type EpistemicStatus = 'accepted' | 'rejected' | 'contested' | 'uncertain' | 'insufficient' | 'not_applicable'

export interface Scope { task_ref?: string; context_refs?: string[] }
export interface Budget { max_output_tokens?: number; deadline_ms?: number; tokenizer?: string }
export interface MemoryRequest {
  kip_memory: string
  request_id?: string
  operation: Operation
  space?: { id?: string; uri?: string }
  scope?: Scope
  budget?: Budget
  idempotency_key?: string
  requires?: Bundle[]
  input: Record<string, unknown>
}
export interface KipErrorObject { code: string; message: string; category?: string; hint?: string }
export interface Receipt { receipt_ref: string; operation: Operation; space_id: string; accepted_seq: number }
export interface Progress {
  receipt_ref: string; phase: Phase; disposition?: Disposition
  resolved_seq?: number; available_seq?: number; reason?: string; error?: KipErrorObject
}
export interface MemoryItem {
  ref: string; text: string; role: Role; epistemic_status: EpistemicStatus
  evidence_refs: string[]; action_eligible: boolean; standing?: 'unproven' | 'validated' | 'revoked' | 'unverifiable'
}
export interface Channels {
  constraints: ChannelState; commitments: ChannelState; dependencies: ChannelState
  failures: ChannelState; experiences: ChannelState; skills: ChannelState; evidence: ChannelState
}
export interface Coverage {
  complete: boolean; scope: Scope; channels: Channels; pending_receipts: string[]
  unverified_preconditions: string[]; action_eligible: boolean
}
export interface AttentionItem {
  ref: string; kind: 'watch_fired' | 'commitment_due'; summary: string; raised_seq: number
  due_at?: string; target_refs: string[]; priority?: number
}
export interface Briefing {
  summary: string; items: MemoryItem[]; uncertainties: string[]; basis_ref: string
  coverage: Coverage; after: Progress[]; continuation_ref?: string
  details?: { basis: unknown; coverage: unknown; elements?: unknown[] }
  attention?: AttentionItem[]; attention_cursor?: string
}
export interface MemoryResponse {
  kip_memory: string; request_id?: string; operation: Operation; status: Status
  receipt?: Receipt; progress?: Progress; result?: unknown; error?: KipErrorObject; warnings: string[]
}
export interface Descriptor {
  kip_memory: string; bundles: Bundle[]; default_scope?: Scope
  default_budget: { max_output_tokens: number; deadline_ms: number }
  tokenizer: string; minimum_response_tokens: number; default_space?: { id: string }
}

/** A Memory Interface failure carrying its KIP error code. */
export class MemoryError extends Error {
  constructor(readonly code: string, message: string) {
    super(message)
    this.name = 'MemoryError'
  }
  toJSON(): KipErrorObject { return { code: this.code, message: this.message } }
}

export const errorOf = (error: unknown): KipErrorObject => {
  if (error instanceof MemoryError) return error.toJSON()
  const known = error as { code?: unknown; message?: unknown } | null
  if (known && typeof known.code === 'string' && typeof known.message === 'string') {
    return { code: known.code, message: known.message }
  }
  const message = error instanceof Error ? error.message : String(error)
  if (message === 'source_suppressed') return { code: 'NotFoundOrNotVisible', message: 'the source is excluded from memory by an earlier forget' }
  if (message === 'memory_change_pending') return { code: 'PreconditionFailed', message: 'a managed memory change is reconciling; retry after it finishes' }
  return { code: 'InternalError', message }
}

/** What this Worker advertises (MI §2): `memory_basic` only. */
export function descriptor(spaceId?: string): Descriptor {
  return {
    kip_memory: KIP_MEMORY,
    bundles: ['memory_basic'],
    default_budget: { max_output_tokens: DEFAULT_OUTPUT_TOKENS, deadline_ms: DEFAULT_DEADLINE_MS },
    tokenizer: TOKENIZER,
    minimum_response_tokens: MINIMUM_RESPONSE_TOKENS,
    ...(spaceId === undefined ? {} : { default_space: { id: spaceId } }),
  }
}

const REQUEST_SCHEMA = { $ref: 'urn:kip:2.0:schema:memory#/$defs/Request' }
const BUNDLES = new Set<string>(['memory_basic', 'memory_experience', 'memory_learning'])

/** Validate the wire shape with the engine's compiled KIP schema before dispatch. */
export function parseMemoryRequest(value: unknown): MemoryRequest {
  if (!isJsonMap(value)) throw new MemoryError('InvalidRequestEnvelope', 'a Memory Interface request is an object')
  // MI §3 gives an unknown level a capability error even though Bundle is an enum.
  if (Array.isArray(value.requires)) {
    const unknown = value.requires.find(level => typeof level === 'string' && !BUNDLES.has(level))
    if (unknown !== undefined) throw new MemoryError('UnsupportedCapability', `unknown memory level ${unknown}`)
  }
  try {
    validateValue(REQUEST_SCHEMA, value)
  } catch (error) {
    throw new MemoryError('InvalidRequestEnvelope', errorOf(error).message)
  }
  const request = value as unknown as MemoryRequest
  if (request.operation === 'recall' && typeof request.input.query === 'string' && !request.input.query.trim()) {
    throw new MemoryError('InvalidRequestEnvelope', 'query is non-empty text')
  }
  return request
}

/**
 * The `requires` check (MI §3): omitted means `memory_basic`; an unknown or
 * unadvertised level is refused before anything runs.
 */
export function checkRequires(request: MemoryRequest): void {
  const required = request.requires?.length ? request.requires : ['memory_basic']
  for (const bundle of required) {
    if (bundle !== 'memory_basic') {
      throw new MemoryError('UnsupportedCapability', `this binding does not advertise ${bundle}`)
    }
  }
  const tokenizer = request.budget?.tokenizer
  if (tokenizer !== undefined && tokenizer !== TOKENIZER && tokenizer !== LEGACY_TOKENIZER) {
    throw new MemoryError('UnsupportedCapability', `tokenizer ${JSON.stringify(tokenizer)} is not supported; this binding counts with ${TOKENIZER}`)
  }
  if ((request.budget?.deadline_ms ?? 0) > MAX_DEADLINE_MS) {
    throw new MemoryError('ResultLimitExceeded', `deadline_ms is at most ${MAX_DEADLINE_MS}`)
  }
}

/**
 * Whether a serialized result fits the output budget. The Worker carries no
 * tokenizer tables, so it bounds conservatively: every o200k token spans at
 * least one UTF-8 byte, so a result of at most N bytes is at most N tokens.
 * That never undercounts; it can refuse or trim a result that would have fit,
 * and it never treats bytes as the token count it reports.
 */
export function fitsBudget(value: unknown, maxTokens: number): boolean {
  return new TextEncoder().encode(JSON.stringify(value)).byteLength <= maxTokens
}

export function failed(request: Pick<MemoryRequest, 'operation' | 'request_id'>, error: unknown): MemoryResponse {
  return {
    kip_memory: KIP_MEMORY,
    ...(request.request_id === undefined ? {} : { request_id: request.request_id }),
    operation: request.operation,
    status: 'failed',
    error: errorOf(error),
    warnings: [],
  }
}

/**
 * A mutation's response from its progress (MI §5): `succeeded` needs
 * available progress, recorded work is `pending`, a terminal failure is
 * `failed` with its error.
 */
export function mutationResponse(
  request: MemoryRequest, receipt: Receipt, progress: Progress, result: unknown, warnings: string[],
): MemoryResponse {
  let status: Status = progress.phase === 'available' ? 'succeeded'
    : progress.phase === 'failed' ? 'failed' : progress.phase === 'processed' ? 'partial' : 'pending'
  if (status === 'succeeded' && warnings.some(w => w.startsWith('partial:'))) status = 'partial'
  if (status === 'pending' && isJsonMap(result) && result.status === 'partial') status = 'partial'
  return {
    kip_memory: KIP_MEMORY,
    ...(request.request_id === undefined ? {} : { request_id: request.request_id }),
    operation: request.operation,
    status,
    receipt,
    progress,
    ...(status === 'failed'
      ? { error: progress.error ?? { code: 'InternalError', message: 'processing failed' } }
      : { result }),
    warnings,
  }
}

export function coverage(scope: Scope, channels: Channels, pending: string[], unverified: string[]): Coverage {
  const complete = Object.values(channels).every(state => state !== 'incomplete') && pending.length === 0
  return { complete, scope, channels, pending_receipts: pending, unverified_preconditions: unverified, action_eligible: complete && unverified.length === 0 }
}
