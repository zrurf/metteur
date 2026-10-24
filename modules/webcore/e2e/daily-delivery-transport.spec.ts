import { test, expect } from '@playwright/test'

// L2 wire fixtures: actual SSE parser/gateway, no daemon durability claim.
for (const outcome of ['error', 'delivery_replay', 'closed'] as const) {
  test(`durable receipt is distinct from ${outcome} and does not invent usage`, async ({
    page,
  }) => {
    const id = 'bdf221e3-c41c-42a0-85e8-4411e1a2b760',
      session = 'c0eb2fa9-06a7-4d3c-b157-c267eb7e8ba2'
    const receipt = {
      schema_version: 1,
      id,
      input_digest: 'fingerprint',
      session,
      turn: '9702449a-d400-49c9-808e-06ba6d1f6863',
      checkpoint: 'checkpoint',
      accepted_revision: 1,
      phase: 'started',
      outcome: null,
      accepted_ms: 1,
      updated_ms: 1,
    }
    const events = [
      { kind: 'delivery', content: '', detail_json: JSON.stringify(receipt) },
      {
        kind: outcome,
        content:
          outcome === 'error'
            ? 'model failed after acceptance'
            : 'Original turn retained; no replay',
        detail_json:
          outcome === 'error' ? JSON.stringify({ turn_elapsed_ms: 9000 }) : JSON.stringify(receipt),
      },
    ]
    await page.route('**/api/chat/stream', (route) =>
      route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: events
          .filter((event) => outcome !== 'closed' || event.kind === 'delivery')
          .map((event) => 'data: ' + JSON.stringify(event) + '\n\n')
          .join(''),
      }),
    )
    await page.goto('/')
    const result = await page.evaluate(
      async ({ id: requestId, session: targetSession }) => {
        const path = '/src/core/grpc-gateway.ts',
          types = '/src/core/delivery.ts'
        const { GrpcGateway } = await import(path),
          { DeliveryAttempt } = await import(types)
        const gateway = new GrpcGateway(''),
          receipts: any[] = [],
          notices: any[] = [],
          usage: any[] = []
        const transport = {
          resourceRequest: async () => ({
            ok: true,
            data: {
              durable_manual_send: true,
              pending_reconciliation: true,
              items: [],
              raw_copies_unchanged: true,
            },
          }),
        }
        const attempt = new DeliveryAttempt(
          transport,
          {
            workspace: 'fixture',
            session: '',
            newSession: targetSession,
            message: 'raw\r\n中文',
            history: [],
            options: { model: 'fixture' },
          },
          (deliveryReceipt: any) => receipts.push(deliveryReceipt),
        )
        // Pin this fixture's logical request before sending, as host recovery does.
        Object.defineProperty(attempt, 'id', { value: requestId })
        const accepted = await attempt.submit(async (snapshot: any, onReceipt: any) =>
          gateway.sendChat(
            snapshot.workspace,
            snapshot.message,
            snapshot.history,
            (message: any) => notices.push(message),
            snapshot.options,
            undefined,
            '',
            (value) => usage.push(value),
            undefined,
            undefined,
            undefined,
            undefined,
            undefined,
            onReceipt,
          ),
        )
        return {
          accepted,
          state: attempt.state,
          execution: attempt.execution,
          receipts,
          notices,
          usage,
        }
      },
      { id, session },
    )
    expect(result.accepted).toBe(true)
    expect(result.state).toBe('accepted')
    expect(result.receipts).toHaveLength(1)
    expect(result.receipts[0].id).toBe(id)
    if (outcome === 'error') {
      expect(result.usage).toHaveLength(1)
      expect(result.usage[0].turnElapsedMs).toBe(9000)
      expect(result.usage[0].totalTokens).toBeNull()
      expect(result.usage[0].inputTokens).toBeNull()
    } else expect(result.usage).toEqual([])
    expect(result.execution?.ok).toBe(outcome === 'delivery_replay')
    if (outcome === 'closed')
      expect(result.execution).toMatchObject({
        ok: false,
        error: expect.stringContaining('execution outcome'),
      })
    if (outcome === 'delivery_replay') expect(result.notices[0].content).toContain('no replay')
  })
}
