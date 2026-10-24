// A single frozen task through production RPCs. No generated text is a verdict.
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile } from 'node:fs/promises'
import assert from 'node:assert/strict'
const [readyPath, specPath, output] = process.argv.slice(2)
const ready = JSON.parse(await readFile(readyPath, 'utf8')),
  spec = JSON.parse(await readFile(specPath, 'utf8'))
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL })),
  workspacePath = ready.workspace
const model = ready.model || 'flash'
const report = {
  task: spec.id,
  mode: ready.mode,
  events: [],
  started: new Date().toISOString(),
  status: 'running',
}
async function resource(method, payload) {
  const r = await client[method]({
    workspacePath,
    schemaVersion: 1,
    requestId: crypto.randomUUID(),
    payloadJson: JSON.stringify(payload),
  })
  if (r.errorJson) throw Error(r.errorJson)
  return JSON.parse(r.payloadJson)
}
async function receive(event) {
  const detail = JSON.parse(event.detailJson || '{}')
  if (!event.kind.includes('reasoning'))
    report.events.push({ kind: event.kind, message: event.message || event.content, detail })
  if (detail.run_id) report.run = detail.run_id
  if (event.kind === 'error')
    throw new Error(event.content || event.message || 'The host reported a failed turn')
  if (event.kind === 'approval_request' || event.kind === 'approval') {
    const body = event.kind === 'approval' ? JSON.parse(event.content) : detail
    const command = body.command || body.subject || ''
    const allow =
      body.request_type === 'oversight_gate' ||
      ['py -3.12 check_public.py', 'py -3.12 -m unittest test_added'].includes(command)
    report.approvals = (report.approvals || 0) + 1
    await client.respondApproval({
      workspacePath,
      requestId: detail.request_id || event.message,
      decision: allow ? 'AllowOnce' : 'DenyOnce',
    })
  }
}
try {
  await client.openWorkspace({ path: workspacePath })
  const goal =
    spec.goal +
    ' Read the repository files. Implement the task and add test_added.py. Run py -3.12 check_public.py and py -3.12 -m unittest test_added. Do not change README.md or check_public.py. Use only the standard library.'
  if (ready.mode === 'react') {
    for await (const e of client.sendChat(
      {
        workspacePath,
        message: goal,
        historyJson: '[]',
        optionsJson: JSON.stringify({
          provider: 'openai-chat',
          model,
          temperature: 0,
          max_tokens: 1024,
          max_iterations: 8,
          permission_mode: 'sandbox',
        }),
      },
      { timeoutMs: 240000 },
    ))
      await receive(e)
  } else {
    const command = (id, description, command, exit = 0) => ({
      id,
      description,
      command,
      expected_exit_code: exit,
      required: true,
    })
    const input = {
      instance_id: ready.instance_id,
      template: spec.id,
      name: spec.id,
      goal,
      constraints: 'Preserve README.md and check_public.py; standard library only.',
      scope_paths: spec.allowed,
      evidence_paths: ['app.py', 'test_added.py'],
      model,
      max_iterations: ready.max_iterations || 8,
      baseline: command(
        'baseline',
        'Frozen baseline',
        'py -3.12 check_public.py',
        spec.id === 'fix_bug' ? 1 : 0,
      ),
      checks: [
        command('public', 'Public regression', 'py -3.12 check_public.py'),
        command('added', 'Added tests', 'py -3.12 -m unittest test_added'),
      ],
      manual_checks: [
        { id: 'scope', description: 'Independent scope and behavior review', required: true },
      ],
    }
    const { graph } = await resource('queryCodingWorkflows', { operation: 'compile', input })
    for (const n of graph.nodes)
      if (n.kind === 'CallLLM') {
        n.data.max_tokens = 1024
        n.data.temperature = 0
      }
    if (ready.mode === 'plan_execute_supervised') {
      const gate = await client.compileDsl({
        workspacePath,
        source:
          'entry start: Start\nreview: OversightCheckpoint\nend: End\nstart -> review\nreview -> end\n',
      })
      const node = gate.nodes.find((n) => n.kind === 'OversightCheckpoint')
      assert(node, 'The supervised mode requires the real oversight checkpoint')
      const native = {
        id: node.id,
        kind: node.kind,
        node_type: node.nodeType,
        position: [0, 0],
        data: JSON.parse(node.dataJson || '{}'),
        pins: node.pins.map((p) => ({
          id: p.id,
          name: p.name,
          key: p.key || null,
          pin_type: p.pinType,
          data_type: p.dataType,
          default: p.defaultJson ? JSON.parse(p.defaultJson) : null,
          optional: p.optional,
          choices: p.choices,
          description: p.description || null,
        })),
      }
      const implementation = graph.nodes.find((n) => n.kind === 'CallLLM'),
        out = implementation.pins.find((p) => p.pin_type === 'ExecOutput')
      const next = graph.edges.find(
        (e) => e.source_node === implementation.id && e.source_pin === out.id,
      )
      assert(next)
      graph.edges.push({
        ...next,
        id: crypto.randomUUID(),
        source_node: native.id,
        source_pin: native.pins.find((p) => p.pin_type === 'ExecOutput').id,
      })
      next.target_node = native.id
      next.target_pin = native.pins.find((p) => p.pin_type === 'ExecInput').id
      graph.nodes.push(native)
      report.supervision =
        'Real OversightCheckpoint after implementation, before acceptance; assisted mode, unapproved control proposals denied.'
    }
    const blueprint = {
      id: graph.id,
      name: graph.name,
      entryNodeId: graph.entry_node_id,
      nodes: graph.nodes.map((n) => ({
        id: n.id,
        nodeType: n.node_type,
        kind: n.kind,
        posX: n.position[0],
        posY: n.position[1],
        dataJson: JSON.stringify(n.data),
        pins: n.pins.map((p) => ({
          id: p.id,
          name: p.name,
          key: p.key || '',
          pinType: p.pin_type,
          dataType: p.data_type,
          defaultJson: p.default === null ? '' : JSON.stringify(p.default),
          optional: p.optional || false,
          choices: p.choices || [],
          description: p.description || '',
        })),
      })),
      edges: graph.edges.map((e) => ({
        id: e.id,
        sourceNode: e.source_node,
        sourcePin: e.source_pin,
        targetNode: e.target_node,
        targetPin: e.target_pin,
      })),
    }
    await client.saveBlueprint({
      workspacePath,
      blueprint,
      filePath: 'blueprints/task.blueprint',
      fileJson: JSON.stringify(graph),
    })
    await writeFile(
      output + '.host-files.json',
      JSON.stringify({ 'blueprints/task.blueprint': JSON.stringify(graph) }),
    )
    const prepared = await resource('queryCodingWorkflows', {
      operation: 'prepare_run',
      blueprint_id: graph.id,
    })
    report.confirmed_digest = prepared.digest
    for await (const e of client.executeBlueprint(
      {
        workspacePath,
        blueprintId: graph.id,
        blueprintJson: JSON.stringify(graph),
        workflowConfirmation: prepared.digest,
      },
      { timeoutMs: 240000 },
    ))
      await receive(e)
    if (report.run)
      report.review = await resource('queryCodingWorkflows', {
        operation: 'review',
        run_id: report.run,
      })
  }
  report.status = 'completed'
} catch (e) {
  report.status = 'failed'
  report.error = String(e)
}
report.finished = new Date().toISOString()
await writeFile(output, JSON.stringify(report, null, 2))
console.log(
  JSON.stringify({ task: spec.id, mode: ready.mode, status: report.status, error: report.error }),
)
if (report.status === 'failed') process.exitCode = 1
