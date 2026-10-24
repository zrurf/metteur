// Cold, non-destructive recovery drill in a new synthetic directory only.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { mkdir, writeFile, readFile, cp, rename, readdir } from 'node:fs/promises'
import { join, resolve, relative, isAbsolute } from 'node:path'
import { createHash, randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [baseURL, destination] = process.argv.slice(2)
assert.equal(new URL(baseURL).hostname, '127.0.0.1')
const output = resolve(destination),
  workspace = join(output, 'workspace'),
  backup = join(output, 'cold-copy'),
  displaced = join(output, 'preserved-before-restore')
await mkdir(output)
await mkdir(workspace)
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
const report = {
  baseURL,
  workspace,
  backup,
  displaced,
  started: new Date().toISOString(),
  mode: 'cold synthetic same-path restore; no user project or paid provider',
  checks: {},
  copies: [],
  error: null,
}
const encode = (value) =>
  JSON.stringify(value, (_, item) => (typeof item === 'bigint' ? item.toString() : item))
async function recovery(action) {
  const response = await client.recovery({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ scope: 'project', action }),
  })
  assert.equal(response.errorJson, '')
  return JSON.parse(response.payloadJson)
}
async function manifest(root, at = '') {
  const files = {}
  for (const entry of await readdir(join(root, at), { withFileTypes: true })) {
    const name = join(at, entry.name)
    if (entry.isDirectory()) Object.assign(files, await manifest(root, name))
    else if (entry.isFile())
      files[name.replaceAll('\\', '/')] = createHash('sha256')
        .update(await readFile(join(root, name)))
        .digest('hex')
  }
  return files
}
try {
  await writeFile(join(workspace, 'saved.txt'), '\uFEFFOriginal saved content 中文\r\n')
  await client.openWorkspace({ path: workspace })
  const graph = await client.compileDsl({
    workspacePath: workspace,
    source: 'entry start: Start\nend: End\nstart -> end',
  })
  await client.saveBlueprint({
    workspacePath: workspace,
    blueprint: graph,
    filePath: 'flow.blueprint',
  })
  let runId = '',
    sessionId = ''
  for await (const event of client.executeBlueprint({
    workspacePath: workspace,
    blueprintId: graph.id,
  })) {
    assert.notEqual(event.kind, 'error')
  }
  const executions = await client.listExecutions({ workspacePath: workspace })
  const execution = executions.executions.find((run) => run.blueprintId === graph.id)
  assert(execution)
  runId = execution.runId
  assert.equal(execution.status.toLowerCase(), 'completed')
  for await (const event of client.sendChat({
    workspacePath: workspace,
    message: 'Synthetic earlier conversation',
    historyJson: '[]',
    optionsJson: JSON.stringify({
      provider: 'mock',
      mock_text: 'Earlier saved conversation remains intact',
      max_tokens: 128,
    }),
  })) {
    assert.notEqual(event.kind, 'error')
    if (event.kind === 'session') sessionId = JSON.parse(event.detailJson).session_id
  }
  assert(sessionId)
  const earlierChat = encode(await client.getChatSession({ workspacePath: workspace, sessionId }))
  const earlierGraph = encode(
    await client.loadBlueprint({ workspacePath: workspace, blueprintId: graph.id }),
  )
  for (const [kind, entity, raw] of [
    ['text', 'saved.txt', 'raw text { 中文\r\n'],
    ['canvas', 'flow.blueprint', '{invalid canvas 中文\r\n'],
    ['chat', sessionId, 'unsent main conversation 中文'],
    ['supervision', runId, 'unsent independent supervision 中文'],
  ]) {
    const input = {
      id: randomUUID(),
      window_id: randomUUID(),
      kind,
      entity,
      edit_sequence: 1,
      raw,
      baseline_identity: null,
      selections: [],
      pending_submission: null,
      selected_model: null,
      view: null,
    }
    const ack = await recovery({ operation: 'backup', input })
    assert.equal(ack.input.raw, raw)
    report.copies.push(ack)
  }
  const tasks = await client.queryContinuations({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ operation: 'list', after: null }),
  })
  assert.equal(tasks.errorJson, '')
  assert.equal(JSON.parse(tasks.payloadJson).items.length, 0)
  report.checks.directRunDidNotInventContinuation = true
  await client.closeWorkspace({ path: workspace })
  await cp(workspace, backup, { recursive: true, errorOnExist: true, force: false })
  const files = await manifest(backup)
  await writeFile(join(output, 'cold-copy-sha256.json'), JSON.stringify(files, null, 2))
  await writeFile(
    join(workspace, 'saved.txt'),
    'Synthetic later mutation retained in displaced directory\n',
  )
  for (const path of [workspace, backup, displaced]) {
    const inside = relative(output, resolve(path))
    assert(
      inside && !inside.startsWith('..') && !isAbsolute(inside),
      'Restore paths must remain inside the newly created evidence directory',
    )
  }
  await rename(workspace, displaced)
  await cp(backup, workspace, { recursive: true, errorOnExist: true, force: false })
  assert.deepEqual(await manifest(workspace), files)
  report.checks.coldCopyBytesVerified = true
  await client.openWorkspace({ path: workspace })
  for (const ack of report.copies) {
    const restored = await recovery({ operation: 'read', id: ack.input.id })
    assert.deepEqual(restored.record, ack)
    assert.equal(restored.restores_permissions, false)
  }
  report.checks.fourDraftKindsRetained = true
  assert.equal(
    encode(await client.getChatSession({ workspacePath: workspace, sessionId })),
    earlierChat,
  )
  assert.equal(
    encode(await client.loadBlueprint({ workspacePath: workspace, blueprintId: graph.id })),
    earlierGraph,
  )
  assert.equal(
    await readFile(join(workspace, 'saved.txt'), 'utf8'),
    '\uFEFFOriginal saved content 中文\r\n',
  )
  assert((await readFile(join(displaced, 'saved.txt'), 'utf8')).includes('later mutation'))
  report.checks.existingConversationBlueprintAndSavedFileRetained = true
  report.checks.laterMutationPreservedSeparately = true
  report.runId = runId
  report.sessionId = sessionId
  await client.closeWorkspace({ path: workspace })
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
