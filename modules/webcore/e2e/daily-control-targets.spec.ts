import { test, expect } from '@playwright/test'

// Actual gateway adapters against a typed transport fixture, not a daemon proof.
test('L2 gateway forwards exact immutable targets and refuses missing targets or old-host capability', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const path = '/src/core/grpc-gateway.ts',
      { GrpcGateway } = await import(path),
      gateway = new GrpcGateway('')
    const calls: any[] = []
    const receipts = new Map<string, any>()
    let supports = true
    gateway.client = {
      chatDelivery: async (request: any) => {
        const payload = JSON.parse(request.payloadJson),
          action = payload.action
        let data: any = { targeted_controls: supports }
        if (payload.operation === 'workflow') {
          if (action.operation === 'capabilities') data = { durable_controls: supports }
          else if (action.operation === 'describe') data = { revision: 0, target: action.target }
          else if (action.operation === 'prepare') {
            const receipt = {
              schema: 1,
              id: action.identity.id,
              target: action.target,
              operation: action.action,
              body: action.body,
              revision: 0,
              phase: 'prepared',
            }
            receipts.set(receipt.id, receipt)
            data = { receipt }
          } else data = { receipt: receipts.get(action.id) }
        }
        return { schemaVersion: 1, payloadJson: JSON.stringify(data), errorJson: '' }
      },
      sendInterrupt: async (request: any) => {
        calls.push({ method: 'interrupt', request })
        if (request.receiptJson) receipts.get(JSON.parse(request.receiptJson).id).phase = 'queued'
        return {}
      },
      abortChat: async (request: any) => {
        calls.push({ method: 'stop', request })
        const { id } = JSON.parse(request.receiptJson)
        receipts.set(id, {
          schema: 1,
          id,
          target: { kind: 'chat', session: request.sessionId, turn: request.turnId },
          operation: 'stop',
          body: null,
          revision: 1,
          phase: 'accepted',
        })
        return {}
      },
    }
    const chat = { kind: 'chat', sessionId: 'original-session', turnId: 'original-turn' },
      blueprint = { kind: 'blueprint', runId: 'original-run' }
    const changingChat = { ...chat },
      pendingSend = gateway.sendInterrupt('workspace', 'raw\r\n input', 'Urgent', changingChat)
    changingChat.turnId = 'new turn during capability read'
    const sent = await pendingSend
    const changingStop = { ...chat },
      pendingStop = gateway.abortChat('workspace', changingStop)
    changingStop.sessionId = 'another session during capability read'
    const stopped = await pendingStop
    const directed = await gateway.sendInterrupt(
      'workspace',
      'urgent supervision',
      'Urgent',
      blueprint,
    )
    const missing = await gateway.sendInterrupt('workspace', 'ambiguous', 'Urgent')
    const unqualified = await gateway.abortChat('workspace')
    supports = false
    const oldHost = await gateway.sendInterrupt('workspace', 'no downgrade', 'Normal', chat)
    const oldStop = await gateway.abortChat('workspace', chat)
    const ids = calls.flatMap((c) =>
      c.request.receiptJson ? [JSON.parse(c.request.receiptJson).id] : [],
    )
    for (const call of calls) {
      delete call.request.receiptJson
      if (call.request.targetKind === 'blueprint') {
        call.request.sessionId = ''
        call.request.turnId = ''
      }
    }
    return { sent, stopped, directed, missing, unqualified, oldHost, oldStop, calls, ids }
  })
  expect(result.sent.ok && result.stopped.ok && result.directed.ok).toBe(true)
  expect(new Set(result.ids).size).toBe(2)
  expect(result.missing.ok || result.unqualified.ok || result.oldHost.ok || result.oldStop.ok).toBe(
    false,
  )
  expect(result.calls).toEqual([
    {
      method: 'interrupt',
      request: {
        workspacePath: 'workspace',
        message: 'raw\r\n input',
        priority: 'Urgent',
        targetKind: 'chat',
        sessionId: 'original-session',
        turnId: 'original-turn',
        runId: '',
      },
    },
    {
      method: 'stop',
      request: {
        workspacePath: 'workspace',
        sessionId: 'original-session',
        turnId: 'original-turn',
      },
    },
    {
      method: 'interrupt',
      request: {
        workspacePath: 'workspace',
        message: 'urgent supervision',
        priority: 'Urgent',
        targetKind: 'blueprint',
        sessionId: '',
        turnId: '',
        runId: 'original-run',
      },
    },
  ])
})
