import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const read = (path) => readFileSync(fileURLToPath(new URL(path, import.meta.url)), 'utf8')
const registry = JSON.parse(read('../src/previews/next/registry.json'))
assert.equal(registry.previews.length, 7)
assert.equal(new Set(registry.previews.map(p => p.id)).size, 7)
if (registry.approval) {
  assert.equal(registry.approval.feedback, '两份 r1 都批准接入')
  assert(registry.approval.sourceCommit && registry.approval.entrySha256)
  assert.equal(registry.integrationScope.length, 2)
} else assert.deepEqual(registry.integrationScope, [])
assert(!read('../src/router.ts').includes('previews/next'))
assert(!read('../src/main.ts').includes('previews/next'))
assert(read('../src/previews/next/index.html').includes("connect-src 'none'"))
assert(!read('../vite.next-preview.config.ts').includes('proxy:'))
console.log('NX preview registry and entry isolation: PASS (approval still pending)')
