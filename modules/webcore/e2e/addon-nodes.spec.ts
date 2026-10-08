import { test, expect } from '@playwright/test'

test('workspace catalogue nodes retain their package identity through canvas save and reload', async ({ page }) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.evaluate(async () => {
    const core = '/src/core/index.ts', route = '/src/router.ts', file = '/src/lib/file-token.ts'
    const catalogPath = '/src/core/node-catalog.ts'
    const { gateway } = await import(core), { fromWireCatalog } = await import(catalogPath)
    const catalog = (await gateway.listNodeKinds()).data
    const addon = fromWireCatalog({ signatureVersion: 1, kinds: ['ComTestNodesCalculate'], infos: [{
      kind: 'ComTestNodesCalculate', nodeType: 'Pure', dynamicPins: false, description: 'Package node',
      addonBindingJson: JSON.stringify({ id: 'com.test.nodes', version: '1.0.0', fingerprint: 'original', scope: 'D:/metteur-demo/metrics' }),
      pins: [{ id: '', key: 'a', name: 'A', pinType: 'DataInput', dataType: 'int', defaultJson: '7', optional: true }],
    }] })
    catalog.kinds.push(...addon.kinds); catalog.nodes.push(...addon.nodes)
    gateway.listNodeKinds = async (path: string) => {
      if (path !== 'D:/metteur-demo/metrics') throw new Error('Workspace scope was lost')
      return { ok: true, data: catalog }
    }
    gateway.readFile = async () => ({ ok: true, data: { content: '', path: 'addon.blueprint' } })
    gateway.writeFile = async (_ws: string, _path: string, content: string) => {
      gateway.addonNodeSaved = JSON.parse(content); return { ok: true, data: undefined }
    }
    const { router } = await import(route), { fileRoute } = await import(file)
    await router.push(fileRoute('addon.blueprint'))
  })
  await expect(page.locator('.blueprint-flow')).toBeVisible()
  await page.locator('.vue-flow__pane').click({ button: 'right', position: { x: 450, y: 180 } })
  await page.getByPlaceholder('Filter…').fill('ComTestNodesCalculate')
  await page.getByRole('menuitem', { name: 'ComTestNodesCalculate', exact: true }).click()
  const node = page.locator('.metteur-node').filter({ hasText: 'ComTestNodesCalculate' })
  await expect(node.locator('input[type=number]')).toHaveValue('7')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  const saved = await page.evaluate(async () => {
    const core = '/src/core/index.ts', storePath = '/src/stores/blueprint.ts'
    const { gateway } = await import(core), { useBlueprintStore } = await import(storePath)
    const store = useBlueprintStore(), catalog = (await gateway.listNodeKinds('D:/metteur-demo/metrics')).data
    catalog.nodes.find((n: { kind: string }) => n.kind === 'ComTestNodesCalculate').addonBinding.fingerprint = 'upgraded'
    catalog.nodes.find((n: { kind: string }) => n.kind === 'ComTestNodesCalculate').pins[0].type = 'string'
    await store.load('D:/metteur-demo/metrics', 'addon.blueprint')
    return { saved: gateway.addonNodeSaved.nodes.find((n: { type: string }) => n.type === 'ComTestNodesCalculate'),
      loaded: store.nodes.find((n: { type: string }) => n.type === 'ComTestNodesCalculate') }
  })
  expect(saved.saved.data['_addon_binding'].fingerprint).toBe('original')
  expect(saved.loaded.data['_addon_binding'].fingerprint).toBe('original')
  expect(saved.saved.inputs[0]).toMatchObject({ key: 'a', name: 'A', type: 'int' })
  expect(saved.loaded.inputs[0]).toMatchObject({ key: 'a', name: 'A', type: 'int' })
})
