import { env } from 'cloudflare:test'
import { describe, expect, it } from 'vitest'
import { handleRequest } from '../src/index.js'
import type { AiBinding, Env } from '../src/types.js'

/**
 * The Memory Interface on the Worker: the same mechanism cases as the Rust
 * `memory_interface::tests`, driven through `POST /v1/{space}/memory`. A
 * fake model stands in for Workers AI; everything else is the real path.
 *
 * @see anda_brain/src/memory_interface/tests.rs
 */
class FakeAi implements AiBinding {
  constructor(private readonly responses: unknown[]) {}
  push(...responses: unknown[]): void { this.responses.push(...responses) }
  async run(): Promise<unknown> {
    const response = this.responses.shift()
    if (response === undefined) throw new Error('unexpected AI call')
    return { response, usage: { input_tokens: 1, output_tokens: 1 } }
  }
}

const testEnv = (ai: AiBinding): Env => ({ BRAIN: env.BRAIN as unknown as Env['BRAIN'], AI: ai, AI_MODEL: 'test-model' })
const uniqueSpace = (prefix: string): string => `${prefix}-${crypto.randomUUID()}`

async function call(runtime: Env, space: string, action: string, body?: unknown): Promise<Record<string, any>> {
  const response = await handleRequest(new Request(`https://brain.example/v1/${space}/${action}`, body === undefined
    ? {} : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }), runtime)
  return { status: response.status, ...(await response.json<Record<string, any>>()) }
}

const memory = (runtime: Env, space: string, request: Record<string, unknown>) =>
  call(runtime, space, 'memory', { kip_memory: '2.0', ...request })

async function stage(runtime: Env, space: string, key: string, text: string, at: string, role = 'user'): Promise<string> {
  const staged = await call(runtime, space, 'memory/sources', { messages: [{ role, content: text }], observed_at: at, idempotency_key: key })
  expect(staged.status).toBe(200)
  return staged.result.source_ref as string
}

async function declareTypes(space: string, types: string[]): Promise<void> {
  await (env.BRAIN.getByName(space) as unknown as { declareSymbols(t: string[], p: string[]): Promise<unknown> }).declareSymbols(types, [])
}

async function claims(runtime: Env, space: string, option: string): Promise<Record<string, any>[]> {
  const read = await call(runtime, space, 'execute_kip_readonly', {
    command: 'FIND(?a) WHERE { ?o {key: :key} ?p (?u, "prefers", ?o) ?a ASSERTION {proposition: ?p} }', parameters: { key: option },
  })
  return (read.result[0].result ?? []) as Record<string, any>[]
}

const prefers = (option: string, kind: string, at: string, scoped = false) => `MUTATE {
  UPSERT CONCEPT ?user { MATCH {type: "Person", key: "user"} SET FIELDS {name: "User"} }
  UPSERT CONCEPT ?option { MATCH {type: "${kind}", key: "${option}"} SET FIELDS {name: "${option}"} }
  ASSERT ?claim (?user, "prefers", ?option) { by: ?user, mode: "stated", evidence: :msg1, at: "${at}"${scoped ? ', context: :contexts' : ''} }
}`
const plan = (commands: string[]) => ({ types: [], predicates: [], commands, summary: 'formed' })

