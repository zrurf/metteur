import { test, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { readFile } from 'node:fs/promises'
import { Daemon } from '../../src/gen/metteur_pb'

// Real production assets, browser gateway, web proxy, daemon DB and owned HTTP
// provider. Node client only seeds the graph and reads host evidence.
test('DW03 production receipt workflow: supervision, temporary consumption, pause/resume and exact Stop', async ({
  page,
  request,
}) => {
  test.setTimeout(90_000)
  const workspace = process.env.LIVE_WORKSPACE!,
    baseURL = process.env.LIVE_BASE_URL!,
    provider = process.env.LIVE_FIXTURE_URL!
  const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
  const query = async (action: unknown) => {
    const result = await client.chatDelivery({
      workspacePath: workspace,
      schemaVersion: 1,
      payloadJson: JSON.stringify({ operation: 'workflow', action }),
    })
    expect(result.errorJson).toBe('')
    return JSON.parse(result.payloadJson)
  }
  const graph = JSON.parse(
    await readFile(
      new URL('../../scripts/fixtures/nx-local-review.blueprint', import.meta.url),
      'utf8',
    ),
  )
  graph.id = crypto.randomUUID()
  graph.name = 'DW03 receipt workflow'
  const first = graph.nodes.find((node: any) => node.kind === 'CallLLM'),
    second = JSON.parse(JSON.stringify(first))
  second.id = crypto.randomUUID()
  for (const pin of second.pins) pin.id = crypto.randomUUID()
  second.position = [800, 0]
  graph.nodes.push(second)
  graph.edges.push({
    id: crypto.randomUUID(),
    source_node: first.id,
    source_pin: first.pins.find((p: any) => p.pin_type === 'ExecOutput').id,
    target_node: second.id,
    target_pin: second.pins.find((p: any) => p.pin_type === 'ExecInput').id,
  })
  // Native file save is authoritative; its mirror and graph validation are real.
  await client.writeFile({
    workspacePath: workspace,
    path: 'blueprints/dw03.blueprint',
    content: JSON.stringify(graph),
    createOnly: true,
  })
  await page.goto(`/?open=${encodeURIComponent(workspace)}`)
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.getByRole('button', { name: 'Execution', exact: true }).first().click()
  const run = () => {
    const events: any[] = []
    const finished = (async () => {
      try {
        for await (const event of client.executeBlueprint({
          workspacePath: workspace,
          blueprintId: graph.id,
          blueprintJson: JSON.stringify(graph),
        }))
          events.push(event)
      } catch (error) {
        events.push({ kind: 'stream_closed', message: String(error) })
      }
    })()
    return { events, finished }
  }
  const firstRun = run()
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled({
    timeout: 15_000,
  })
  await page.getByRole('button', { name: 'Pause', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeEnabled()
  const panel = page.getByRole('region', { name: 'Run concierge' }),
    input = panel.getByRole('textbox', { name: 'Message' })
  await input.fill('DW03_SUPERVISION_MESSAGE: report recorded progress only')
  await input.press('Enter')
  await expect(
    panel.getByText('Recorded progress read through the local provider.', { exact: false }),
  ).toBeVisible()
  const raw = '  DW03_TEMPORARY_INPUT 中文\n  '
  await input.fill(raw)
  await input.press('ControlOrMeta+Enter')
  await expect(input).toHaveValue('')
  const received = await query({ operation: 'list', after: null }),
    temporary = received.items.find((r: any) => r.operation === 'temporary_input')
  expect(temporary.body).toBe(raw)
  expect(['queued', 'claimed']).toContain(temporary.phase)
  await page.getByRole('button', { name: 'Continue', exact: true }).click()
  await firstRun.finished
  const committed = await query({ operation: 'read', id: temporary.id })
  expect(committed.receipt.phase).toBe('applied')
  expect(firstRun.events.some((e) => e.kind === 'error')).toBe(false)
  const providerProof = await (await request.get(`${provider}/fixture`)).json()
  expect(providerProof.workflowInputRequests).toBeGreaterThan(0)
  expect(providerProof.supervisionRequests).toBe(1)
  await page.getByRole('button', { name: 'Requests', exact: true }).click()
  await page.getByText('Control & Input Receipts', { exact: true }).click()
  await expect(page.getByText('Consumed · node committed', { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.getByRole('button', { name: 'Execution', exact: true }).first()).toBeVisible()
  await page.getByRole('button', { name: 'Execution', exact: true }).first().click()
  const secondRun = run()
  await expect(page.getByRole('button', { name: 'Stop', exact: true }).first()).toBeEnabled({
    timeout: 15_000,
  })
  await page.getByRole('button', { name: 'Stop', exact: true }).first().click()
  await secondRun.finished
  const stopped = await query({ operation: 'list', after: null })
  expect(stopped.items.some((r: any) => r.operation === 'stop' && r.phase === 'accepted')).toBe(
    true,
  )
})
