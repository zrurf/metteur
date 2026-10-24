import { test, expect } from '@playwright/test'

test('manual snapshots and pushed invalidations share one queue and retain changes during reads', async ({
  page,
}) => {
  await page.goto('/')
  const sequence = await page.evaluate(async () => {
    const vue = '/node_modules/.vite/deps/vue.js',
      refresh = '/src/lib/workspace-refresh.ts',
      events = '/src/stores/workspace-events.ts'
    const { effectScope, nextTick } = await import(vue),
      { useWorkspaceRefresh } = await import(refresh)
    const stream = (await import(events)).useWorkspaceEventsStore(),
      scope = effectScope(),
      order: string[] = []
    let releaseManual!: () => void,
      releaseFirstEvent!: () => void,
      eventStarted!: () => void,
      completed!: () => void
    const manualGate = new Promise<void>((resolve) => {
      releaseManual = resolve
    })
    const eventGate = new Promise<void>((resolve) => {
      releaseFirstEvent = resolve
    })
    const started = new Promise<void>((resolve) => {
      eventStarted = resolve
    })
    const done = new Promise<void>((resolve) => {
      completed = resolve
    })
    let count = 0
    const updates = scope.run(() =>
      useWorkspaceRefresh(async () => {
        count++
        order.push(`event-${count}`)
        if (count === 1) {
          eventStarted()
          await eventGate
        } else completed()
      }),
    )!
    try {
      const manual = updates.run(async () => {
        order.push('manual-start')
        await manualGate
        order.push('manual-end')
      })
      await nextTick()
      stream.revision++
      // The production coalescer has a 60 ms debounce. Keep the manual read
      // outstanding until the invalidation is queued behind it.
      await new Promise((resolve) => setTimeout(resolve, 90))
      releaseManual()
      await manual
      await started
      stream.revision++
      await nextTick()
      releaseFirstEvent()
      await done
      return order
    } finally {
      releaseManual()
      releaseFirstEvent()
      scope.stop()
    }
  })
  expect(sequence).toEqual(['manual-start', 'manual-end', 'event-1', 'event-2'])
})

test('late automatic restore cannot replace a manual selection and a failed close preserves it', async ({
  page,
}) => {
  await page.goto('/')
  const result = await page.evaluate(async () => {
    const workspace = '/src/stores/workspace.ts',
      core = '/src/core/index.ts'
    const store = (await import(workspace)).useWorkspaceStore(),
      { gateway } = await import(core)
    const originalOpen = gateway.openWorkspace,
      originalClose = gateway.closeWorkspace
    let release!: () => void
    const gate = new Promise<void>((resolve) => {
      release = resolve
    })
    gateway.openWorkspace = async (path: string) => {
      if (path.endsWith('old')) await gate
      return { ok: true, data: { id: path, path, status: 'open' } }
    }
    gateway.closeWorkspace = async () => ({
      ok: false,
      error: 'workspace still has an active operation',
    })
    sessionStorage.setItem('metteur.last-workspace', 'D:/synthetic/old')
    try {
      const restoring = store.restore()
      await store.open('D:/synthetic/new')
      release()
      await restoring
      const selection = store.active?.path
      const closed = await store.close()
      return { selection, closed, afterClose: store.active?.path }
    } finally {
      release()
      gateway.openWorkspace = originalOpen
      gateway.closeWorkspace = originalClose
    }
  })
  expect(result).toEqual({
    selection: 'D:/synthetic/new',
    closed: false,
    afterClose: 'D:/synthetic/new',
  })
})
