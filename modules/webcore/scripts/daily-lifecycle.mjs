// Public production RPCs and real bounded Jobs; lifecycle evidence, not a runner.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { localDelayCommand } from './local-job-fixture.mjs'
import { readSse } from '../src/core/sse.ts'
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import assert from 'node:assert/strict'

const [readyPath, output, mode = 'ordinary-and-close'] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8'))
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const report = { mode, started: new Date().toISOString(), readyPath, checks: {}, error: null }
async function resource(method, workspacePath, payload, revision = 0) {
  const r = await client[method]({
    schemaVersion: 1,
    requestId: randomUUID(),
    workspacePath,
    expectedRevision: BigInt(revision),
    payloadJson: JSON.stringify(payload),
  })
  assert.equal(r.errorJson, '')
  return JSON.parse(r.payloadJson)
}
async function until(read, accept) {
  const end = Date.now() + 10000
  let value
  do {
    value = await read()
    if (accept(value)) return value
    await new Promise((resolve) => setTimeout(resolve, 25))
  } while (Date.now() < end)
  throw Error(`Lifecycle condition not met: ${JSON.stringify(value)}`)
}
async function job(workspace, purpose) {
  await mkdir(workspace)
  await writeFile(join(workspace, 'ack.txt'), 'saved original\n')
  await client.openWorkspace({ path: workspace })
  const input = {
    id: randomUUID(),
    window_id: randomUUID(),
    kind: 'text',
    entity: 'ack.txt',
    edit_sequence: 1,
    raw: 'ACKed before lifecycle change 中文\r\n',
    baseline_identity: null,
    selections: [],
    pending_submission: null,
    selected_model: null,
    view: null,
  }
  const ack = await resource('recovery', workspace, {
    scope: 'project',
    action: { operation: 'backup', input },
  })
  const events = [],
    options = {
      provider: 'openai-chat',
      model: 'fixture',
      max_tokens: 512,
      max_iterations: 7,
      permission_mode: 'ask',
      continuation_policy: {
        detached: true,
        max_turns: 4,
        token_limit: 300000,
        expires_ms: Date.now() + 3600000,
        purpose,
      },
    }
  let taskId = ''
  for await (const event of client.sendChat({
    workspacePath: workspace,
    message: `M2_LOCAL_JOB: ${purpose}`,
    historyJson: '[]',
    optionsJson: JSON.stringify(options),
  })) {
    events.push(event)
    assert.notEqual(event.kind, 'error', event.content)
    if (event.kind === 'approval') {
      const detail = JSON.parse(event.content),
        id = JSON.parse(event.detailJson).request_id
      assert.equal(detail.command, localDelayCommand())
      await client.respondApproval({
        workspacePath: workspace,
        requestId: id,
        decision: 'AllowOnce',
      })
    }
    if (event.kind === 'waiting') taskId = JSON.parse(event.detailJson).task_id
  }
  assert(taskId)
  const task = await resource('queryContinuations', workspace, { operation: 'read', id: taskId })
  assert.equal(task.task.state, 'waiting')
  assert.equal(task.task.budget.used_turns, 1)
  assert.equal(Object.values(task.task.sources).filter((s) => s.kind === 'job').length, 1)
  return { workspace, ack, task, events }
}
try {
  if (['ordinary-and-close', 'ordinary-only', 'close-only'].includes(mode)) {
    await mkdir(output)
    if (mode !== 'close-only')
      for (const transport of ['grpc', 'sse']) {
        const workspace = join(output, `ordinary-${transport}`)
        await mkdir(workspace)
        await writeFile(join(workspace, 'disconnect.txt'), 'original')
        await client.openWorkspace({ path: workspace })
        const id = randomUUID(),
          abort = new AbortController(),
          events = []
        const input = {
          workspacePath: workspace,
          message: 'ordinary cancellation fixture',
          historyJson: '[]',
          optionsJson: JSON.stringify({
            provider: 'mock',
            mock_steps: [
              {
                tool_calls: [
                  {
                    name: 'EditFile',
                    arguments: {
                      path: 'disconnect.txt',
                      edits: [{ old_string: 'original', new_string: 'UNWANTED_AFTER_DISCONNECT' }],
                    },
                  },
                ],
              },
              { text: 'must not execute after observer disconnect' },
            ],
            mock_delay_ms: 3000,
            max_tokens: 128,
            max_iterations: 1,
            permission_mode: 'full',
            delivery: { id, expected_revision: 0 },
          }),
        }
        let accepted
        const seenSession = new Promise((resolve) => {
          accepted = resolve
        })
        const stream = (async () => {
          try {
            const receive = async function* () {
              if (transport === 'grpc') {
                yield* client.sendChat(input, { signal: abort.signal })
                return
              }
              const response = await fetch(`${ready.baseURL}/api/chat/stream`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                  workspace_path: workspace,
                  message: input.message,
                  history_json: input.historyJson,
                  options_json: input.optionsJson,
                }),
                signal: abort.signal,
              })
              assert(response.ok)
              for await (const message of readSse(response, abort.signal)) {
                const event = JSON.parse(message.data)
                yield { kind: event.kind, content: event.content, detailJson: event.detail_json }
              }
            }
            for await (const event of receive()) {
              events.push(event)
              if (event.kind === 'session') accepted(JSON.parse(event.detailJson).session_id)
            }
          } catch (error) {
            assert(abort.signal.aborted)
            return String(error)
          }
        })()
        const session = await seenSession
        await assert.rejects(
          client.closeWorkspace({ path: workspace }),
          /active execution or chat turn/,
        )
        report.checks.activeOrdinaryCloseRejected = true
        abort.abort()
        await stream
        const admission = await until(
          () => resource('chatDelivery', workspace, { operation: 'admission' }),
          (r) => r.state === 'idle',
        )
        const receipt = await resource('chatDelivery', workspace, { operation: 'read', id })
        const tasks = await resource('queryContinuations', workspace, {
          operation: 'list',
          after: null,
        })
        assert.equal(tasks.items.length, 0)
        assert.equal(receipt.receipt.phase, 'finished')
        assert.equal(
          await readFile(join(workspace, 'disconnect.txt'), 'utf8'),
          'original',
          'Disconnected ordinary chat must not execute its later tool call',
        )
        const replay = []
        for await (const event of client.sendChat(input)) replay.push(event)
        assert(replay.some((event) => event.kind === 'delivery_replay'))
        const after = await resource('chatDelivery', workspace, { operation: 'read', id })
        assert.equal(after.receipt.turn, receipt.receipt.turn)
        assert.equal(after.receipt.session, session)
        report.checks[`ordinary_${transport}_cancels_without_effects_or_replay`] = true
        report.ordinary ??= []
        report.ordinary.push({ transport, workspace, session, events, admission, receipt, replay })
        await client.closeWorkspace({ path: workspace })
      }
    if (mode !== 'ordinary-only') {
      const detached = await job(
        join(output, 'detached-close'),
        'Synthetic approved detached workspace close',
      )
      await client.closeWorkspace({ path: detached.workspace })
      await client.openWorkspace({ path: detached.workspace })
      const reopened = await resource('queryContinuations', detached.workspace, {
        operation: 'read',
        id: detached.task.task.id,
      })
      assert(['paused', 'needs_attention'].includes(reopened.task.state), JSON.stringify(reopened))
      assert.equal(reopened.task.budget.used_turns, detached.task.task.budget.used_turns)
      assert.equal(reopened.task.budget.consumed_tokens, detached.task.task.budget.consumed_tokens)
      const copy = await resource('recovery', detached.workspace, {
        scope: 'project',
        action: { operation: 'read', id: detached.ack.input.id },
      })
      assert.deepEqual(copy.record, detached.ack)
      await resource(
        'controlContinuation',
        detached.workspace,
        { id: reopened.task.id, action: 'stop' },
        reopened.revision,
      )
      await client.closeWorkspace({ path: detached.workspace })
      report.detached = { ...detached, reopened }
      report.checks.workspaceClosePreservesACKAndBudgetWithoutAutomaticDispatch = true
    }
  } else if (mode === 'detached-reconnect') {
    await mkdir(output)
    const before = await (await fetch(`${ready.providerUrl}/fixture`)).json()
    const started = await job(
      join(output, 'detached-reconnect'),
      'Synthetic approved detached reconnect',
    )
    const completed = await until(
      () =>
        resource('queryContinuations', started.workspace, {
          operation: 'read',
          id: started.task.task.id,
        }),
      (task) => task.task.state === 'completed',
    )
    assert.equal(completed.task.session, started.task.task.session)
    assert.equal(completed.task.budget.used_turns, 2)
    assert.equal(Object.values(completed.task.sources).length, 1)
    assert.equal(Object.values(completed.task.sources)[0].terminal, 'succeeded')
    assert.equal(
      completed.task.dispatch_history.filter((dispatch) => dispatch.state === 'committed').length,
      1,
    )
    const after = await (await fetch(`${ready.providerUrl}/fixture`)).json()
    assert.equal(after.calls - before.calls, 3)
    report.detached = { ...started, completed }
    report.checks.approvedDetachedJobResumesExactlyOnceWithoutObserver = true
    await client.closeWorkspace({ path: started.workspace })
  } else if (mode === 'prepare-restart') {
    await mkdir(output)
    const prepared = await job(
      join(output, 'restart-workspace'),
      'Synthetic approved detached daemon restart',
    )
    report.prepared = prepared
    report.ready = ready
    report.checks.durableBeforeOwnedDaemonExit = true
  } else if (mode === 'verify-restart') {
    const prepared = JSON.parse(
      await readFile(join(output, 'prepare-restart.json'), 'utf8'),
    ).prepared
    await client.openWorkspace({ path: prepared.workspace })
    const recovered = await resource('queryContinuations', prepared.workspace, {
      operation: 'read',
      id: prepared.task.task.id,
    })
    report.recovered = recovered
    assert(
      recovered.policy.expires_ms > Date.now(),
      'Restart guard must be verified before policy expiry',
    )
    assert(['ready', 'needs_attention'].includes(recovered.task.state))
    assert.equal(recovered.workflow.reason_code, 'effects_unknown')
    assert.equal(recovered.task.budget.used_turns, prepared.task.task.budget.used_turns)
    assert.equal(recovered.task.budget.consumed_tokens, prepared.task.task.budget.consumed_tokens)
    assert.equal(
      Object.values(recovered.task.sources).find((s) => s.kind === 'job').terminal,
      'indeterminate',
    )
    assert.equal(recovered.task.dispatch_history.length, prepared.task.task.dispatch_history.length)
    const copy = await resource('recovery', prepared.workspace, {
      scope: 'project',
      action: { operation: 'read', id: prepared.ack.input.id },
    })
    assert.deepEqual(copy.record, prepared.ack)
    await assert.rejects(
      resource(
        'controlContinuation',
        prepared.workspace,
        { id: recovered.task.id, action: 'resume' },
        recovered.revision,
      ),
    )
    const proof = await (await fetch(`${ready.providerUrl}/fixture`)).json()
    assert.equal(proof.calls, 0)
    const jobs = await client.listJobs({ workspacePath: prepared.workspace })
    assert.equal(jobs.jobs.length, 0)
    report.recovered = recovered
    report.checks.actualDaemonRestartRetainsACKBudgetAndUnknownSourceWithoutReplay = true
    await client.closeWorkspace({ path: prepared.workspace })
  } else throw Error('Unknown lifecycle mode')
  report.status = 'PASSED'
} catch (error) {
  report.status = 'FAILED'
  report.error = String(error)
  process.exitCode = 1
} finally {
  report.finished = new Date().toISOString()
  await writeFile(
    join(output, `${mode}.json`),
    JSON.stringify(report, (_, value) => (typeof value === 'bigint' ? value.toString() : value), 2),
  )
  console.log(
    JSON.stringify({ mode, status: report.status, checks: report.checks, error: report.error }),
  )
}
