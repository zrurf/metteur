import { test, expect, type Page } from '@playwright/test'

async function openWorkspace(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByTestId('file-tree')).toBeVisible()
}

test('legacy IDs retain inline values and instance defaults without reviving cleared overrides', async ({
  page,
}) => {
  await openWorkspace(page)
  const result = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      storePath = '/src/stores/blueprint.ts'
    const { gateway } = await import(corePath)
    const { useBlueprintStore } = await import(storePath)
    gateway.readFile = async () => ({
      ok: true,
      data: {
        content: JSON.stringify({
          id: 'old',
          edges: [],
          nodes: [
            {
              id: 'old-node',
              type: 'Delay',
              title: 'Delay',
              category: 'flow',
              position: { x: 0, y: 0 },
              inputs: [
                {
                  id: 'old-pin',
                  key: 'Ms',
                  name: 'Ms',
                  kind: 'data-in',
                  type: 'string',
                  default: 27,
                },
              ],
              outputs: [],
              data: { 'old-pin': 11, provider: 'mock' },
            },
          ],
        }),
      },
    })
    const store = useBlueprintStore()
    await store.load('test', 'migration.blueprint')
    const node = store.nodes[0],
      pin = node.inputs[0]
    const before = { pin: { ...pin }, value: node.values[pin.id], config: { ...node.data } }
    delete node.values[pin.id]
    await store.load('test', 'migration.blueprint')
    return { before, after: store.nodes[0].values, pinId: store.nodes[0].inputs[0].id }
  })
  expect(result.before.pin).toMatchObject({ name: 'Ms', type: 'int', default: 27 })
  expect(result.before.value).toBe('11')
  expect(result.before.config).toEqual({ provider: 'mock' })
  expect(result.before.pin.id).not.toBe('old-pin')
  expect(result.pinId).toBe(result.before.pin.id)
  expect(result.after).toEqual({})
})

