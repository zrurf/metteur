// Capture retained data through the real gateway before/after a runtime upgrade.
// No Send, Run, provider request, recovery write or file rewrite is performed.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, readdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [readyPath, outputPath, baselinePath, mode] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8'))
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const rpc = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const encode = (value) =>
  JSON.stringify(value, (_, v) => (typeof v === 'bigint' ? v.toString() : v), 2)
const plain = (value) => JSON.parse(encode(value))
const sha = (value) => createHash('sha256').update(value).digest('hex')
const baseline =
  baselinePath && baselinePath !== '-' ? JSON.parse(await readFile(baselinePath, 'utf8')) : null
if (baseline) for (const item of baseline.workspaces) await rpc.openWorkspace({ path: item.path })
else
  for (const path of ready.retainedWorkspaces ?? [ready.workspace])
    await rpc.openWorkspace({ path })
const listed = (await rpc.listWorkspaces({})).workspaces
const snapshot = { recordedUtc: new Date().toISOString(), baseURL: ready.baseURL, workspaces: [] }
async function resource(method, workspacePath, payload) {
  const response = await rpc[method]({
    workspacePath,
    schemaVersion: 1,
    requestId: randomUUID(),
    payloadJson: JSON.stringify(payload),
  })
  assert.equal(response.errorJson, '', `${method} ${workspacePath}`)
  return JSON.parse(response.payloadJson)
}
async function pages(method, workspacePath, envelope) {
  const items = []
  let after = null
  do {
    const payload =
      method === 'recovery'
        ? { ...envelope, action: { operation: 'list', after } }
        : { operation: 'list', after }
    const page = await resource(method, workspacePath, payload)
    items.push(...page.items)
    const next = page.next ?? null
    assert(!next || next !== after, 'Pagination did not advance')
    after = next
  } while (after)
  return items
}
async function files(root, at = '') {
  const hashes = {}
  for (const entry of await readdir(join(root, at), { withFileTypes: true })) {
    if (['.metteur', '.git', 'node_modules', '__pycache__'].includes(entry.name)) continue
    const relative = join(at, entry.name)
    if (entry.isDirectory()) Object.assign(hashes, await files(root, relative))
    else if (entry.isFile())
      hashes[relative.replaceAll('\\', '/')] = sha(await readFile(join(root, relative)))
  }
  return hashes
}
for (const entry of listed.sort((a, b) => a.path.localeCompare(b.path))) {
  const path = entry.path
  const histories = []
  for (const session of (await rpc.listChatSessions({ workspacePath: path })).sessions) {
    const record = await rpc.getChatSession({ workspacePath: path, sessionId: session.sessionId })
    histories.push({ id: session.sessionId, hash: sha(record.historyJson), record: plain(record) })
  }
  histories.sort((a, b) => a.id.localeCompare(b.id))
  const recovery = []
  for (const scope of ['project', 'personal']) {
    for (const item of await pages('recovery', path, { scope })) {
      if (!item.discarded)
        recovery.push({
          scope,
          record: (
            await resource('recovery', path, { scope, action: { operation: 'read', id: item.id } })
          ).record,
        })
    }
  }
  recovery.sort((a, b) => (a.scope + a.record.input.id).localeCompare(b.scope + b.record.input.id))
  const tasks = await pages('queryContinuations', path)
  const executions = plain((await rpc.listExecutions({ workspacePath: path })).executions)
  snapshot.workspaces.push({
    path,
    histories,
    recovery,
    tasks,
    executions,
    files: await files(path),
  })
}
await writeFile(resolve(outputPath), encode(snapshot))
if (baseline) {
  for (const before of baseline.workspaces) {
    const after = snapshot.workspaces.find((item) => item.path === before.path)
    assert(after, `Workspace missing: ${before.path}`)
    assert.deepEqual(after.files, before.files, 'Saved file content changed')
    assert.deepEqual(after.histories, before.histories, 'Original conversation changed')
    assert.deepEqual(after.recovery, before.recovery, 'Acknowledged draft or position changed')
    for (const old of before.tasks) {
      const kept = after.tasks.find((item) => item.task.id === old.task.id)
      assert(kept, 'Task missing')
      assert.deepEqual(kept.task.budget, old.task.budget, 'Budget changed')
      assert.deepEqual(kept.task.sources, old.task.sources, 'Source/event ledger changed')
    }
    for (const old of before.executions)
      assert(
        after.executions.some((item) => item.runId === old.runId && item.status === old.status),
        'Execution missing or changed',
      )
  }
}
if (mode === '--close') {
  for (const item of snapshot.workspaces) {
    assert(
      item.executions.every(
        (run) => !['running', 'paused', 'starting'].includes(run.status.toLowerCase()),
      ),
      'Active execution prevents maintenance',
    )
    assert(
      item.tasks.every((record) => !record.task.dispatch),
      'Dispatch prevents maintenance',
    )
  }
  for (const item of snapshot.workspaces) await rpc.closeWorkspace({ path: item.path })
}
console.log(
  encode({
    snapshot: outputPath,
    baselineMatched: Boolean(baseline),
    closed: mode === '--close',
    workspaces: snapshot.workspaces.map((w) => ({
      path: w.path,
      chats: w.histories.length,
      drafts: w.recovery.length,
      tasks: w.tasks.length,
      runs: w.executions.length,
      files: Object.keys(w.files).length,
    })),
  }),
)
