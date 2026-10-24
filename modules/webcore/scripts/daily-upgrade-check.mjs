// Verify forward upgrade and cold-copy rollback with preserved new and old work.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { mkdir, readFile, writeFile, cp, rename } from 'node:fs/promises'
import { join, resolve, dirname, relative, isAbsolute } from 'node:path'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [oldReadyPath, currentReadyPath, destination] = process.argv.slice(2)
const oldReady = JSON.parse(await readFile(oldReadyPath, 'utf8')),
  currentReady = JSON.parse(await readFile(currentReadyPath, 'utf8')),
  output = resolve(destination)
await mkdir(output)
const workspace = oldReady.workspace,
  parent = dirname(output),
  oldBackup = join(output, 'r8-before-upgrade'),
  upgraded = join(output, 'upgraded-preserved'),
  fallback = join(output, 'r8-fallback-preserved')
for (const path of [workspace, oldBackup, upgraded, fallback]) {
  const inside = relative(parent, resolve(path))
  assert(inside && !inside.startsWith('..') && !isAbsolute(inside))
}
assert(workspace.includes('s4-r8-compatibility'))
for (const ready of [oldReady, currentReady])
  assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const connect = (ready) => createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL })),
  old = connect(oldReady),
  current = connect(currentReady)
const report = {
  started: new Date().toISOString(),
  workspace,
  output,
  oldReadyPath,
  currentReadyPath,
  checks: {},
  error: null,
}
async function recovery(action) {
  const response = await current.recovery({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ scope: 'project', action }),
  })
  assert.equal(response.errorJson, '')
  return JSON.parse(response.payloadJson)
}
try {
  await old.openWorkspace({ path: workspace })
  const id = randomUUID().slice(0, 8),
    path = `compatibility-${id}.txt`,
    body = 'Original R8 saved file 中文\r\n'
  await old.writeFile({ workspacePath: workspace, path, content: body })
  let sessionId = ''
  for await (const event of old.sendChat({
    workspacePath: workspace,
    message: `Original R8 conversation ${id}`,
    historyJson: '[]',
    optionsJson: JSON.stringify({
      provider: 'mock',
      mock_text: 'Preserved older conversation',
      max_tokens: 128,
    }),
  })) {
    assert.notEqual(event.kind, 'error')
    if (event.kind === 'session') sessionId = JSON.parse(event.detailJson).session_id
  }
  assert(sessionId)
  const graph = await old.compileDsl({
    workspacePath: workspace,
    source: 'entry start: Start\nend: End\nstart -> end',
  })
  await old.saveBlueprint({
    workspacePath: workspace,
    blueprint: graph,
    filePath: `compatibility-${id}.blueprint`,
  })
  for await (const event of old.executeBlueprint({
    workspacePath: workspace,
    blueprintId: graph.id,
  }))
    assert.notEqual(event.kind, 'error')
  const execution = (await old.listExecutions({ workspacePath: workspace })).executions.find(
    (run) => run.blueprintId === graph.id,
  )
  assert(execution)
  const before = await old.getChatSession({ workspacePath: workspace, sessionId })
  await old.closeWorkspace({ path: workspace })
  await cp(workspace, oldBackup, { recursive: true, force: false, errorOnExist: true })
  report.checks.upgradeStartsFromVerifiedR8ConversationFileAndRun = true
  await current.openWorkspace({ path: workspace })
  const newer = await current.getChatSession({ workspacePath: workspace, sessionId })
  assert.deepEqual(JSON.parse(newer.historyJson), JSON.parse(before.historyJson))
  assert(
    (await current.listExecutions({ workspacePath: workspace })).executions.some(
      (run) => run.runId === execution.runId,
    ),
  )
  assert.equal(await readFile(join(workspace, path), 'utf8'), body)
  const input = {
    id: randomUUID(),
    window_id: randomUUID(),
    kind: 'text',
    entity: path,
    edit_sequence: 1,
    raw: 'New protected draft after upgrading R8 中文\r\n',
    baseline_identity: null,
    selections: [],
    pending_submission: null,
    selected_model: null,
    view: null,
  }
  const ack = await recovery({ operation: 'backup', input })
  await writeFile(
    join(output, 'protected-input-export.json'),
    JSON.stringify(
      { source_workspace: workspace, records: [ack], permissions_restored: false },
      null,
      2,
    ),
  )
  report.checks.currentReadsOldDataAndProtectsNewDraftWithoutReset = true
  await current.closeWorkspace({ path: workspace })
  await rename(workspace, upgraded)
  await cp(oldBackup, workspace, { recursive: true, force: false, errorOnExist: true })
  await old.openWorkspace({ path: workspace })
  assert(
    (await old.listExecutions({ workspacePath: workspace })).executions.some(
      (run) => run.runId === execution.runId,
    ),
  )
  assert.deepEqual(
    JSON.parse((await old.getChatSession({ workspacePath: workspace, sessionId })).historyJson),
    JSON.parse(before.historyJson),
  )
  await old.writeFile({
    workspacePath: workspace,
    path,
    content: 'Later R8 fallback edit retained separately\r\n',
  })
  await old.closeWorkspace({ path: workspace })
  report.checks.r8RollbackUsesItsColdCopyAndReadsItsOriginalRun = true
  await rename(workspace, fallback)
  await cp(upgraded, workspace, { recursive: true, force: false, errorOnExist: true })
  await current.openWorkspace({ path: workspace })
  assert.deepEqual((await recovery({ operation: 'read', id: input.id })).record, ack)
  assert.equal(await readFile(join(workspace, path), 'utf8'), body)
  assert.equal(
    await readFile(join(fallback, path), 'utf8'),
    'Later R8 fallback edit retained separately\r\n',
  )
  assert.deepEqual(
    JSON.parse((await current.getChatSession({ workspacePath: workspace, sessionId })).historyJson),
    JSON.parse(before.historyJson),
  )
  await current.closeWorkspace({ path: workspace })
  report.checks.returnToCurrentRetainsACKAndSeparateFallbackEdit = true
  report.ack = ack
  report.sessionId = sessionId
  report.runId = execution.runId
  report.status = 'PASSED'
} catch (error) {
  report.status = 'FAILED'
  report.error = String(error)
  process.exitCode = 1
} finally {
  report.finished = new Date().toISOString()
  await writeFile(join(output, 'result.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify({ status: report.status, checks: report.checks, error: report.error }))
}