test('wire round trips preserve structural types, defaults, nulls and function metadata', async ({
  page,
}) => {
  await openWorkspace(page)
  const result = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      wirePath = '/src/core/grpc-gateway.ts'
    const libPath = '/src/lib/blueprint.ts',
      catalogPath = '/src/core/node-catalog.ts'
    const { gateway } = await import(corePath)
    const { fromProtoBlueprint, toProtoBlueprint } = await import(wirePath)
    const { makeCallFunctionNode, isPinCompatible } = await import(libPath)
    const { fromWireCatalog } = await import(catalogPath)
    const catalog = (await gateway.listNodeKinds()).data
    const types = [
      'float',
      'int',
      'bool',
      'string',
      'context',
      'json',
      'any',
      'list<int>',
      'object{CaseSensitive:list<float>}',
      'choice',
    ]
    const pins = types.map((type, i) => ({
      id: `pin-${i}`,
      key: `key-${i}`,
      name: `Input${i}`,
      pinType: 'DataInput',
      dataType: type,
      defaultJson: i === 1 ? '7' : '',
      optional: true,
      choices: type === 'choice' ? ['a', 'b'] : [],
      description: `Description ${i}`,
    }))
    const input = {
      id: 'graph',
      name: 'Roundtrip',
      entryNodeId: 'node',
      nodes: [
        {
          id: 'node',
          kind: 'FutureNode',
          nodeType: 'Pure',
          posX: 3,
          posY: 4,
          pins,
          dataJson: JSON.stringify({
            provider: 'mock',
            Input0: 1.5,
            Input1: 2,
            Input2: false,
            Input3: 'hello',
            Input5: { a: 1 },
            Input6: null,
            Input7: [1, 2],
            Input8: { a: [2.5] },
          }),
        },
      ],
      edges: [],
    }
    const domain = fromProtoBlueprint(input)
    const output = toProtoBlueprint(domain)
    const fn = makeCallFunctionNode(
      {
        name: 'LibraryFunction',
        inputs: [
          { name: 'Count', type: 'int', default: 5, optional: true, description: 'How many' },
        ],
        outputs: [{ name: 'Result', type: 'list<int>' }],
      },
      catalog.nodes.find((n: { kind: string }) => n.kind === 'CallFunction'),
      { x: 0, y: 0 },
      'fn',
    )
    const matrix = [
      ['int', 'float'],
      ['float', 'int'],
      ['float', 'string'],
      ['list<float>', 'list<int>'],
      ['object{CaseSensitive:list<float>}', 'object{CaseSensitive:list<int>}'],
      ['context', 'string'],
      ['json', 'context'],
      ['list<int>', 'list<string>'],
      ['object{a:int}', 'object{b:bool}'],
    ].map(([a, b]) => isPinCompatible(a, b))
    return {
      pins: output.nodes[0].pins,
      values: JSON.parse(output.nodes[0].dataJson),
      nodeType: output.nodes[0].nodeType,
      matrix,
      functionKind: fn.data.kind,
      functionName: fn.data.data.function,
      fnPin: fn.data.inputs.find((p: { name: string }) => p.name === 'Count'),
      legacy: fromWireCatalog({ kinds: ['Start'] }),
      partial: fromWireCatalog({ kinds: ['Start'], signatureVersion: 1, infos: [] }),
    }
  })
  expect(result.nodeType).toBe('Pure')
  expect(result.pins.map((p: { dataType: string }) => p.dataType)).toEqual([
    'float',
    'int',
    'bool',
    'string',
    'context',
    'json',
    'any',
    'list<int>',
    'object{CaseSensitive:list<float>}',
    'choice',
  ])
  expect(result.pins[1]).toMatchObject({
    id: 'pin-1',
    defaultJson: '7',
    optional: true,
    description: 'Description 1',
  })
  expect(result.pins[9].choices).toEqual(['a', 'b'])
  expect(result.values).toEqual({
    provider: 'mock',
    Input0: 1.5,
    Input1: 2,
    Input2: false,
    Input3: 'hello',
    Input5: { a: 1 },
    Input6: null,
    Input7: [1, 2],
    Input8: { a: [2.5] },
  })
  expect(result.matrix).toEqual([true, false, true, false, false, false, true, true, true])
  expect(result.functionKind).toBe('CallFunction')
  expect(result.functionName).toBe('LibraryFunction')
  expect(result.fnPin).toMatchObject({
    type: 'int',
    default: 5,
    optional: true,
    description: 'How many',
  })
  expect(result.legacy).toMatchObject({ ready: false, nodes: [], kinds: ['Start'] })
  expect(result.partial).toMatchObject({ ready: false, nodes: [] })
})

test('palette creates an unpreset registered kind with daemon defaults and saves its contract', async ({
  page,
}) => {
  await openWorkspace(page)
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      routePath = '/src/router.ts',
      filePath = '/src/lib/file-token.ts'
    const { gateway } = await import(corePath)
    const catalog = (await gateway.listNodeKinds()).data
    catalog.kinds.push('FutureNode')
    catalog.nodes.push({
      kind: 'FutureNode',
      nodeType: 'Pure',
      dynamicPins: false,
      description: 'Runtime extension',
      pins: [
        {
          id: '',
          key: 'count',
          name: 'Count',
          kind: 'data-in',
          type: 'int',
          default: 7,
          optional: true,
          description: 'How many',
        },
      ],
    })
    gateway.listNodeKinds = async () => ({ ok: true, data: catalog })
    gateway.readFile = async () => ({
      ok: true,
      data: {
        content: gateway.catalogTestFile ? JSON.stringify(gateway.catalogTestFile) : '',
        path: 'catalog.blueprint',
      },
    })
    gateway.writeFile = async (_ws: string, _path: string, content: string) => {
      gateway.catalogTestFile = JSON.parse(content)
      return { ok: true, data: undefined }
    }
    const { router } = await import(routePath)
    const { fileRoute } = await import(filePath)
    await router.push(fileRoute('catalog.blueprint'))
  })
  await expect(page.locator('.blueprint-flow')).toBeVisible()
  await expect(page.getByTestId('node-catalog-status')).toHaveCount(0)
  await page.locator('.vue-flow__pane').click({ button: 'right', position: { x: 450, y: 180 } })
  await page.getByPlaceholder('Filter…').fill('FutureNode')
  await page.getByRole('menuitem', { name: 'FutureNode', exact: true }).click()
  const node = page.locator('.metteur-node').filter({ hasText: 'FutureNode' })
  await expect(node).toBeVisible()
  await expect(node.locator('input[type=number]')).toHaveValue('7')
  await node.locator('input[type=number]').fill('9')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  const saved = await page.evaluate(async () => {
    const path = '/src/stores/blueprint.ts',
      corePath = '/src/core/index.ts'
    const { gateway } = await import(corePath)
    if (!gateway.catalogTestFile?.nodes.some((n: { type: string }) => n.type === 'FutureNode'))
      throw new Error('Catalogue node was not written')
    const { useBlueprintStore } = await import(path)
    const store = useBlueprintStore()
    await store.load('D:/metteur-demo/metrics', 'catalog.blueprint')
    if (store.keyFor(store.nodes, store.edges) !== store.savedKeys['catalog.blueprint'])
      throw new Error('Saved catalogue node became dirty on reload')
    return store.nodes.find((n: { type: string }) => n.type === 'FutureNode')
  })
  expect(saved).toMatchObject({
    type: 'FutureNode',
    nodeType: 'Pure',
    inputs: [{ name: 'Count', type: 'int', default: 7, optional: true }],
  })
  expect(Object.values(saved.values)).toEqual(['9'])
})

