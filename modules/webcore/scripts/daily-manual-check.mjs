// Prepare or collect the two remaining human scenarios. Never starts a run/model.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { randomUUID, createHash } from 'node:crypto'
import assert from 'node:assert/strict'

const [readyPath, destination, mode = 'prepare'] = process.argv.slice(2)
assert(['prepare', 'collect'].includes(mode))
const ready = JSON.parse(await readFile(readyPath, 'utf8')),
  root = resolve(destination)
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const encode = (value) =>
  JSON.stringify(value, (_, v) => (typeof v === 'bigint' ? v.toString() : v), 2)
let manual
if (mode === 'prepare') {
  await mkdir(root)
  const workspace = join(root, 'workspace')
  await mkdir(workspace)
  await writeFile(join(workspace, 'app.ts'), 'export const manualAcceptance = "AC23-AC27-only"\n')
  await writeFile(join(workspace, 'sleep-draft.txt'), '这是磁盘原文；请追加未保存的验收文字。\n')
  await client.openWorkspace({ path: workspace })
  const blueprints = []
  for (const [filePath, name, ms] of [
    ['input-guard.blueprint', '输入法占用检查（两分钟，Stop后十秒内收尾）', 120000],
    ['sleep-wake.blueprint', '睡眠唤醒检查（等待90秒）', 90000],
  ]) {
    const count = filePath === 'input-guard.blueprint' ? 12 : 1
    const delays = Array.from({ length: count }, (_, index) => `delay${index}`)
    const flow = ['start', ...delays, 'end']
    const source = [
      'entry start: Start',
      ...delays.map((id) => `${id}: Delay(Ms = ${ms / count})`),
      'end: End',
      ...flow.slice(0, -1).map((id, index) => `${id} -> ${flow[index + 1]}`),
    ].join('\n')
    const graph = await client.compileDsl({
      workspacePath: workspace,
      source,
    })
    graph.name = name
    graph.nodes.forEach((node, index) => {
      node.posX = index * 320
      node.posY = 180
    })
    await client.saveBlueprint({ workspacePath: workspace, blueprint: graph, filePath })
    blueprints.push({ id: graph.id, filePath, name, ms })
  }
  manual = {
    workspace,
    baseURL: ready.baseURL,
    url: `${ready.baseURL}/?open=${encodeURIComponent(workspace)}`,
    readyPath,
    blueprints,
    preparedUtc: new Date().toISOString(),
    scope:
      'Actual Windows IME and actual S3 sleep/wake only; existing accepted stories are not repeated. Delay nodes have no shell, model or file side effects.',
  }
  await writeFile(join(root, 'manual.json'), encode(manual))
} else manual = JSON.parse(await readFile(join(root, 'manual.json'), 'utf8'))

const workspacePath = manual.workspace
async function resource(method, payload) {
  const response = await client[method]({
    workspacePath,
    schemaVersion: 1,
    requestId: randomUUID(),
    payloadJson: JSON.stringify(payload),
  })
  assert.equal(response.errorJson, '')
  return JSON.parse(response.payloadJson)
}
const recovery = await resource('recovery', {
  scope: 'project',
  action: { operation: 'list', after: null },
})
const copies = []
for (const item of recovery.items.filter((entry) => !entry.discarded))
  copies.push(
    await resource('recovery', { scope: 'project', action: { operation: 'read', id: item.id } }),
  )
const sessions = (await client.listChatSessions({ workspacePath })).sessions
const histories = []
for (const session of sessions)
  histories.push(await client.getChatSession({ workspacePath, sessionId: session.sessionId }))
const disk = await readFile(join(workspacePath, 'sleep-draft.txt'))
const snapshot = {
  recordedUtc: new Date().toISOString(),
  mode,
  workspacePath,
  executions: (await client.listExecutions({ workspacePath })).executions,
  tasks: await resource('queryContinuations', { operation: 'list', after: null }),
  recovery: copies,
  recoveryMetadata: recovery.items,
  histories,
  audit: (await client.listAuditLog({ workspacePath })).entries,
  savedFileSha256: createHash('sha256').update(disk).digest('hex'),
  savedFileText: disk.toString('utf8'),
}
const file = mode === 'prepare' ? 'before.json' : `after-${Date.now()}.json`
await writeFile(join(root, file), encode(snapshot))
console.log(
  JSON.stringify({
    mode,
    url: manual.url,
    evidence: root,
    snapshot: file,
    runs: snapshot.executions.length,
    tasks: snapshot.tasks.items.length,
    histories: histories.length,
  }),
)
