#!/usr/bin/env node
// Supplement the public anda_kip document constants with files from the same
// release. This is a build-time maintainer operation; runtime never reads files.
import { createHash } from 'node:crypto'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const source = process.env.ANDA_KIP_SOURCE
if (!source) throw new Error('Set ANDA_KIP_SOURCE to the published anda_kip crate directory')
const manifest = readFileSync(join(source, 'Cargo.toml'), 'utf8')
const version = manifest.match(/\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)?.[1]
// The release the build resolves, not the Cargo.toml requirement: the supplement
// must come from the anda_kip the service is compiled against.
const pinned = readFileSync(join(root, 'Cargo.lock'), 'utf8')
  .match(/\nname = "anda_kip"\nversion = "([^"]+)"/)?.[1]
if (!version || version !== pinned) {
  throw new Error(`Source version ${version} must match the anda_kip ${pinned} in Cargo.lock`)
}
const files = [
  'SPECIFICATION.md', 'Invariants.md', 'brain/ExperienceLearningArchitecture.md',
  'grammar/KQL.ebnf', 'grammar/KML.ebnf', 'grammar/META.ebnf',
  'schemas/kip-request.schema.json', 'schemas/kip-response.schema.json',
]
const destination = join(root, 'anda_brain/assets/kip-reference')
const hashes = {}
// Read every input before writing anything, so a missing source cannot cause a
// partially refreshed bundle. Hash the crate-exported context as provenance too.
const contents = files.map(file => [file, readFileSync(join(source, file))])
for (const file of [...files, 'KIPSyntax.md', 'profiles/CognitiveMemoryProfile-2.0.md',
  'brain/KIPRecall.md', 'brain/KIPFormation.md', 'brain/KIPMaintenance.md', 'brain/KIPVocabulary.md',
  'Cognitive-Consistency.md', 'Memory-Interface.md', 'brain/MemoryInterface.md',
  'brain/Brain-Runtime.md', 'brain/Validated-Learning.md', 'schemas/kip-common.schema.json',
  'schemas/kip-projection.schema.json', 'schemas/kip-memory.schema.json',
  'schemas/kip-cognitive-records.schema.json', 'schemas/kip-element.schema.json',
  'schemas/kip-schema-package.schema.json']) {
  hashes[file] = createHash('sha256').update(readFileSync(join(source, file))).digest('hex')
}
const provenance = JSON.stringify({ source: 'anda_kip', version, sha256: hashes }, null, 2) + '\n'
contents.push(['manifest.json', Buffer.from(provenance)])
if (process.argv.includes('--worker')) {
  const worker = join(root, 'anda-brain-worker')
  // anda_kip owns the reference version; kip-do has an independent release cadence.
  const ids = {
    'SPECIFICATION.md': 'specification', 'Invariants.md': 'invariants',
    'brain/ExperienceLearningArchitecture.md': 'experience-learning',
    'grammar/KQL.ebnf': 'grammar-kql', 'grammar/KML.ebnf': 'grammar-kml', 'grammar/META.ebnf': 'grammar-meta',
    'schemas/kip-request.schema.json': 'schema-request', 'schemas/kip-response.schema.json': 'schema-response',
    'KIPSyntax.md': 'syntax', 'profiles/CognitiveMemoryProfile-2.0.md': 'profile',
    'brain/KIPRecall.md': 'recall', 'brain/KIPFormation.md': 'formation', 'brain/KIPMaintenance.md': 'maintenance',
    'brain/KIPVocabulary.md': 'vocabulary',
    'Cognitive-Consistency.md': 'consistency', 'Memory-Interface.md': 'memory-interface',
    'brain/MemoryInterface.md': 'memory-interface-card', 'brain/Brain-Runtime.md': 'brain-runtime',
    'brain/Validated-Learning.md': 'validated-learning', 'schemas/kip-common.schema.json': 'schema-common',
    'schemas/kip-projection.schema.json': 'schema-projection',
    'schemas/kip-memory.schema.json': 'schema-memory', 'schemas/kip-cognitive-records.schema.json': 'schema-records',
    'schemas/kip-element.schema.json': 'schema-element', 'schemas/kip-schema-package.schema.json': 'schema-package',
  }
  const constants = {
    syntax: ['KIP_SYNTAX', 'KIPSyntax.md'], profile: ['COGNITIVE_MEMORY_PROFILE', 'CognitiveMemoryProfile-2.0.md'],
    recall: ['KIP_RECALL_CARD', 'KIPRecall.md'], formation: ['KIP_FORMATION_CARD', 'KIPFormation.md'],
    vocabulary: ['KIP_VOCABULARY_CARD', 'KIPVocabulary.md'],
    maintenance: ['KIP_MAINTENANCE_CARD', 'KIPMaintenance.md'], 'memory-interface-card': ['MEMORY_AGENT_CARD', 'MemoryInterface.md'],
  }
  const documents = Object.entries(hashes).map(([file, sha256]) => {
    const id = ids[file]
    if (!id) throw new Error(`Missing document ID: ${file}`)
    const existing = constants[id]
    return { id, source: file, sha256, ...(existing
      ? { constant: existing[0], asset: existing[1] }
      : { content: readFileSync(join(source, file), 'utf8') }) }
  })
  contents.push(['../../../anda-brain-worker/assets/kip-reference.json',
    Buffer.from(JSON.stringify({ version, documents }, null, 2) + '\n')])
}
for (const [file, content] of contents) {
  const target = join(destination, file)
  if (process.argv.includes('--check')) {
    if (!readFileSync(target).equals(content)) throw new Error(`Reference drift: ${file}`)
  } else {
    mkdirSync(dirname(target), { recursive: true })
    writeFileSync(target, content)
  }
}
console.log(`KIP reference ${version}: ${process.argv.includes('--check') ? 'verified' : 'generated'} from ${resolve(source)}`)
