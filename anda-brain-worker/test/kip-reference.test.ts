import { describe, expect, it } from 'vitest'
import { AiResponseError, createMutationPlan, createRecallAnswer, createRecallPlan } from '../src/ai.js'
import { readReference, REFERENCE_PAGE_BYTES } from '../src/kip-reference.js'
import { REFERENCE_DOCUMENTS, REFERENCE_VERSION } from '../src/references.generated.js'
import type { AiBinding } from '../src/types.js'

const request = { document: 'syntax', section: 'kql', offset: 0 }
const mutation = { types: [], predicates: [], commands: [], summary: '' }
const answer = { answer: '', found: false, uncertainty: 1 }

class SequenceAi implements AiBinding {
  calls: Record<string, unknown>[] = []
  constructor(private responses: unknown[]) {}
  async run(_model: string, input: Record<string, unknown>) {
    this.calls.push(structuredClone(input))
    if (!this.responses.length) throw new Error('unexpected AI call')
    return { response: this.responses.shift(), usage: { input_tokens: 10, output_tokens: 5 } }
  }
}

describe('embedded KIP references', () => {
  it('reassembles every compiled document and verifies release hashes without I/O', async () => {
    expect(REFERENCE_VERSION).toBe('0.14.2')
    expect(REFERENCE_DOCUMENTS).toHaveLength(24)
    for (const doc of REFERENCE_DOCUMENTS) {
      let offset: number | null = 0
      let text = ''
      while (offset !== null) {
        const result = readReference({ document: doc.id, section: null, offset })
        expect(new TextEncoder().encode(result.content).length).toBeLessThanOrEqual(REFERENCE_PAGE_BYTES)
        if (result.next_offset !== null) expect(result.next_offset).toBeGreaterThan(offset)
        text += result.content
        offset = result.next_offset
      }
      expect(text).toBe(doc.content)
      const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text))
      expect(Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('')).toBe(doc.sha256)
    }
  })

  it('discovers documents and sections, and refuses invalid identifiers and UTF-8 offsets', () => {
    expect(readReference({ document: 'index', section: null, offset: 0 }).content).toContain('specification: SPECIFICATION.md')
    const index = readReference({ document: 'syntax', section: 'index', offset: 0 }).content
    expect(index).toContain('2. KQL — Read')
    expect(readReference(request).content).toContain('2. KQL — Read')
    expect(readReference({ ...request, section: '2. KQL — Read' })).toMatchObject({ content: readReference(request).content })
    for (const section of ['kml', 'meta', 'envelope']) expect(readReference({ ...request, section }).content.length).toBeGreaterThan(0)
    for (const document of ['../SPECIFICATION.md', 'https://example.com', '__proto__', 'unknown']) {
      expect(() => readReference({ ...request, document })).toThrow('unknown reference document')
    }
    expect(() => readReference({ ...request, section: 'no-such-heading' })).toThrow('unknown or ambiguous')
    for (const offset of [-1, 0.5, Number.MAX_SAFE_INTEGER, 4]) {
      expect(() => readReference({ ...request, section: null, offset })).toThrow('offset')
    }
    expect(() => readReference({ ...request, operation: 'arm_watch' })).toThrow('invalid reference')
  })

  it('serves lookup rounds in mutation, recall planning and recall answering, summing usage', async () => {
    for (const [run, placeholder, final] of [
      [createMutationPlan, mutation, { ...mutation, summary: 'deferred' }],
      [createRecallPlan, { commands: [] }, { commands: [] }],
      [createRecallAnswer, answer, { answer: 'No supported fact.', found: false, uncertainty: 1 }],
    ] as const) {
      const ai = new SequenceAi([{ ...placeholder, references: [request] }, final])
      const result = await run(ai, 'fixture', [])
      expect(result.usage).toEqual({ input_tokens: 20, output_tokens: 10 })
      expect(ai.calls).toHaveLength(2)
      const messages = ai.calls[1]!.messages as { role: string; content: string }[]
      const receipt = JSON.parse(messages[0]!.content.split('\n\n# Embedded reference lookup result\n').at(-1)!)
      expect(receipt.kind).toBe('embedded_protocol_references')
      expect(receipt.results[0].reference.content).toContain('2. KQL — Read')
      expect(receipt).not.toHaveProperty('evidence')
      expect(receipt).not.toHaveProperty('memories')
      expect(receipt).not.toHaveProperty('coverage')
    }
  })

  it('preserves the one-call path when no references are requested', async () => {
    const ai = new SequenceAi([{ ...mutation, summary: 'nothing to store' }])
    expect((await createMutationPlan(ai, 'fixture', [])).value.summary).toBe('nothing to store')
    expect(ai.calls).toHaveLength(1)
  })

  it('rejects mixed reference and action/answer responses before returning a plan', async () => {
    for (const fields of [{ commands: ['PURGE ?x WHERE { ?x CONCEPT {} }'] },
      { types: ['NewType'] }, { runtime: [{ operation: 'arm_watch' }] }, { digests: { digest_x: {} } }, { summary: 'done' }]) {
      const ai = new SequenceAi([{ ...mutation, ...fields, references: [request] }])
      await expect(createMutationPlan(ai, 'fixture', [])).rejects.toThrow('cannot be combined')
      expect(ai.calls).toHaveLength(1)
    }
    const ai = new SequenceAi([{ ...answer, answer: 'protocol is a remembered fact', references: [request] }])
    await expect(createRecallAnswer(ai, 'fixture', [])).rejects.toThrow('cannot be combined')
  })

  it('bounds round and page counts and retains usage on exhaustion', async () => {
    for (const batches of [[1, 1, 1, 1], [4, 4, 1], [5]]) {
      const ai = new SequenceAi(batches.map(count => ({ commands: [], references: Array(count).fill(request) })))
      try {
        await createRecallPlan(ai, 'fixture', [])
        throw new Error('expected exhaustion')
      } catch (error) {
        expect(error).toBeInstanceOf(AiResponseError)
        expect((error as AiResponseError).message).toContain('budget exhausted')
        expect((error as AiResponseError).usage).toEqual({ input_tokens: batches.length * 10, output_tokens: batches.length * 5 })
      }
      expect(ai.calls).toHaveLength(batches.length)
    }
  })

  it('returns bounded errors for unavailable references so the model can recover', async () => {
    const ai = new SequenceAi([{ commands: [], references: [{ ...request, document: 'unknown' }] }, { commands: [] }])
    await createRecallPlan(ai, 'fixture', [])
    const messages = ai.calls[1]!.messages as { content: string }[]
    const receipt = JSON.parse(messages[0]!.content.split('\n\n# Embedded reference lookup result\n').at(-1)!)
    expect(receipt.results[0].error).toContain('unknown reference document')
    expect(receipt.remaining_pages).toBe(7)
  })

  it('retains all model usage if the final plan fails validation after a reference round', async () => {
    const ai = new SequenceAi([{ commands: [], references: [request] }, { commands: 42 }])
    await expect(createRecallPlan(ai, 'fixture', [])).rejects.toMatchObject({
      name: 'AiResponseError', usage: { input_tokens: 20, output_tokens: 10 },
    })
  })
})
