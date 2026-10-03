import { test, expect, type Page } from '@playwright/test'

async function openWorkspace(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message' })).toBeVisible()
}

for (const failure of ['file-result', 'file-throw', 'mirror-result', 'mirror-throw']) {
  test(`${failure} stays visible, preserves the draft and blocks Run until retry`, async ({ page }) => {
    await openWorkspace(page)
    await page.evaluate(async (failureMode) => {
      const corePath = '/src/core/index.ts', routePath = '/src/router.ts', filePath = '/src/lib/file-token.ts'
      const { gateway } = await import(corePath)
      gateway.saveTest = { failure: failureMode, writes: 0, mirrors: 0, runs: 0, disk: null, mirror: 'old graph' }
      gateway.readFile = async () => ({ ok: true, data: { content: '', path: 'save.blueprint' } })
      gateway.writeFile = async (_ws: string, _path: string, content: string) => {
        const state = gateway.saveTest
        state.writes++
        if (state.failure === 'file-throw') throw new Error('disk unavailable')
        if (state.failure === 'file-result') return { ok: false, error: 'disk unavailable' }
        state.disk = JSON.parse(content)
        return { ok: true, data: undefined }
      }
      gateway.saveBlueprint = async (_ws: string, graph: unknown) => {
        const state = gateway.saveTest
        state.mirrors++
        if (state.failure === 'mirror-throw') throw new Error('mirror unavailable')
        if (state.failure === 'mirror-result') return { ok: false, error: 'mirror unavailable' }
        state.mirror = JSON.parse(JSON.stringify(graph))
        return { ok: true, data: undefined }
      }
      gateway.executeBlueprint = async () => {
        gateway.saveTest.runs++
        return { ok: true, data: undefined }
      }
      const { router } = await import(routePath)
      const { fileRoute } = await import(filePath)
      await router.push(fileRoute('save.blueprint'))
    }, failure)
    await expect(page.locator('.vue-flow__node')).toHaveCount(2)
    await page.getByRole('button', { name: 'Run', exact: true }).click()
    await expect(page.getByTestId('blueprint-save-error')).toContainText(failure.startsWith('file') ? 'File save failed' : 'daemon mirror is not synchronized')
    const failed = await page.evaluate(async () => {
      const corePath = '/src/core/index.ts', storePath = '/src/stores/blueprint.ts'
      const { gateway } = await import(corePath)
      const store = (await import(storePath)).useBlueprintStore()
      return { ...gateway.saveTest, nodes: store.nodes.length, cached: store.graphs['save.blueprint'].nodes.length, saved: store.saved }
    })
    expect(failed).toMatchObject({ writes: 1, mirrors: failure.startsWith('file') ? 0 : 1, runs: 0, mirror: 'old graph', nodes: 2, cached: 2, saved: false })
    expect(failed.disk === null).toBe(failure.startsWith('file'))
    await page.evaluate(async () => {
      const path = '/src/core/index.ts'
      ;(await import(path)).gateway.saveTest.failure = ''
    })
    await page.getByRole('button', { name: 'Run', exact: true }).click()
    await expect(page.getByTestId('blueprint-save-error')).toHaveCount(0)
    await expect.poll(() => page.evaluate(async () => {
      const path = '/src/core/index.ts'
      return (await import(path)).gateway.saveTest.runs
    })).toBe(1)
    const synced = await page.evaluate(async () => {
      const path = '/src/core/index.ts'
      const state = (await import(path)).gateway.saveTest
      return { disk: state.disk, mirror: state.mirror }
    })
    expect(synced.disk).toEqual(synced.mirror)
  })
}

test('a pending save keeps one snapshot and leaves later edits dirty', async ({ page }) => {
  await openWorkspace(page)
  const result = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts', storePath = '/src/stores/blueprint.ts'
    const { gateway } = await import(corePath)
    const store = (await import(storePath)).useBlueprintStore()
    gateway.readFile = async () => ({ ok: true, data: { content: '' } })
    await store.load('test', 'pending.blueprint')
    let release!: () => void
    const wait = new Promise<void>((resolve) => { release = resolve })
    let writes = 0, disk = '', mirror = ''
    gateway.writeFile = async (_ws: string, _file: string, content: string) => {
      writes++
      disk = content
      await wait
      return { ok: true, data: undefined }
    }
    gateway.saveBlueprint = async (_ws: string, graph: unknown) => {
      mirror = JSON.stringify(graph)
      return { ok: true, data: undefined }
    }
    const before = store.keyFor(store.nodes, store.edges)
    const pending = store.save('test', 'pending.blueprint')
    store.nodes[0].title = 'edited during save'
    const overlap = await store.save('test', 'pending.blueprint')
    release()
    const ok = await pending
    return { ok, overlap, writes, disk: JSON.parse(disk), mirror: JSON.parse(mirror), before,
      baseline: store.savedKeys['pending.blueprint'], live: store.keyFor(store.nodes, store.edges), saved: store.saved }
  })
  expect(result).toMatchObject({ ok: true, overlap: false, writes: 1, saved: false })
  expect(result.disk).toEqual(result.mirror)
  expect(result.baseline).toBe(result.before)
  expect(result.live).not.toBe(result.baseline)
})
