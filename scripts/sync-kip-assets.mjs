#!/usr/bin/env node
/**
 * Re-syncs everything this repository vendors from `anda_kip`.
 *
 * Two kinds of file, and the difference is why this script exists.
 *
 * **Verbatim.** The role cards, `KIPSyntax.md` and
 * `CognitiveMemoryProfile-2.0.md` are the protocol's, not ours. The Rust service
 * reads them straight out of the crate (`agents::prompts::mode_reference()` and
 * `memory_runtime`), so it keeps no copy. `@ldclabs/kip-do` ships none of them,
 * so the Worker keeps copies — and a copy nobody can refresh is how a model
 * ends up writing against a contract that moved.
 *
 * **Half ours.** Each `Brain*.md` is the KIP 2.0 reference policy for one mode
 * plus a section A describing *this* deployment. The reference half is
 * upstream's and must track it; section A is ours and must survive. Splitting
 * on the `# A.` heading lets both be true, which the previous "diff them by
 * hand when the reference policies change" note did not: between KIP 2.0
 * `40e655f` landing and this script, Watch, WorkingState, DerivationState,
 * `MnemonicState.utility`, `LIST DEPENDENTS` and `PURGE PAYLOAD` were in the
 * reference policies and in none of our five copies of them.
 *
 * Uses the sibling `anda-db` checkout by default. Set `ANDA_KIP_SOURCE` to an
 * `anda_kip` crate directory to sync against a published release instead.
 */
import { copyFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const kip = resolve(process.env.ANDA_KIP_SOURCE ?? resolve(root, '..', 'anda-db', 'rs', 'anda_kip'))

if (!existsSync(kip)) {
  console.error(`anda_kip source not found at ${kip}; nothing to sync from`)
  process.exit(1)
}

/** Copied byte for byte; the Worker is the only consumer that needs a copy. */
const VERBATIM = [
  ...['KIPFormation.md', 'KIPRecall.md', 'KIPMaintenance.md', 'KIPVocabulary.md', 'MemoryInterface.md'].map(
    (name) => [join(kip, 'brain', name), `anda-brain-worker/assets/${name}`],
  ),
  [join(kip, 'KIPSyntax.md'), 'anda-brain-worker/assets/KIPSyntax.md'],
  [
    join(kip, 'profiles', 'CognitiveMemoryProfile-2.0.md'),
    'anda-brain-worker/assets/CognitiveMemoryProfile-2.0.md',
  ],
]

/** The deployments whose mode prompts carry a reference half. */
const DEPLOYMENTS = ['anda_brain/assets', 'anda-brain-worker/assets']
const POLICIES = ['BrainFormation.md', 'BrainRecall.md', 'BrainMaintenance.md']

/** Where the reference policy ends and this deployment's contract begins. */
const CONTRACT = /\n---\n\n# A\. /

let changed = 0

for (const [source, target] of VERBATIM) {
  const destination = join(root, target)
  if (!existsSync(source)) {
    console.error(`missing upstream file: ${source}`)
    process.exit(1)
  }
  if (existsSync(destination) && readFileSync(source, 'utf8') === readFileSync(destination, 'utf8')) {
    continue
  }
  copyFileSync(source, destination)
  console.log(`copied ${target}`)
  changed += 1
}

for (const policy of POLICIES) {
  const source = join(kip, 'brain', policy)
  if (!existsSync(source)) {
    console.error(`missing upstream policy: ${source}`)
    process.exit(1)
  }
  // Upstream ends at "# 36. Final Principle"; anything after a `# A.` heading
  // there would be a deployment contract in the protocol repo, which is not a
  // thing. Trimmed to one trailing newline so the join is exact.
  const reference = readFileSync(source, 'utf8').replace(/\s+$/, '\n')

  for (const deployment of DEPLOYMENTS) {
    const destination = join(root, deployment, policy)
    const current = readFileSync(destination, 'utf8')
    const match = CONTRACT.exec(current)
    if (!match) {
      // Refused rather than overwritten: without the boundary this script
      // cannot tell the deployment contract from the reference half, and
      // guessing would delete the half that is ours.
      console.error(
        `${deployment}/${policy} has no "# A." deployment contract heading; ` +
          'sync it by hand and restore the boundary',
      )
      process.exit(1)
    }
    const merged = reference + current.slice(match.index + 1)
    if (merged === current) continue
    writeFileSync(destination, merged)
    console.log(`re-synced ${deployment}/${policy}`)
    changed += 1
  }
}

if (changed === 0) {
  console.log('every vendored asset is already current')
} else {
  console.log(`\n${changed} file(s) updated — now run:`)
  console.log('  pnpm --filter @ldclabs/anda-brain-worker run codegen:prompts')
}
