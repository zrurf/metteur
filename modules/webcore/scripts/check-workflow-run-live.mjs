import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, readdir } from 'node:fs/promises'
import { join } from 'node:path'
import assert from 'node:assert/strict'
const [readyPath, output] = process.argv.slice(2),
  ready = JSON.parse(await readFile(readyPath, 'utf8'))
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL })),
  workspacePath = ready.workspace
const report = { checks: [], events: [] }
async function rpc(method, payload, id = crypto.randomUUID(), revision = 0) {
  const r = await client[method]({
    workspacePath,
    schemaVersion: 1,
    requestId: id,
    expectedRevision: BigInt(revision),
    payloadJson: JSON.stringify(payload),
  })
  assert.equal(r.errorJson, '')
  return JSON.parse(r.payloadJson)
}
try {
  const name = (await readdir(workspacePath))
    .filter((p) => p.startsWith('Live review ') && p.endsWith('.blueprint'))
    .sort()
    .at(-1)
  assert(name, 'Use the saved production UI instance')
  const graph = JSON.parse(await readFile(join(workspacePath, name), 'utf8'))
  await client.openWorkspace({ path: workspacePath })
  const prepared = await rpc('queryCodingWorkflows', { operation: 'prepare_run', blueprint_id: graph.id })
  let run
  for await (const event of client.executeBlueprint(
    { workspacePath, blueprintId: graph.id, blueprintJson: JSON.stringify(graph), workflowConfirmation: prepared.digest },
    { timeoutMs: 60000 },
  )) {
    const detail = JSON.parse(event.detailJson || '{}')
    if (detail.run_id) run = detail.run_id
    report.events.push({ kind: event.kind, message: event.message, detail })
    assert.notEqual(event.kind, 'error', event.message)
    if (event.kind === 'approval_request') {
      const allowed = JSON.stringify(detail).includes('process.exit(0)')
      await client.respondApproval({
        workspacePath,
        requestId: detail.request_id,
        decision: allowed ? 'AllowOnce' : 'DenyOnce',
      })
    }
  }
  assert(run)
  const review = await rpc('queryCodingWorkflows', { operation: 'review', run_id: run })
  assert.equal(review.source.kind, 'independent')
  assert.deepEqual(
    review.criteria.map((c) => c.status),
    ['passed', 'passed', 'needs_review'],
  )
  report.checks.push(
    'saved ordinary graph executed real commands; host projection has version-bound checks and independent provenance',
  )
  const input = {
      run_id: run,
      evidence_digest: review.digest,
      criterion_id: 'scope',
      decision: 'accepted',
      note: 'Local fixture scoped file reviewed',
    },
    id = crypto.randomUUID()
  const receipt = await rpc('recordWorkflowReview', input, id, review.revision)
  assert.equal(receipt.receipt.revision, 1)
  const replay = await rpc('queryCodingWorkflows', { operation: 'receipt', request_id: id })
  assert.deepEqual(replay, receipt)
  assert.equal(
    (await rpc('queryCodingWorkflows', { operation: 'review', run_id: run })).criteria.find(
      (c) => c.id === 'scope',
    ).decision.current,
    true,
  )
  report.checks.push(
    'human decision persisted and original receipt reconciled through real grpc-web',
  )
  const original = await readFile(join(workspacePath, 'app.ts'), 'utf8')
  await writeFile(join(workspacePath, 'app.ts'), original + '// evidence changed\n')
  const changed = await rpc('queryCodingWorkflows', { operation: 'review', run_id: run })
  assert.equal(changed.criteria[1].status, 'stale')
  assert.equal(changed.criteria.find((c) => c.id === 'scope').decision.current, false)
  report.checks.push(
    'subsequent file change invalidated current acceptance without rewriting the historical receipt',
  )
} catch (e) {
  report.failure = String(e)
}
await writeFile(output, JSON.stringify(report, null, 2))
console.log(JSON.stringify({ checks: report.checks, failure: report.failure }))
if (report.failure) process.exitCode = 1