describe('Memory Interface', () => {
  it('advertises only what it serves', async () => {
    const runtime = testEnv(new FakeAi([]))
    const space = uniqueSpace('mi-descriptor')
    const info = await call(runtime, space, 'info')
    expect(info.result.memory_interface.bundles).toEqual(['memory_basic'])
    expect(info.result.memory_interface.tokenizer).toBe('o200k_base@tiktoken-rs-0.12')
    const learning = await memory(runtime, space, { operation: 'recall', requires: ['memory_learning'], input: { mode: 'attention' } })
    expect(learning.error.code).toBe('UnsupportedCapability')
    const tokenizer = await memory(runtime, space, { operation: 'recall', budget: { tokenizer: 'cl100k_base' }, input: { mode: 'attention' } })
    expect(tokenizer.error.code).toBe('UnsupportedCapability')
    // The name advertised through Brain 0.13.1 is the same encoding.
    const legacy = await memory(runtime, space, { operation: 'recall', budget: { tokenizer: 'o200k_base@tiktoken-rs-0.12.0' }, input: { mode: 'attention' } })
    expect(legacy.error).toBeUndefined()
    const keyless = await memory(runtime, space, { operation: 'observe', input: { source_ref: 'src-x' } })
    expect(keyless.error.code).toBe('InvalidRequestEnvelope')
    const future = await memory(runtime, space, { operation: 'recall', request_id: 'req-1', requires: ['future_bundle'], input: { mode: 'attention' } })
    expect(future.error.code).toBe('UnsupportedCapability')
    expect(future.request_id).toBe('req-1')
    const described = await call(runtime, space, 'execute_kip_readonly', { command: 'DESCRIBE CAPABILITIES' })
    expect(described.result[0].result.supported.registry.memory_interface.bundles).toEqual(['memory_basic'])
  })

  it('validates recall at the wire boundary using KIP character and receipt limits', async () => {
    const runtime = testEnv(new FakeAi([]))
    const space = uniqueSpace('mi-wire')
    for (const input of [
      { mode: 'resume' }, { mode: 'attention', time: {} },
      { mode: 'attention', after: ['r1', 'r1'] }, { target_ref: 42 },
      { mode: 'attention', goal: 'x'.repeat(4097) }, { query: ' \n\t ' },
    ]) {
      const result = await memory(runtime, space, { operation: 'recall', input })
      expect(result.error?.code).toBe('InvalidRequestEnvelope')
    }
    const result = await memory(runtime, space, {
      operation: 'recall', request_id: '🌏'.repeat(1024), input: { mode: 'attention' },
    })
    expect(result.error).toBeUndefined()
    expect(result.request_id).toBe('🌏'.repeat(1024))
  })

  it('stages sources idempotently and observes them once', async () => {
    const ai = new FakeAi([plan([prefers('dark', 'ColorScheme', '2026-01-01T00:00:00.000Z')])])
    const runtime = testEnv(ai)
    const space = uniqueSpace('mi-observe')
    await declareTypes(space, ['ColorScheme'])
    const source = await stage(runtime, space, 's1', 'I prefer dark mode', '2026-01-01T00:00:00.000Z')
    expect(await stage(runtime, space, 's1', 'I prefer dark mode', '2026-01-01T00:00:00.000Z')).toBe(source)
    const conflict = await call(runtime, space, 'memory/sources', { messages: [{ role: 'user', content: 'other' }], idempotency_key: 's1' })
    expect(conflict.status).toBe(409)

    const observed = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:s1', input: { source_ref: source } })
    expect(observed.status).toBe('succeeded')
    expect(observed.progress).toMatchObject({ phase: 'available', disposition: 'formed' })
    expect(observed.result.memory_refs.length).toBeGreaterThan(0)
    // A replay returns the same receipt and never re-runs extraction (the
    // fake model has no response left to give).
    const replay = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:s1', request_id: 'retry', input: { source_ref: source } })
    expect(replay.receipt).toEqual(observed.receipt)
    const changed = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:s1', scope: { task_ref: 'task-9' }, input: { source_ref: source } })
    expect(changed.error.code).toBe('IdempotencyConflict')
    const view = await call(runtime, space, `memory/receipts/${observed.receipt.receipt_ref}`)
    expect(view.result.progress.phase).toBe('available')
    // The receipt satisfies a barrier.
    const barrier = await memory(runtime, space, { operation: 'recall', input: { mode: 'attention', after: [observed.receipt.receipt_ref] } })
    expect(barrier.result.coverage.pending_receipts).toEqual([])
    const foreign = await memory(runtime, space, { operation: 'recall', input: { mode: 'attention', after: ['rcpt-' + '0'.repeat(40)] } })
    expect(foreign.error.code).toBe('NotFoundOrNotVisible')
  })

  it('keeps a scoped observation in its task', async () => {
    const ai = new FakeAi([plan([prefers('light', 'ColorScheme', '2026-02-01T00:00:00.000Z')])])
    const runtime = testEnv(ai)
    const space = uniqueSpace('mi-scope')
    await declareTypes(space, ['ColorScheme'])
    const source = await stage(runtime, space, 't1', 'For task A, use light mode', '2026-02-01T00:00:00.000Z')
    // The unscoped plan is refused by the gate: nothing is formed.
    const refused = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:t1', scope: { task_ref: 'task-a' }, input: { source_ref: source } })
    expect(refused.status).toBe('failed')
    expect(await claims(runtime, space, 'light')).toEqual([])
    ai.push(plan([prefers('light', 'ColorScheme', '2026-02-01T00:00:00.000Z', true)]))
    const scoped = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:t1b', scope: { task_ref: 'task-a' }, input: { source_ref: source } })
    expect(scoped.status).toBe('succeeded')
    const [claim] = await claims(runtime, space, 'light')
    expect(claim!.context_refs).toHaveLength(1)

    const find = `FIND(?a) WHERE { ?a ASSERTION {id: "${claim!.id as string}"} } LIMIT 1`
    ai.push({ commands: [find] }, { answer: 'You prefer light mode.', found: true, uncertainty: 0.1 })
    const other = await memory(runtime, space, { operation: 'recall', scope: { task_ref: 'task-b' }, input: { query: 'Which color scheme?' } })
    expect(other.result.items.some((item: any) => item.text.includes('light'))).toBe(false)
    ai.push({ commands: [find] }, { answer: 'You prefer light mode.', found: true, uncertainty: 0.1 })
    const own = await memory(runtime, space, { operation: 'recall', scope: { task_ref: 'task-a' }, input: { query: 'Which color scheme?' } })
    const item = own.result.items.find((entry: any) => entry.role === 'fact' && entry.text.includes('light'))
    expect(item?.epistemic_status).toBe('accepted')
  })

  it('routes revisions by history and repairs a misrecording', async () => {
    const ai = new FakeAi([plan([prefers('vegetarian', 'Diet', '2026-01-01T00:00:00.000Z')])])
    const runtime = testEnv(ai)
    const space = uniqueSpace('mi-revise')
    await declareTypes(space, ['Diet'])
    const source = await stage(runtime, space, 'r0', 'Alice talked about dinner', '2026-01-01T00:00:00.000Z')
    await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:r0', input: { source_ref: source } })
    const [wrong] = await claims(runtime, space, 'vegetarian')

    // A world change never supersedes: the gate refuses the plan.
    ai.push(plan([`ASSERT (:x, "prefers", :y) { by: :x, mode: "stated", evidence: :msg1, at: "2026-09-01T00:00:00.000Z" } SUPERSEDING "${wrong!.id as string}"`]))
    const moved = await stage(runtime, space, 'r1', 'I changed my mind', '2026-09-01T00:00:00.000Z')
    const world = await memory(runtime, space, { operation: 'revise', idempotency_key: 'revise:r1', input: { source_ref: moved, change_kind: 'world_change' } })
    expect(world.status).toBe('failed')

    // Without a target the report is preserved, nothing is repaired.
    const report = await stage(runtime, space, 'r2', 'You misheard me', '2026-09-24T00:00:00.000Z')
    const untargeted = await memory(runtime, space, { operation: 'revise', idempotency_key: 'revise:r2', input: { source_ref: report, change_kind: 'misrecorded' } })
    expect(untargeted.status).toBe('partial')
    expect(untargeted.progress.disposition).toBe('evidence_only')

    ai.push(plan([]))
    const repaired = await memory(runtime, space, { operation: 'revise', idempotency_key: 'revise:r3', input: { source_ref: report, change_kind: 'misrecorded', target_ref: wrong!.id } })
    expect(repaired.status).toBe('succeeded')
    const [after] = await claims(runtime, space, 'vegetarian')
    expect(after!._system.recording_validity.status).toBe('invalidated')
    expect(after!.lifecycle.status).toBe('active')
  })

  it('keeps feedback as attributed Evidence, not a grade', async () => {
    const runtime = testEnv(new FakeAi([]))
    const space = uniqueSpace('mi-feedback')
    const source = await stage(runtime, space, 'f1', 'I completed the deployment successfully', '2026-09-01T00:00:00.000Z', 'assistant')
    const missing = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'feedback:bad', input: { source_ref: source, decision_ref: 'X-999' } })
    expect(missing.error.code).toBe('NotFoundOrNotVisible')
    const feedback = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'feedback:f1', input: { source_ref: source } })
    expect(feedback.status).toBe('succeeded')
    expect(feedback.progress.disposition).toBe('evidence_only')
    const read = await call(runtime, space, 'execute_kip_readonly', { command: 'FIND(?e.evidence_class) WHERE { ?e EVIDENCE {id: :id} }', parameters: { id: feedback.result.memory_refs[0] } })
    expect(read.result[0].result).toEqual(['agent_statement'])
  })

  it('forgets completely and blocks re-ingestion', async () => {
    const runtime = testEnv(new FakeAi([plan([prefers('dark', 'ColorScheme', '2026-01-01T00:00:00.000Z')])]))
    const space = uniqueSpace('mi-forget')
    await declareTypes(space, ['ColorScheme'])
    const source = await stage(runtime, space, 'g1', 'I prefer dark mode', '2026-01-01T00:00:00.000Z')
    await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:g1', input: { source_ref: source } })
    const [claim] = await claims(runtime, space, 'dark')
    const forget = await memory(runtime, space, { operation: 'forget', idempotency_key: 'forget:g1', input: { target_ref: claim!.id, mode: 'semantic' } })
    expect(forget.status).toBe('succeeded')
    expect(forget.progress.disposition).toBe('erased')
    expect(forget.result.status).toBe('completed')
    const retained = await call(runtime, space, `memory/plans/${forget.result.plan_ref as string}`)
    expect(retained.result.plan.status).toBe('completed')
    expect(await claims(runtime, space, 'dark')).toEqual([])
    const restage = await call(runtime, space, 'memory/sources', { messages: [{ role: 'user', content: 'I prefer dark mode' }], observed_at: '2026-01-01T00:00:00.000Z', idempotency_key: 'g1-again' })
    expect(restage.status).toBe(404)
  })

  it('surfaces a scoped constraint within budget and expands its basis', async () => {
    const constraint = `MUTATE {
      CREATE CONCEPT ?rule {
        TYPE "Insight" NAME "No Friday deploys"
        SET ATTRIBUTES {summary: "Never deploy on Fridays", insight_class: "constraint"}
        SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts}
      }
    }`
    const ai = new FakeAi([plan([constraint])])
    const runtime = testEnv(ai)
    const space = uniqueSpace('mi-recall')
    const source = await stage(runtime, space, 'c1', 'Never deploy on Fridays', '2026-09-20T00:00:00.000Z')
    const observed = await memory(runtime, space, { operation: 'observe', idempotency_key: 'observe:c1', scope: { task_ref: 'release' }, input: { source_ref: source } })
    expect(observed.progress.disposition).toBe('formed')

    ai.push({ commands: [] }, { answer: 'Plan for 2026-09-25.', found: false, uncertainty: 0.5 })
    const action = await memory(runtime, space, { operation: 'recall', scope: { task_ref: 'release' }, input: { query: 'Help me schedule the deployment on 2026-09-25', mode: 'action', after: [observed.receipt.receipt_ref] } })
    const rule = action.result.items.find((item: any) => item.role === 'constraint')
    expect(rule?.text).toContain('Fridays')
    expect(action.result.coverage.channels.constraints).toBe('complete')
    // What recall returned reaches the next Maintenance cycle once.
    const stub = env.BRAIN.getByName(space) as unknown as { maintenanceAssessment(): Promise<Record<string, any>> }
    const assessment = await stub.maintenanceAssessment()
    expect(assessment.exposures?.some((tally: any) => tally.retrieved > 0)).toBe(true)
    expect((await stub.maintenanceAssessment()).exposures).toBeUndefined()

    ai.push({ commands: [] }, { answer: 'x', found: false, uncertainty: 1 })
    const tiny = await memory(runtime, space, { operation: 'recall', scope: { task_ref: 'release' }, budget: { max_output_tokens: 20 }, input: { query: 'deploy?', mode: 'action' } })
    expect(tiny.error.code).toBe('ResultLimitExceeded')

    const expanded = await memory(runtime, space, { operation: 'recall', input: { target_ref: action.result.basis_ref, detail: 'evidence' } })
    expect(expanded.result.details.coverage.plans.constraints.method).toBe('exact')
    expect(expanded.result.details.elements.length).toBeGreaterThan(0)
    expect(expanded.result.coverage.action_eligible).toBe(false)
  })
})

