/**
 * The Worker half of the Memory Interface: one request in, one Response out.
 * Intake, receipts, scope, feedback, repair, forget and the retained basis
 * live in the Durable Object (`memory-ledger.ts`); this file runs the model
 * passes around them. Formation on this Worker completes within the request,
 * so a successful observe or revise answers `succeeded` with available
 * progress; the receipt is still the barrier a later recall names.
 */
import { formMemoryWithResults, recallMemory } from './operations.js'
import {
  DEFAULT_DEADLINE_MS,
  DEFAULT_OUTPUT_TOKENS,
  KIP_MEMORY,
  MemoryError,
  checkRequires,
  errorOf,
  failed,
  fitsBudget,
  mutationResponse,
  parseMemoryRequest,
  type Briefing,
  type MemoryRequest,
  type MemoryResponse,
  type Progress,
} from './memory-wire.js'
import type { IntakeRecord, MemoryIntent, PassTrace } from './memory-ledger.js'
import type { BrainRpc, Env } from './types.js'
import type { JsonMap, KipResult } from '@ldclabs/kip-do'

/**
 * This Worker has one API key, so every caller is one namespace: handles are
 * scoped to it and to the Space.
 */
export const NAMESPACE = 'api'

export async function handleMemory(env: Env, brain: BrainRpc, spaceId: string, body: unknown, owner = true): Promise<MemoryResponse> {
  let request: MemoryRequest
  try {
    request = parseMemoryRequest(body)
  } catch (error) {
    const envelope = body as { operation?: unknown; request_id?: unknown } | null
    const operation = envelope?.operation
    const requestId = envelope?.request_id
    return failed({
      operation: typeof operation === 'string' && ['recall', 'observe', 'revise', 'feedback', 'forget'].includes(operation)
        ? operation as MemoryRequest['operation'] : 'recall',
      ...(typeof requestId === 'string' && requestId.length > 0 && requestId.length <= 2048 && [...requestId].length <= 1024
        ? { request_id: requestId } : {}),
    }, error)
  }
  try {
    if (request.space && (request.space.uri !== undefined || request.space.id !== spaceId)) {
      throw new MemoryError('NotFoundOrNotVisible', "the requested Space is not this connection's Space")
    }
    checkRequires(request)
    switch (request.operation) {
      case 'recall':
        return await recall(env, brain, spaceId, request)
      case 'forget':
        return respond(request, await brain.memoryForget(NAMESPACE, spaceId, request, owner))
      default:
        return await intake(env, brain, spaceId, request)
    }
  } catch (error) {
    return failed(request, error)
  }
}

function respond(request: MemoryRequest, record: IntakeRecord): MemoryResponse {
  const progress: Progress = record.terminal ?? { receipt_ref: record.receipt.receipt_ref, phase: 'recorded' }
  const result = record.result ?? { summary: 'Recorded; processing is pending.', memory_refs: [] }
  return mutationResponse(request, record.receipt, progress, result, record.warnings)
}

async function intake(env: Env, brain: BrainRpc, spaceId: string, request: MemoryRequest): Promise<MemoryResponse> {
  const admission = await brain.memoryAdmit(NAMESPACE, spaceId, request)
  if ('replay' in admission) return respond(request, admission.replay)
  const receipt = admission.record.receipt.receipt_ref
  const input = request.input as Record<string, unknown>
  if (request.operation === 'feedback') {
    // Both are references `parseMemoryRequest` already checked.
    const about = { decision_ref: input.decision_ref ?? null, attempt_ref: input.attempt_ref ?? null } as JsonMap
    return respond(request, await brain.memoryCaptureEvidence(receipt, admission.messages, admission.observed_at, 'feedback', about))
  }
  if (request.operation === 'revise' && input.change_kind === 'misrecorded' && input.target_ref === undefined) {
    return respond(request, await brain.memoryCaptureEvidence(receipt, admission.messages, admission.observed_at, 'revise-report'))
  }
  try {
    const { results } = await formMemoryWithResults(env, brain,
      { messages: admission.messages, timestamp: admission.observed_at }, admission.identity, admission.intent as MemoryIntent)
    return respond(request, await brain.memoryFinishFormation(receipt, traceOf(results)))
  } catch (error) {
    return respond(request, await brain.memoryFailFormation(receipt, errorOf(error)))
  }
}

/** What a pass committed, from its operation results. */
export function traceOf(results: readonly KipResult[]): PassTrace {
  const trace: PassTrace = { formed: [], evidence: [], assertions: [], created: [], max_seq: null }
  for (const result of results) {
    const outcome = result.extensions?.['kip-do/outcome']
    if (!outcome || outcome.status !== 'committed') continue
    if (typeof outcome.space_seq === 'number') trace.max_seq = Math.max(trace.max_seq ?? 0, outcome.space_seq)
    for (const change of outcome.changes) {
      if (typeof change.id !== 'string') continue
      if (change.op === 'create') trace.created.push(change.id)
      if (change.kind === 'evidence') trace.evidence.push(change.id)
      else {
        trace.formed.push(change.id)
        if (change.kind === 'assertion' && change.op === 'create') trace.assertions.push(change.id)
      }
    }
  }
  return trace
}

