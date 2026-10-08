import { test, expect } from '@playwright/test'

test('readonly addon functions require an explicit named file copy and preserve version binding on canvas', async ({ page }, testInfo) => {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/functions')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
  await page.evaluate(async () => {
    const core = '/src/core/index.ts'
    const { gateway } = await import(core)
    const fn = { id: 'readonly', name: 'ComTestLibrary', description: 'Read-only addon com.test@1.0.0', source: 'addon',
      inputs: [{ name: 'A', type: 'int' }], outputs: [{ name: 'Result', type: 'int' }],
      addonBinding: { package: { id: 'com.test', version: '1.0.0' }, body: 'original' } }
    gateway.listFunctions = async () => ({ ok: true, data: [fn] })
    gateway.importFunction = async (path: string, source: unknown, name: string, file: string) => {
      gateway.importedFunction = { path, source, name, file }
      if (name === 'StaleCopy') return { ok: false, error: 'Addon function changed; reload before importing' }
      return { ok: true, data: { name, filePath: file } }
    }
  })
  await page.getByRole('button', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: 'Addons', exact: true }).click()
  const library = page.getByTestId('addon-function-library')
  await expect(library).toContainText('com.test@1.0.0')
  await expect(library).toContainText('A: int → Result: int')
  await library.getByRole('button', { name: 'Import copy' }).click()
  const modal = page.locator('div.max-w-md').filter({ has: page.getByRole('heading', { name: 'Import editable function copy' }) })
  await modal.getByLabel('New function name').fill('StaleCopy')
  await modal.getByLabel('New blueprint file path').fill('functions/copy.blueprint')
  await modal.getByRole('button', { name: 'Import copy' }).click()
  await expect(library.getByRole('alert')).toContainText('changed; reload')
  await expect(library.getByRole('button', { name: 'Open StaleCopy' })).toHaveCount(0)
  await modal.getByLabel('New function name').fill('EditableCopy')
  await modal.getByRole('button', { name: 'Import copy' }).click()
  await expect(library.getByRole('button', { name: 'Open EditableCopy' })).toBeVisible()
  await library.getByRole('button', { name: 'Open EditableCopy' }).scrollIntoViewIfNeeded()
  await page.screenshot({ path: testInfo.outputPath('addon-function-library.png'), fullPage: true })
  const actual = await page.evaluate(async () => {
    const core = '/src/core/index.ts', blueprint = '/src/lib/blueprint.ts'
    const { gateway } = await import(core), { makeCallFunctionNode } = await import(blueprint)
    const source = gateway.importedFunction.source
    const catalog = (await gateway.listNodeKinds()).data
    const node = makeCallFunctionNode(source, catalog.nodes.find((n: { kind: string }) => n.kind === 'CallFunction'), { x: 100, y: 100 }, 'call-copy')
    source.addonBinding.body = 'upgraded'
    return { imported: gateway.importedFunction, binding: node.data.data['_addon_function_binding'] }
  })
  expect(actual.imported).toMatchObject({ path: 'D:/metteur-demo/functions', name: 'EditableCopy', file: 'functions/copy.blueprint' })
  expect(actual.binding.body).toBe('original')
})