test('legacy daemon shows a compatibility state while existing graphs retain pins and wires', async ({
  page,
}) => {
  await openWorkspace(page)
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      routePath = '/src/router.ts',
      filePath = '/src/lib/file-token.ts'
    const catalogPath = '/src/core/node-catalog.ts'
    const { gateway } = await import(corePath)
    const { fromWireCatalog } = await import(catalogPath)
    gateway.listNodeKinds = async () => ({
      ok: true,
      data: fromWireCatalog({ kinds: ['LegacyNode'] }),
    })
    const a = crypto.randomUUID(),
      b = crypto.randomUUID(),
      out = crypto.randomUUID(),
      input = crypto.randomUUID()
    const graph = {
      id: crypto.randomUUID(),
      name: 'Legacy',
      nodes: [
        {
          id: a,
          type: 'LegacyNode',
          title: 'LegacyNode',
          category: 'module',
          position: { x: 20, y: 20 },
          inputs: [],
          outputs: [{ id: out, name: 'Result', kind: 'data-out', type: 'int' }],
        },
        {
          id: b,
          type: 'LegacyNode',
          title: 'LegacyNode',
          category: 'module',
          position: { x: 320, y: 20 },
          outputs: [],
          inputs: [
            {
              id: input,
              name: 'Count',
              kind: 'data-in',
              type: 'float',
              default: 3,
              optional: true,
            },
          ],
        },
      ],
      edges: [
        { id: crypto.randomUUID(), source: a, target: b, sourceHandle: out, targetHandle: input },
      ],
    }
    gateway.readFile = async () => ({
      ok: true,
      data: { content: JSON.stringify(graph), path: 'legacy.blueprint' },
    })
    gateway.writeFile = async (_ws: string, _path: string, content: string) => {
      const saved = JSON.parse(content)
      if (
        saved.edges.length !== graph.edges.length ||
        graph.edges.some((edge, i) =>
          Object.entries(edge).some(([key, value]) => saved.edges[i][key] !== value),
        )
      )
        throw new Error('Legacy edges changed')
      if (saved.nodes[1].inputs[0].id !== input) throw new Error('Legacy pin ID changed')
      gateway.catalogTestSaved = true
      return { ok: true, data: undefined }
    }
    const { router } = await import(routePath)
    const { fileRoute } = await import(filePath)
    await router.push(fileRoute('legacy.blueprint'))
  })
  await expect(page.getByTestId('node-catalog-status')).toContainText(
    'does not provide supported pin signatures',
  )
  await expect(page.locator('.metteur-node')).toHaveCount(2)
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect
    .poll(() =>
      page.evaluate(async () => {
        const path = '/src/core/index.ts'
        return (await import(path)).gateway.catalogTestSaved
      }),
    )
    .toBe(true)
  await page.locator('.vue-flow__pane').click({ button: 'right', position: { x: 450, y: 280 } })
  await page.getByPlaceholder('Filter…').fill('Start')
  await expect(page.getByRole('menuitem')).toHaveCount(0)
})