async function recall(env: Env, brain: BrainRpc, _spaceId: string, request: MemoryRequest): Promise<MemoryResponse> {
  const input = request.input as {
    query?: string; target_ref?: string; mode?: string; goal?: string; context?: string; after?: string[]
    detail?: string; time?: { valid_at?: string; as_of_seq?: number }; attention_cursor?: string
  }
  const maxTokens = request.budget?.max_output_tokens ?? DEFAULT_OUTPUT_TOKENS
  const deadline = Date.now() + (request.budget?.deadline_ms ?? DEFAULT_DEADLINE_MS)
  if (input.target_ref !== undefined) {
    // An expansion has its own budget (MI §6).
    return briefingResponse(request, await brain.memoryExpand(NAMESPACE, input.target_ref, input.detail === 'evidence'),
      request.budget?.max_output_tokens ?? 65_536, [])
  }
  const mode = input.mode ?? 'answer'
  const after = await brain.memoryBarrier(NAMESPACE, input.after ?? [])
  const asOf = input.time?.as_of_seq
  if (asOf !== undefined && after.some(p => (p.available_seq ?? -1) > asOf)) {
    throw new MemoryError('PreconditionFailed', 'an after receipt is newer than the fixed as_of_seq')
  }
  const uncertainties: string[] = []
  for (const progress of after) {
    if (progress.phase === 'failed') uncertainties.push(`receipt ${progress.receipt_ref} failed processing; its source is not in memory: ${progress.reason ?? 'no reason recorded'}`)
    else if (progress.phase === 'recorded') uncertainties.push(`receipt ${progress.receipt_ref} is still being processed; an answer may not reflect it`)
  }
  const warnings: string[] = []
  let attention: { items: Briefing['attention']; cursor: string } | undefined
  if (mode === 'attention' || mode === 'resume') {
    let page
    try {
      page = await brain.recallAttention({ ...(input.attention_cursor ? { attention_cursor: input.attention_cursor } : {}), limit: 20 })
    } catch (error) {
      throw new MemoryError('CursorInvalid', error instanceof Error ? error.message : String(error))
    }
    attention = { items: await brain.memoryScopedAttention(request.scope, page.items), cursor: page.attention_cursor }
  }
  let cited: string[] = []
  let summary: string | undefined
  let evidenceComplete = mode === 'attention'
  if (mode !== 'attention' && input.query) {
    try {
      const remaining = deadline - Date.now()
      if (remaining <= 0) throw new MemoryError('ExecutionTimeout', 'deadline')
      // The budget caps the pass's own model deadline, so a pass past it is
      // cancelled rather than left running (and billed) behind the answer.
      const output = await recallMemory(env, brain,
        { query: recallPrompt(input.query, mode, request, input) }, remaining) as { answer?: string; memories?: { entity: string }[] }
      cited = (output.memories ?? []).map(memory => memory.entity).filter(Boolean)
      summary = output.answer
      evidenceComplete = true
    } catch (error) {
      uncertainties.push(`the recall pass did not complete: ${errorOf(error).message}`)
    }
  }
  const delivered = await brain.memoryDeliver(NAMESPACE, request.scope, cited, {
    mode, ...(input.time?.valid_at ? { valid_at: input.time.valid_at } : {}), ...(asOf === undefined ? {} : { as_of_seq: asOf }),
    ...(input.query ? { query: input.query } : {}), after, uncertainties, evidence_complete: evidenceComplete,
    ...(summary ? { summary } : {}), max_tokens: maxTokens, warnings,
    ...(attention ? { attention: attention.items, attention_cursor: attention.cursor } : {}),
  })
  return briefingResponse(request, delivered.briefing, maxTokens, delivered.warnings)
}

function briefingResponse(request: MemoryRequest, briefing: Briefing, maxTokens: number, warnings: string[]): MemoryResponse {
  if (!fitsBudget(briefing, maxTokens)) {
    throw new MemoryError('ResultLimitExceeded', `the result does not fit ${maxTokens} tokens`)
  }
  const status = briefing.coverage.complete ? 'succeeded' : briefing.coverage.pending_receipts.length ? 'pending' : 'partial'
  return {
    kip_memory: KIP_MEMORY,
    ...(request.request_id === undefined ? {} : { request_id: request.request_id }),
    operation: 'recall', status, result: briefing, warnings: [...new Set(warnings)],
  }
}

/** The Recall pass's question, with scope, times and transient context stated as data. */
function recallPrompt(query: string, mode: string, request: MemoryRequest, input: { goal?: string; context?: string; time?: { valid_at?: string; as_of_seq?: number } }): string {
  const lines = [query, '', `[Memory Interface recall — mode ${mode}. Read-only: write nothing.]`]
  if (request.scope?.task_ref || request.scope?.context_refs?.length) {
    lines.push(`Scope: task ${request.scope.task_ref ?? 'none'}, contexts ${JSON.stringify(request.scope.context_refs ?? [])}; memory from other tasks does not apply.`)
  }
  if (input.time?.valid_at) lines.push(`Answer for world time ${input.time.valid_at} (FOR TIME).`)
  if (input.time?.as_of_seq !== undefined) lines.push(`Answer as the Brain stood at AS OF SEQ ${input.time.as_of_seq}.`)
  if (input.goal) lines.push(`Goal: ${input.goal}`)
  if (input.context) lines.push(`Current situation (transient, not memory; do not store): ${input.context}`)
  return lines.join('\n')
}