describe('Memory Interface regressions', () => {
  it('keeps a scope whose handle a maintenance plan archived', async () => {
    const rule = (name: string) => `MUTATE {
      CREATE CONCEPT ?rule {
        TYPE "Insight" NAME "${name}"
        SET ATTRIBUTES {summary: "${name}", insight_class: "constraint"}
        SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts}
      }
    }`
    const ai = new FakeAi([]), runtime = testEnv(ai), space = uniqueSpace('mi-scope-handle')
    const scope = { task_ref: 'release' }
    const observe = async (key: string, name: string) => {
      ai.push(plan([rule(name)]))
      const source = await stage(runtime, space, key, name, '2026-09-20T00:00:00.000Z')
      const observed = await memory(runtime, space, { operation: 'observe', idempotency_key: `observe:${key}`, scope, input: { source_ref: source } })
      expect(observed.progress.disposition).toBe('formed')
    }
    const constraints = async () => {
      ai.push({ commands: [] }, { answer: 'Plan it.', found: true, uncertainty: 0.2 })
      const recalled = await memory(runtime, space, { operation: 'recall', scope, input: { query: 'When can I deploy?', mode: 'action' } })
      return recalled.result.items.filter((item: any) => item.role === 'constraint').length
    }
    await observe('r1', 'Never deploy on Fridays')
    // The maintenance gate admits a bounded Event selection; the handle is an Event.
    const archived = await call(runtime, space, 'execute_kip', { command: 'TRANSITION ?e TO "archived" WHERE { ?e {type: "Event"} } LIMIT 20' })
    expect(archived.result[0].status).toBe('succeeded')
    expect(await constraints()).toBe(1)
    await observe('r2', 'Never deploy after 5pm')
    expect(await constraints()).toBe(2)
  })

  it('does not erase another source after retrying rejected feedback with the same key', async () => {
    const runtime = testEnv(new FakeAi([])), space = uniqueSpace('mi-feedback-retry')
    const original = await stage(runtime, space, 'old', 'Original feedback', '2026-01-01T00:00:00.000Z')
    const replacement = await stage(runtime, space, 'new', 'Corrected feedback', '2026-01-02T00:00:00.000Z')
    const refused = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'retry',
      input: { source_ref: original, decision_ref: 'X-999' } })
    expect(refused.error.code).toBe('NotFoundOrNotVisible')
    const accepted = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'retry',
      input: { source_ref: replacement } })
    expect(accepted.status).toBe('succeeded')
    const id = accepted.result.memory_refs[0]
    const readPayload = async () => {
      const read = await call(runtime, space, 'execute_kip_readonly', {
        command: 'FIND(?e.payload) WHERE { ?e EVIDENCE {id: :id} } LIMIT 1', parameters: { id },
      })
      expect(read.result[0].status).toBe('succeeded')
      return read.result[0].result
    }
    const before = await readPayload()
    expect(JSON.stringify(before)).toContain('Corrected feedback')
    const forgotten = await memory(runtime, space, { operation: 'forget', idempotency_key: 'forget-old',
      input: { target_ref: original, mode: 'payload_only' } })
    expect(forgotten.result.status).toBe('completed')
    expect(await readPayload()).toEqual(before)
    const ownForget = await memory(runtime, space, { operation: 'forget', idempotency_key: 'forget-new',
      input: { target_ref: replacement, mode: 'payload_only' } })
    expect(ownForget.result.status).toBe('completed')
    expect(JSON.stringify(await readPayload())).not.toContain('Corrected feedback')
  })

  it('cancels the recall pass when its deadline budget runs out', async () => {
    let signal: AbortSignal | undefined
    const ai: AiBinding = {
      async run(_model, input, options) {
        // The plan answers at once; the answer outlives the budget.
        if (input.max_tokens === 900) return { response: { commands: [] } }
        signal = options?.signal
        return new Promise(() => {})
      },
    }
    const response = await memory(testEnv(ai), uniqueSpace('mi-deadline'), { operation: 'recall', budget: { deadline_ms: 300 }, input: { query: 'anything?' } })
    expect(response.status).toBe('partial')
    expect(response.result.uncertainties.join(' ')).toContain('the recall pass did not complete')
    expect(signal?.aborted).toBe(true)
  })

  it('stages only RFC 3339 instants that name their offset', async () => {
    const runtime = testEnv(new FakeAi([])), space = uniqueSpace('mi-instant')
    for (const observed_at of ['2026-09-27T10:00:00', 'Thu, 01 Jan 2026 00:00:00 GMT']) {
      const staged = await call(runtime, space, 'memory/sources', { messages: [{ role: 'user', content: 'x' }], observed_at, idempotency_key: observed_at })
      expect(staged.status).toBe(400)
    }
    const offset = await call(runtime, space, 'memory/sources', { messages: [{ role: 'user', content: 'x' }], observed_at: '2026-09-27T12:00:00+02:00', idempotency_key: 'offset' })
    expect(offset.status).toBe(200)
    expect((await call(runtime, space, `memory/sources/${offset.result.source_ref as string}`)).result.observed_at).toBe('2026-09-27T10:00:00.000Z')
  })

  it('keeps the most optional items that fit the output budget', async () => {
    const ai = new FakeAi([]), runtime = testEnv(ai), space = uniqueSpace('mi-budget')
    for (let i = 0; i < 6; i++) {
      const created = await call(runtime, space, 'execute_kip', {
        command: 'CREATE CONCEPT ?e {TYPE "Experience" NAME :name SET ATTRIBUTES {goal: "ship the release", summary: :summary, outcome_status: "success"}}',
        parameters: { name: `deploy run ${i}`, summary: `The deploy run ${i} went fine` },
      })
      expect(created.result[0].status).toBe('succeeded')
    }
    const ask = async (max: number) => {
      ai.push({ commands: [] }, { answer: 'Deploys went fine.', found: true, uncertainty: 0.2 })
      return memory(runtime, space, { operation: 'recall', budget: { max_output_tokens: max }, input: { query: 'deploy' } })
    }
    const full = await ask(65_536)
    expect(full.result.items.filter((item: any) => item.role === 'experience').length).toBeGreaterThan(1)
    expect(full.result.coverage.channels.experiences).toBe('complete')
    const size = new TextEncoder().encode(JSON.stringify(full.result)).byteLength
    const trimmed = await ask(size - 1)
    expect(trimmed.result.items).toHaveLength(full.result.items.length - 1)
    expect(trimmed.result.coverage.channels.experiences).toBe('incomplete')
  })

  it('replays staging with an implicit observation time', async () => {
    const runtime = testEnv(new FakeAi([])), space = uniqueSpace('mi-implicit-time')
    const input = { messages: [{ role: 'user', content: 'same source' }], idempotency_key: 'retry' }
    const first = await call(runtime, space, 'memory/sources', input)
    await new Promise(resolve => setTimeout(resolve, 5))
    const again = await call(runtime, space, 'memory/sources', input)
    expect(again.status).toBe(200)
    expect(again.result).toEqual(first.result)
    expect((await call(runtime, space, 'memory/sources', { ...input, messages: [{ role: 'user', content: 'changed' }] })).status).toBe(409)
  })

  it('does not return scoped feedback to another task', async () => {
    const ai = new FakeAi([]), runtime = testEnv(ai), space = uniqueSpace('mi-feedback-scope')
    const source = await stage(runtime, space, 's', 'Private task A feedback', '2026-01-01T00:00:00.000Z')
    const feedback = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'f', scope: { task_ref: 'task-a' }, input: { source_ref: source } })
    const id = feedback.result.memory_refs[0]
    ai.push({ commands: [`FIND(?e) WHERE { ?e EVIDENCE {id: "${id}"} } LIMIT 1`] }, { answer: 'Private task A feedback', found: true, uncertainty: 0 })
    const other = await memory(runtime, space, { operation: 'recall', scope: { task_ref: 'task-b' }, input: { query: 'What feedback?' } })
    expect(other.result.items).toEqual([])
    expect(other.result.summary).not.toContain('Private task A feedback')
    const original = await call(runtime, space, 'execute_kip', { command: 'CREATE EVIDENCE ?e { SET FIELDS {evidence_class: "tool_result", payload: "tool feedback", observed_at: "2026-01-01T00:00:00.000Z"} }' })
    expect(original.result[0].status, JSON.stringify(original)).toBe('succeeded')
    const copied = await memory(runtime, space, { operation: 'feedback', idempotency_key: 'tool-feedback', scope: { task_ref: 'task-a' }, input: { source_ref: original.result[0].extensions['kip-do/outcome'].handles.e } })
    const read = await call(runtime, space, 'execute_kip_readonly', { command: 'FIND(?e.evidence_class) WHERE { ?e EVIDENCE {id: :id} } LIMIT 1', parameters: { id: copied.result.memory_refs[0] } })
    expect(read.result[0].result).toEqual(['tool_result'])
  })

  it('erases a source-owned constraint without deleting shared identities', async () => {
    const ai = new FakeAi([plan([`MUTATE {
      UPSERT CONCEPT ?person { MATCH {type: "Person", key: "shared-person"} SET FIELDS {name: "Alice"} }
      CREATE CONCEPT ?rule { TYPE "Insight" NAME "No Friday deploys"
        SET ATTRIBUTES {summary: "Never deploy on Fridays", insight_class: "constraint"}
        SET FACET "MemoryScope" {task_ref: :scope_task, context_refs: :contexts} }
    }`])])
    const runtime = testEnv(ai), space = uniqueSpace('mi-source-concept')
    const source = await stage(runtime, space, 's', 'Never deploy on Fridays', '2026-01-01T00:00:00.000Z')
    const observed = await memory(runtime, space, { operation: 'observe', idempotency_key: 'o', scope: { task_ref: 'release' }, input: { source_ref: source } })
    expect(observed.status).toBe('succeeded')
    const forgotten = await memory(runtime, space, { operation: 'forget', idempotency_key: 'f', input: { target_ref: source, mode: 'semantic' } })
    expect(forgotten.result.status).toBe('completed')
    const rules = await call(runtime, space, 'execute_kip_readonly', { command: 'FIND(?c) WHERE { ?c {type: "Insight"} } LIMIT 5' })
    expect(rules.result[0].result).toEqual([])
    const people = await call(runtime, space, 'execute_kip_readonly', { command: 'FIND(?c) WHERE { ?c {key: "shared-person"} } LIMIT 5' })
    expect(people.result[0].result).toHaveLength(1)
  })

  it('replaces an answer based on an invalidated extraction', async () => {
    const ai = new FakeAi([plan([prefers('vegetarian', 'Diet', '2026-01-01T00:00:00.000Z')])])
    const runtime = testEnv(ai), space = uniqueSpace('mi-repaired-summary')
    await declareTypes(space, ['Diet'])
    const source = await stage(runtime, space, 's', 'Alice talked about dinner', '2026-01-01T00:00:00.000Z')
    await memory(runtime, space, { operation: 'observe', idempotency_key: 'o', input: { source_ref: source } })
    const [wrong] = await claims(runtime, space, 'vegetarian')
    const report = await stage(runtime, space, 'r', 'You misheard me', '2026-09-01T00:00:00.000Z')
    ai.push(plan([]))
    expect((await memory(runtime, space, { operation: 'revise', idempotency_key: 'r', input: { source_ref: report, target_ref: wrong!.id, change_kind: 'misrecorded' } })).status).toBe('succeeded')
    ai.push({ commands: [`FIND(?a) WHERE { ?a ASSERTION {id: "${wrong!.id}"} } LIMIT 1`] }, { answer: 'You are vegetarian.', found: true, uncertainty: 0 })
    const recalled = await memory(runtime, space, { operation: 'recall', input: { query: 'What diet?' } })
    expect(recalled.result.summary).not.toBe('You are vegetarian.')
    expect(recalled.warnings.length).toBeGreaterThan(0)
  })
})
