// One isolated seed through the actual production grpc-web -> daemon -> provider.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { fromJson } from '@bufbuild/protobuf'
import { Daemon, SaveBlueprintRequestSchema } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'
import { join } from 'node:path'

const ready = JSON.parse(await readFile(process.argv[2], 'utf8'))
const spec = JSON.parse(await readFile(process.argv[3], 'utf8'))
const destination = process.argv[4]
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const workspacePath = ready.workspace
const report = { id: spec.id, seed_digest: spec.seed_digest, model: ready.model, variant: ready.variant, started_utc: new Date().toISOString(), events: [], checks: {}, status: 'RUNNING' }
const controller = new AbortController()
function encode(value) { return JSON.stringify(value, (_, v) => typeof v === 'bigint' ? v.toString() : v, 2) }
async function resource(method, payload, expectedRevision = 0) {
  const result = await client[method]({ schemaVersion: 1, requestId: randomUUID(), workspacePath, expectedRevision: BigInt(expectedRevision), payloadJson: JSON.stringify(payload) })
  if (result.errorJson) throw Error(result.errorJson)
  return JSON.parse(result.payloadJson)
}
const tasks = async () => (await resource('queryContinuations', { operation: 'list', after: null })).items
let sessionId
let waitingId
let stopSent = false
let approvalCount = 0
try {
  await client.openWorkspace({ path: workspacePath })
  let blueprintId
  if (spec.mode === 'blueprint') {
    const body = JSON.parse(await readFile(new URL('./fixtures/nx-local-review-request.json', import.meta.url), 'utf8'))
    body.workspace_path = workspacePath
    for (const node of body.blueprint.nodes) {
      if (node.kind === 'CallLLM') node.data_json = JSON.stringify({ max_tokens: 512, model: ready.model, prompt: 'Read evidence.txt using ReadFile. Return the exact synthetic evidence text. Do not edit, start jobs, or request another blueprint.' })
    }
    const source = JSON.parse(body.file_json)
    for (const node of source.nodes) {
      const executable = body.blueprint.nodes.find(item => item.id === node.id)
      if (executable) node.data = JSON.parse(executable.data_json || '{}')
    }
    body.file_json = JSON.stringify(source)
    await mkdir(join(workspacePath, 'blueprints'), { recursive: true })
    blueprintId = body.blueprint.id
    await client.saveBlueprint(fromJson(SaveBlueprintRequestSchema, body))
  }
  const options = { provider: 'openai-chat', model: ready.model, temperature: 0, max_tokens: spec.limits.max_tokens, max_iterations: spec.limits.max_iterations, permission_mode: spec.mode === 'deny' ? 'ask' : 'sandbox' }
  if (spec.checks.continuation) options.continuation_policy = { detached: true, max_turns: spec.limits.max_turns, token_limit: spec.limits.token_limit, expires_ms: Date.now() + 180000, purpose: spec.goal.slice(0, 500) }
  if (spec.mode === 'stale_prompt') {
    const id = randomUUID()
    const first = await resource('writePromptResource', { scope: 'project', draft: { id, name: 'Synthetic prompt', tags: [], body: 'SYNTHETIC_OLD_PROMPT' } })
    const selected = await resource('selectPromptResource', { reference: first.reference, purpose: { kind: 'chat' } })
    await resource('writePromptResource', { scope: 'project', draft: { id, name: 'Synthetic prompt', tags: [], body: 'SYNTHETIC_NEW_PROMPT' } }, first.reference.revision)
    options.prompt_snapshots = [selected.id]
  }
  let message = spec.goal.replaceAll('{blueprint_id}', blueprintId || '').replaceAll('{delay_command}', ready.delay_command)
  message = message.replace(/\{test_command:([^}]+)\}/g, (_, name) => ready.test_command.replace('{file}', name))
  if (spec.checks.continuation) {
    // Resolve the fixture's relative deadline on the host. Models have no clock
    // tool and must never invent a Unix timestamp for the absolute API field.
    const now = Date.now()
    message += `\nSynthetic host clock: current Unix milliseconds ${now}; for AwaitEvents use deadline_ms ${now + 60000}, mode "any", and params {}. Use only the source returned by the host.`
  }
  report.resolved_message = message
  try {
    const stream = client.sendChat({ workspacePath, message, optionsJson: JSON.stringify(options) }, { timeoutMs: 160000, signal: controller.signal })
    for await (const event of stream) {
      // Reasoning remains in the daemon; ordinary result files do not duplicate it.
      if (!event.kind.includes('reasoning')) report.events.push({ kind: event.kind, content: event.content, detail: event.detailJson ? JSON.parse(event.detailJson) : null })
      if (event.kind === 'session') sessionId = JSON.parse(event.detailJson).session_id
      if (event.kind === 'waiting') {
        waitingId = JSON.parse(event.detailJson).task_id
        if (spec.checks.stop_on_wait) {
          const task = (await tasks()).find(x => x.task.id === waitingId)
          await resource('controlContinuation', { id: waitingId, action: 'stop' }, task.revision)
          stopSent = true
        }
      }
      if (event.kind === 'approval') {
        approvalCount++
        const detail = JSON.parse(event.content)
        const requestId = JSON.parse(event.detailJson).request_id
        if (detail.request_type === 'blueprint_launch') {
          if (spec.checks.launch === 'stale') {
            const graph = await client.loadBlueprint({ workspacePath, blueprintId })
            graph.name += ' changed after proposal'
            const filePath = 'blueprints/review.blueprint'
            const source = JSON.parse(await readFile(join(workspacePath, filePath), 'utf8'))
            source.name = graph.name
            await client.saveBlueprint({ workspacePath, blueprint: graph, filePath, fileJson: JSON.stringify(source) })
          }
          const payload = { request_id: requestId, intent_id: detail.intent_id, original_digest: detail.digest, allow: true, resume_on_completion: spec.checks.launch !== 'report_only', allow_source_read: false }
          try {
            await resource('confirmBlueprintLaunch', payload)
            report.checks.launch_confirmation_recorded = true
          } catch (error) {
            if (spec.checks.launch !== 'stale') throw error
            if (!/stale|changed|version|manifest/i.test(String(error))) throw error
            report.checks.stale_launch_rejected = true
            report.checks.stale_launch_error = String(error)
            await client.respondApproval({ workspacePath, requestId, decision: 'DenyOnce' }).catch(() => {})
          }
        } else {
          // Synthetic test commands only; never approve arbitrary model text.
          const command = detail.command || detail.subject || ''
          const namedTest = spec.checks.test || spec.checks.executed_test
          const allowedTest = namedTest ? ready.test_command.replace('{file}', namedTest) : null
          const allow = spec.mode !== 'deny' && typeof command === 'string' && (command === ready.delay_command || command === allowedTest)
          await client.respondApproval({ workspacePath, requestId, decision: allow ? 'AllowOnce' : 'DenyOnce' })
          report.checks.approval_denied ||= !allow
        }
      }
    }
  } catch (error) {
    if (spec.mode !== 'stale_prompt') throw error
    report.checks.stale_prompt_rejected = String(error).toLowerCase().includes('changed') || String(error).toLowerCase().includes('revision')
    if (!report.checks.stale_prompt_rejected) throw error
  }
  if (spec.checks.continuation) {
    const until = Date.now() + 150000
    let last
    while (Date.now() < until) {
      const found = await tasks()
      last = found.find(t => t.task.id === waitingId) || found[0]
      if (last && ['completed', 'stopped', 'needs_attention'].includes(last.task.state)) break
      await new Promise(resolve => setTimeout(resolve, 100))
    }
    report.task = last
    if (spec.checks.reconnect && last) {
      const freshClient = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
      const value = await freshClient.queryContinuations({ schemaVersion: 1, requestId: randomUUID(), workspacePath, payloadJson: JSON.stringify({ operation: 'read', id: last.task.id }) })
      report.checks.reconnected_same_task = JSON.parse(value.payloadJson).task.id === last.task.id
    }
  }
  if (!sessionId && report.task) sessionId = report.task.task.session
  if (sessionId) {
    const session = await client.getChatSession({ workspacePath, sessionId })
    report.saved_history = JSON.parse(session.historyJson || '[]')
    report.session_id = sessionId
  }
  const audit = await client.listAuditLog({ workspacePath })
  report.audit = audit.entries.map(e => ({ operation: e.operation, detail: JSON.parse(e.detailJson || '{}') }))
  report.executions = (await client.listExecutions({ workspacePath })).executions.map(e => ({ id: e.runId, status: e.status }))
  report.approval_count = approvalCount
  report.checks.stop_sent = stopSent
  report.status = 'EXECUTED_ASSERTIONS_PENDING'
} catch (error) {
  report.status = 'DRIVER_ERROR'
  report.error = String(error)
} finally {
  controller.abort()
  if (report.status === 'DRIVER_ERROR') await client.abortChat({ workspacePath }, { timeoutMs: 5000 }).catch(() => {})
  await client.closeWorkspace({ path: workspacePath }, { timeoutMs: 10000 }).catch(() => {})
  report.finished_utc = new Date().toISOString()
  await writeFile(destination, encode(report))
  console.log(JSON.stringify({ id: spec.id, model: ready.model, variant: ready.variant, status: report.status, event_count: report.events.length, evidence: destination }))
  process.exit(report.status === 'DRIVER_ERROR' ? 1 : 0)
}
