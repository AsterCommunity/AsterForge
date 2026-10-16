// Offline index structure/revision checks; never infer consumer runtime or rollout success.
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const required = ['capability / owning crate', 'Forge revision', 'consumer / revision / lock source',
  'integration status', 'adapter / call sites', 'validation evidence', 'rollout evidence', 'next issue']
let baseline
for (const page of ['docs/guide/reference-projects.md', 'docs/en/guide/reference-projects.md']) {
  const source = readFileSync(resolve(root, page), 'utf8')
  for (const field of required) {
    const row = source.split('\n').find((line) => line.startsWith(`| ${field} |`))
    if (!row || !row.slice(`| ${field} |`.length).replace(/\|/g, '').trim()) {
      throw new Error(`${page}: missing evidence field ${field}`)
    }
  }
  const revisions = {}
  for (const repo of ['AsterDrive', 'AsterForge']) {
    const links = [...source.matchAll(new RegExp(
      `https://github\\.com/AsterCommunity/${repo}/commit/([a-f0-9]{40})`, 'g'
    ))]
    const values = new Set(links.map((match) => match[1]))
    if (values.size !== 1) throw new Error(`${page}: expected one pinned ${repo} revision`)
    revisions[repo] = [...values][0]
  }
  if (baseline && JSON.stringify(baseline) !== JSON.stringify(revisions)) {
    throw new Error('Bilingual consumer evidence revisions differ')
  }
  baseline = revisions
  for (const match of source.matchAll(
    /https:\/\/github\.com\/AsterCommunity\/(AsterDrive|AsterForge)\/blob\/([a-f0-9]{40})\/([^\s)#]+)/g
  )) {
    const [, repo, sha, path] = match
    if (sha !== revisions[repo]) throw new Error(`${page}: source link outside declared ${repo} baseline`)
    if (repo === 'AsterForge') {
      execFileSync('git', ['cat-file', '-e', `${sha}:${path}`], { cwd: root, stdio: 'pipe' })
    }
    // Consumer paths/lock are checked in Drive; no sibling checkout or network is required here.
  }
}
console.log(`Consumer evidence OK: ${JSON.stringify(baseline)}; consumer runtime/rollout not checked`)
