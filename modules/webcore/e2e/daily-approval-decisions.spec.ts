import { test, expect, type Page } from '@playwright/test'
import { createHash } from 'node:crypto'

const detail = JSON.stringify({ command: 'echo immutable approval', outside_workspace: false })
const digest = createHash('sha256').update(detail).digest('hex')
function host(kind: 'chat' | 'blueprint' = 'chat', mode = 'accept') {
  const id = crypto.randomUUID(),
    target =
      kind === 'chat'
        ? { kind, session: crypto.randomUUID(), turn: crypto.randomUUID(), run: null }
        : { kind, session: null, turn: null, run: crypto.randomUUID() }
  const calls: any[] = [],
    receipts = new Map<string, any>()
  const flags = {
    mode,
    offline: false,
    unsupported: false,
    active: true,
    release: null as null | (() => void),
    pendingDigest: digest,
  }
  async function handle(value: any): Promise<any> {
    calls.push(structuredClone(value))
    if (value.method === 'respond') {
      if (flags.mode === 'reject') return { ok: false, error: 'Synthetic decision response failed' }
      receipts.set(value.id, {
        schema_version: 1,
        id: value.id,
        target: value.binding.target,
        content_digest: value.binding.content_digest,
        decision: value.allow ? 'AllowOnce' : 'DenyOnce',
        phase: 'accepted',
      })
      if (flags.mode === 'hold')
        await new Promise<void>((resolve) => {
          flags.release = resolve
        })
      if (flags.mode === 'lost') {
        flags.offline = true
        return { ok: false, error: 'Lost approval acknowledgement' }
      }
      return { ok: true, data: undefined }
    }
    if (value.method === 'control') {
      flags.active = false
      return { ok: true, data: undefined }
    }
    const action = value.payload
    if (flags.offline) return { ok: false, error: 'Receipt query disconnected' }
    if (action.operation === 'capabilities')
      return {
        ok: true,
        data: {
          durable_decisions: !flags.unsupported,
          fixed_target_content: true,
          read_or_retire: true,
        },
      }
    if (action.operation === 'describe')
      return {
        ok: true,
        data: {
          receipt: receipts.get(action.id) ?? null,
          pending: { id, target, content_digest: flags.pendingDigest },
        },
      }
    if (action.operation === 'read_or_retire' && !receipts.has(action.id)) {
      if (action.content_digest !== flags.pendingDigest)
        return {
          ok: false,
          error: 'identity_conflict: content changed; original pending request kept',
        }
      receipts.set(action.id, {
        schema_version: 1,
        id: action.id,
        target: action.target,
        content_digest: action.content_digest,
        decision: null,
        phase: 'retired',
      })
    }
    return { ok: true, data: { receipt: receipts.get(action.id) ?? null } }
  }
  return { id, target, flags, calls, receipts, handle }
}
async function setup(page: Page, fixture: ReturnType<typeof host>, selectedOther = false) {
  await page.exposeFunction('dwApprovalTransport', fixture.handle)
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/approvals')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
  await expect(page.getByRole('button', { name: /^Conversations/ })).toBeEnabled()
  await page.evaluate(
    async ({ id, target, content, contentDigest, other }) => {
      const { gateway } = await import('/src/core/index.ts')
      Object.defineProperty(gateway, 'demo', { value: false, configurable: true })
      gateway.connected.value = true
      const original = gateway.resourceRequest.bind(gateway)
      gateway.resourceRequest = (method: string, ws: string, payload: unknown) =>
        method === 'approvalDelivery'
          ? (window as any).dwApprovalTransport({ method: 'query', ws, payload })
          : original(method, ws, payload)
      gateway.respondApproval = (ws: string, request: string, allow: boolean, binding: unknown) =>
        (window as any).dwApprovalTransport({ method: 'respond', ws, id: request, allow, binding })
      gateway.abortChat = (ws: string, control: unknown) =>
        (window as any).dwApprovalTransport({ method: 'control', ws, target: control })
      gateway.cancel = (ws: string, run: string) =>
        (window as any).dwApprovalTransport({ method: 'control', ws, run })
      if (target.kind === 'chat') {
        const chat = (await import('/src/stores/chat.ts')).useChatStore()
        chat.sessionId = other ? crypto.randomUUID() : target.session!
        chat.activeTurn = other ? crypto.randomUUID() : target.turn!
        chat.streaming = true
        chat.busy = false
        chat.draft = 'Keep selected conversation draft'
        chat.approval = {
          requestId: id,
          detail: content,
          contentDigest,
          ...(other ? { target } : {}),
        }
      } else {
        const store = (await import('/src/stores/execution.ts')).useExecutionStore()
        store.runId = target.run
        store.status = 'running'
        store.current = {
          runId: target.run!,
          status: 'Running',
          startedAt: 1,
          updatedAt: 1,
          snapshot: {
            runtime: {
              active: true,
              pause_requested: false,
              cancel_requested: false,
              pending_approval_ids: [id],
            },
          },
        }
        store.approval = {
          id,
          title: 'Approve exact blueprint command',
          tool: 'shell',
          command: 'echo immutable approval',
          detail: 'This run requests this command.',
          contentDigest,
        }
        gateway.listExecutions = async () => {
          const controls = await (window as any).dwApprovalState()
          return {
            ok: true,
            data: [
              {
                runId: target.run!,
                status: controls.active ? 'Running' : 'Cancelled',
                startedAt: 1,
                updatedAt: Date.now(),
                snapshot: {
                  runtime: {
                    active: controls.active,
                    pending_approval_ids: controls.active ? [id] : [],
                    pause_requested: false,
                    cancel_requested: !controls.active,
                  },
                },
              },
            ],
          }
        }
      }
    },
    {
      id: fixture.id,
      target: fixture.target,
      content: detail,
      contentDigest: digest,
      other: selectedOther,
    },
  )
  await expect(page.getByRole('dialog')).toBeVisible()
}
test('L2 actual gateway encodes the fixed approval target and exact content digest', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const { GrpcGateway } = await import('/src/core/grpc-gateway.ts'),
      gateway = new GrpcGateway('')
    let captured: any
    gateway.client = {
      respondApproval: async (value: unknown) => {
        captured = value
      },
    }
    const binding = {
      target: { kind: 'chat' as const, session: crypto.randomUUID(), turn: crypto.randomUUID() },
      content_digest: 'a'.repeat(64),
    }
    await gateway.respondApproval('fixture', 'approval-id', true, binding)
    return { captured, binding }
  })
  expect(JSON.parse(result.captured.targetJson)).toEqual(result.binding.target)
  expect(result.captured.contentDigest).toBe(result.binding.content_digest)
  expect(result.captured.decision).toBe('AllowOnce')
})
test('L2 failed decision retains its dialog; original receipt retirement closes without granting', async ({
  page,
}) => {
  const fixture = host('chat', 'reject')
  await setup(page, fixture)
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText(
    'Synthetic decision response failed',
  )
  await expect(page.getByRole('button', { name: 'Allow', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Check Decision Receipt', exact: true }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  await expect(page.getByText('Decision Receipt', { exact: true })).toBeVisible()
  expect(fixture.receipts.get(fixture.id).phase).toBe('retired')
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(1)
})
test('L2 lost accepted reply reconciles the original decision with no second approval', async ({
  page,
}) => {
  const fixture = host('chat', 'lost')
  await setup(page, fixture)
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect(
    page.getByRole('button', { name: 'Check Decision Receipt', exact: true }),
  ).toBeVisible()
  await expect(page.getByRole('button', { name: 'Deny', exact: true })).toBeDisabled()
  fixture.flags.offline = false
  await page.getByRole('button', { name: 'Check Decision Receipt', exact: true }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(1)
})
test('L2 late approval response cannot clear another workspace request or its draft', async ({
  page,
}) => {
  const fixture = host('chat', 'hold')
  await setup(page, fixture)
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect.poll(() => !!fixture.flags.release).toBe(true)
  const nextId = await page.evaluate(async () => {
    await (
      await import('/src/stores/workspace.ts')
    )
      .useWorkspaceStore()
      .open('D:/metteur-demo/approval-other')
    const chat = (await import('/src/stores/chat.ts')).useChatStore(),
      id = crypto.randomUUID()
    chat.sessionId = crypto.randomUUID()
    chat.activeTurn = crypto.randomUUID()
    chat.busy = false
    chat.draft = 'New workspace draft'
    chat.approval = {
      requestId: id,
      detail: JSON.stringify({ command: 'New workspace command' }),
      contentDigest: 'b'.repeat(64),
    }
    return id
  })
  fixture.flags.release!()
  await expect(page.getByRole('dialog')).toContainText('New workspace command')
  const state = await page.evaluate(async () => {
    const chat = (await import('/src/stores/chat.ts')).useChatStore()
    return { id: chat.approval?.requestId, draft: chat.draft }
  })
  expect(state).toEqual({ id: nextId, draft: 'New workspace draft' })
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(1)
})
test('L2 direct Stop stays available during a blueprint decision with no response', async ({
  page,
}) => {
  const fixture = host('blueprint', 'hold')
  await page.exposeFunction('dwApprovalState', () => ({ active: fixture.flags.active }))
  await setup(page, fixture)
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect.poll(() => !!fixture.flags.release).toBe(true)
  await expect(page.getByRole('button', { name: 'Allow', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Stop run directly', exact: true }).click()
  await expect.poll(() => fixture.calls.filter((call) => call.method === 'control').length).toBe(1)
  expect(fixture.calls.find((call) => call.method === 'control').run).toBe(fixture.target.run)
  await expect(page.getByRole('dialog')).toHaveCount(0)
  fixture.flags.release!()
})
test('L2 a saved-task approval stops its own turn without replacing the selected conversation', async ({
  page,
}) => {
  const fixture = host('chat')
  await setup(page, fixture, true)
  const before = await page.evaluate(async () => {
    const chat = (await import('/src/stores/chat.ts')).useChatStore()
    return { session: chat.sessionId, draft: chat.draft }
  })
  await page.getByRole('button', { name: 'Stop turn directly', exact: true }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  const stopped = fixture.calls.find((call) => call.method === 'control')
  expect(stopped.target).toEqual({
    kind: 'chat',
    sessionId: fixture.target.session,
    turnId: fixture.target.turn,
  })
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(0)
  const after = await page.evaluate(async () => {
    const chat = (await import('/src/stores/chat.ts')).useChatStore()
    return { session: chat.sessionId, draft: chat.draft }
  })
  expect(after).toEqual(before)
})
test('L2 content mismatch and old-host capability keep the request and grant nothing', async ({
  page,
}) => {
  const fixture = host('chat')
  fixture.flags.pendingDigest = 'c'.repeat(64)
  await setup(page, fixture)
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('content changed')
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(0)
  await page.getByRole('button', { name: 'Check Decision Receipt', exact: true }).click()
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('identity_conflict')
  expect(fixture.receipts.size).toBe(0)
  await page.getByRole('button', { name: 'Close receipt details', exact: true }).click()
  fixture.flags.unsupported = true
  await page.evaluate(async () => {
    const chat = (await import('/src/stores/chat.ts')).useChatStore()
    chat.approval = {
      requestId: crypto.randomUUID(),
      detail: 'Old host request',
      contentDigest: 'd'.repeat(64),
    }
  })
  await page.getByRole('button', { name: 'Allow', exact: true }).click()
  await expect(page.getByRole('dialog').getByRole('alert')).toContainText('Update the host')
  await expect(page.getByRole('button', { name: 'Stop turn directly', exact: true })).toBeEnabled()
  expect(fixture.calls.filter((call) => call.method === 'respond')).toHaveLength(0)
})
