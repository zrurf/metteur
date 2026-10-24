import { test, expect } from '@playwright/test'

test('L2 actual gateway orders controls, keeps immutable targets and reconciles a lost ACK', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const path = '/src/core/grpc-gateway.ts',
      { GrpcGateway } = await import(path),
      gateway = new GrpcGateway('')
    const calls: any[] = [],
      records = new Map<string, any>(),
      run = crypto.randomUUID()
    let revision = 0
    const receive = (request: any, action: string) => {
      const identity = JSON.parse(request.receiptJson),
        record = records.get(identity.id) ?? {
          schema: 1,
          id: identity.id,
          target: { kind: 'blueprint', run: request.runId },
          operation: action,
          body: null,
        }
      record.phase = action === 'temporary_input' ? 'queued' : 'accepted'
      record.revision = ++revision
      records.set(identity.id, record)
      calls.push({ action, request })
      return {}
    }
    gateway.client = {
      listWorkspaces: async () => ({ workspaces: [] }),
      chatDelivery: async (request: any) => {
        const q = JSON.parse(request.payloadJson),
          a = q.action
        let data: any
        if (q.operation === 'capabilities') data = { targeted_controls: true }
        else if (a.operation === 'capabilities') data = { durable_controls: true }
        else if (a.operation === 'describe') data = { revision, target: a.target }
        else if (a.operation === 'prepare') {
          const r = {
            schema: 1,
            id: a.identity.id,
            target: a.target,
            operation: a.action,
            body: a.body,
            revision: a.identity.revision,
            phase: 'prepared',
          }
          records.set(r.id, r)
          data = { receipt: r }
        } else {
          const r = records.get(a.id)
          if (r.phase === 'prepared') r.phase = 'retired'
          data = { receipt: r }
        }
        return { schemaVersion: 1, payloadJson: JSON.stringify(data), errorJson: '' }
      },
      pauseExecution: async (request: any) => receive(request, 'pause'),
      resumeExecution: async (request: any) => receive(request, 'resume'),
      cancelExecution: async (request: any) => {
        receive(request, 'stop')
        throw Error('Lost ACK')
      },
      sendInterrupt: async (request: any) => receive(request, 'temporary_input'),
    }
    const target = { kind: 'blueprint', runId: run },
      sending = gateway.sendInterrupt('workspace', '  原文\r\n  ', 'Urgent', target)
    target.runId = crypto.randomUUID()
    const input = await sending,
      paused = await gateway.pause('workspace', run),
      resumed = await gateway.resume('workspace', run),
      stopped = await gateway.cancel('workspace', run)
    return { input, paused, resumed, stopped, calls, run }
  })
  expect([result.input, result.paused, result.resumed, result.stopped].every((r) => r.ok)).toBe(
    true,
  )
  expect(result.calls.map((c) => c.action)).toEqual(['temporary_input', 'pause', 'resume', 'stop'])
  expect(result.calls.every((c) => c.request.runId === result.run)).toBe(true)
  expect(result.calls[0].request.message).toBe('  原文\r\n  ')
})

test('L2 receipt view is collapsed, queries on demand, and reopens without executing a control', async ({
  page,
}) => {
  const run = crypto.randomUUID(),
    id = crypto.randomUUID()
  let reads = 0,
    mutations = 0
  await page.exposeFunction('workflowReceiptHost', ({ payload }: any) => {
    if (payload.operation !== 'workflow') return null
    const q = payload.action
    reads++
    if (q.operation === 'list')
      return {
        ok: true,
        data: {
          items: [
            {
              schema: 1,
              id,
              target: { kind: 'blueprint', run },
              operation: 'temporary_input',
              revision: 0,
              phase: 'unconsumed',
              body: '  original 中文\r\n  ',
            },
          ],
          next: null,
        },
      }
    mutations++
    return { ok: false, error: 'No control is allowed in this read-only fixture' }
  })
  const install = async () => {
    await page.goto('/')
    if (await page.getByLabel('Workspace path').count()) {
      await page.getByLabel('Workspace path').fill('D:/metteur-demo/receipts')
      await page.getByRole('button', { name: 'Open', exact: true }).click()
    }
    await expect(page.getByRole('button', { name: 'Chat', exact: true }).first()).toBeVisible()
    await page.evaluate(async () => {
      const path = '/src/core/index.ts',
        { gateway } = await import(path)
      Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
      const original = gateway.resourceRequest.bind(gateway)
      gateway.resourceRequest = (method: string, ws: string, payload: any, ...rest: any[]) =>
        payload.operation === 'workflow'
          ? (window as any).workflowReceiptHost({ payload })
          : original(method, ws, payload, ...rest)
    })
    await page.getByRole('button', { name: 'Chat', exact: true }).first().click()
  }
  await install()
  expect(reads).toBe(0)
  await page.getByText('Control & Input Receipts', { exact: true }).click()
  await expect(page.getByText('Run ended · input was not consumed', { exact: true })).toBeVisible()
  await page.getByText('Source & Content', { exact: true }).click()
  await expect(page.locator('.workflow-receipts pre')).toHaveText('  original 中文\r\n  ')
  await page.reload()
  await install()
  await page.getByText('Control & Input Receipts', { exact: true }).click()
  await expect(page.getByText('Run ended · input was not consumed', { exact: true })).toBeVisible()
  expect(mutations).toBe(0)
})
