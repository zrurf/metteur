// The retained R8 binaries read a closed synthetic current DB at the same path.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [oldReady, newReady, restoreResult, output] = process.argv.slice(2)
await mkdir(output)
const old = JSON.parse(await readFile(oldReady, 'utf8')),
  current = JSON.parse(await readFile(newReady, 'utf8')),
  original = JSON.parse(await readFile(restoreResult, 'utf8'))
assert.equal(original.status, 'PASSED')
const workspace = original.workspace
assert(workspace.includes('s4-cold-restore-fixture-fix'))
for (const ready of [old, current]) assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = (ready) => createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const legacy = client(old),
  candidate = client(current)
const report = {
  started: new Date().toISOString(),
  oldReady,
  newReady,
  workspace,
  checks: {},
  error: null,
}
async function copies() {
  for (const copy of original.copies) {
    const response = await candidate.recovery({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({
        scope: 'project',
        action: { operation: 'read', id: copy.input.id },
      }),
    })
    assert.equal(response.errorJson, '')
    assert.deepEqual(JSON.parse(response.payloadJson).record, copy)
  }
}
try {
  const bytes = await readFile(join(workspace, 'saved.txt'))
  await candidate.openWorkspace({ path: workspace })
  await copies()
  const before = await candidate.getChatSession({
    workspacePath: workspace,
    sessionId: original.sessionId,
  })
  await candidate.closeWorkspace({ path: workspace })
  await legacy.openWorkspace({ path: workspace })
  const history = await legacy.getChatSession({
    workspacePath: workspace,
    sessionId: original.sessionId,
  })
  assert(history.historyJson.includes('Earlier saved conversation remains intact'))
  assert(
    (await legacy.readFile({ workspacePath: workspace, path: 'saved.txt' })).content.includes(
      'Original saved content',
    ),
  )
  const executions = await legacy.listExecutions({ workspacePath: workspace })
  assert(executions.executions.some((run) => run.runId === original.runId))
  await assert.rejects(
    legacy.recovery({
      schemaVersion: 1,
      requestId: randomUUID(),
      workspacePath: workspace,
      payloadJson: JSON.stringify({ scope: 'project', action: { operation: 'capabilities' } }),
    }),
    (error) => error.code === 12,
  )
  report.checks.r8ReadsExistingFileConversationAndRunWithoutRecoveryCapability = true
  await legacy.closeWorkspace({ path: workspace })
  await candidate.openWorkspace({ path: workspace })
  await copies()
  const after = await candidate.getChatSession({
    workspacePath: workspace,
    sessionId: original.sessionId,
  })
  assert.equal(after.historyJson, before.historyJson)
  assert.equal(after.transcriptJson, before.transcriptJson)
  assert.deepEqual(await readFile(join(workspace, 'saved.txt')), bytes)
  report.checks.returnToCurrentRetainsFourDraftKindsTranscriptAndExactSavedBytes = true
  await candidate.closeWorkspace({ path: workspace })
  report.status = 'PASSED'
} catch (error) {
  report.status = 'FAILED'
  report.error = String(error)
  process.exitCode = 1
} finally {
  report.finished = new Date().toISOString()
  await writeFile(join(output, 'result.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify(report))
}
