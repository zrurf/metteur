import { expect, test, type Page } from '@playwright/test'

test('returning to the same workspace cannot revive an obsolete selection request', async ({ page }) => {
  await page.goto('/')
  const outcome = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts', workspacePath = '/src/stores/workspace.ts'
    const promptPath = '/src/stores/prompt-resources.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const { nextTick } = await import('/node_modules/.vite/deps/vue.js')
    const pinia = createPinia()
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/project/a', locked: true }
    const prompts = (await import(promptPath)).usePromptResourcesStore(pinia)
    let release: (value: unknown) => void = () => {}
    gateway.resourceRequest = () => new Promise(resolve => { release = resolve })
    const reference = { scope: { scope: 'project', owner: '/project/a' }, id: 'old', revision: 1, content_digest: 'old' }
    const pending = prompts.addChat(reference).then(() => 'accepted', () => 'discarded')
    workspace.active = { path: '/project/b', locked: true }; await nextTick()
    workspace.active = { path: '/project/a', locked: true }; await nextTick()
    release({ ok: true, data: { id: 'obsolete', reference, name: 'Old request', body: 'old', purpose: { kind: 'chat' } } })
    return { result: await pending, selected: prompts.selections.length }
  })
  expect(outcome).toEqual({ result: 'discarded', selected: 0 })
})

test('replacing selections during validation cannot certify an unchecked new selection', async ({ page }) => {
  await page.goto('/')
  const outcome = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts', workspacePath = '/src/stores/workspace.ts'
    const promptPath = '/src/stores/prompt-resources.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const pinia = createPinia()
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/project/a', locked: true }
    const prompts = (await import(promptPath)).usePromptResourcesStore(pinia)
    const snapshot = (id: string) => ({ id, name: id, body: id, purpose: { kind: 'chat' },
      reference: { scope: { scope: 'project', owner: '/project/a' }, id, revision: 1, content_digest: id } })
    prompts.selections = [{ snapshot: snapshot('original'), status: 'current' }]
    let release: (value: unknown) => void = () => {}
    gateway.resourceRequest = () => new Promise(resolve => { release = resolve })
    const pending = prompts.validateChat()
    prompts.selections = [{ snapshot: snapshot('unchecked'), status: 'current' }]
    release({ ok: true, data: { reference: snapshot('original').reference } })
    return { valid: await pending, selected: prompts.selections[0].snapshot.id }
  })
  expect(outcome).toEqual({ valid: false, selected: 'unchecked' })
})

async function openPromptResources(page: Page) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/resource-race')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async () => {
    const corePath = '/src/core/index.ts', routerPath = '/src/router.ts', urlPath = '/src/lib/workspace-url.ts'
    const { gateway } = await import(corePath)
    gateway.connected.value = true
    const entry = (scope: string, body = `${scope} body`, id = scope, name = `${scope} prompt`) => ({
      reference: { scope: { scope, owner: scope === 'project' ? 'D:/metteur-demo/resource-race' : 'user' }, id, revision: 1, content_digest: id },
      content: { header: { id, name, tags: [] }, body, raw_digest: scope, body_digest: scope, deleted: false, copied_from: null },
    })
    const fixture = gateway.resourceRace = { writes: 0, writtenId: '', finish: () => {} }
    gateway.resourceRequest = async (method: string, _workspace: string, payload: { scope: string; draft?: { id: string } }) => {
      if (method === 'queryPromptResources') return { ok: true, data: payload.scope === 'project' ? [entry('project'), entry('project', 'other body', 'project-other', 'Other project prompt')] : [entry(payload.scope)] }
      if (method === 'writePromptResource') {
        fixture.writes++
        fixture.writtenId = payload.draft?.id ?? ''
        return new Promise(resolve => { fixture.finish = () => resolve({ ok: true, data: entry('project', 'saved project body') }) })
      }
      return { ok: false, error: 'Unexpected fixture operation' }
    }
    await (await import(routerPath)).router.push((await import(urlPath)).wurl('/prompts'))
  })
}

test('a late Project save cannot close a new Personal draft', async ({ page }) => {
  await openPromptResources(page)
  const surface = page.getByRole('region', { name: 'Prompt resources', exact: true })
  await expect(surface.getByRole('heading', { name: 'project prompt', exact: true })).toBeVisible()
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit', exact: true }).click()
  await surface.getByLabel('Prompt body').fill('saved project body')
  await surface.getByRole('button', { name: 'Save revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.writes)).toBe(1)
  await surface.getByRole('button', { name: 'Personal', exact: true }).click()
  await expect(surface.getByRole('heading', { name: 'personal prompt', exact: true })).toBeVisible()
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit', exact: true }).click()
  await surface.getByLabel('Prompt body').fill('keep this personal draft')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.finish())
  await expect(surface.getByLabel('Prompt body')).toHaveValue('keep this personal draft')
})


test('filtering the resource list cannot retarget an existing edit draft', async ({ page }) => {
  await openPromptResources(page)
  const surface = page.getByRole('region', { name: 'Prompt resources', exact: true })
  await expect(surface.getByRole('heading', { name: 'project prompt', exact: true })).toBeVisible()
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit', exact: true }).click()
  await surface.getByLabel('Prompt body').fill('edited original project resource')
  await surface.getByLabel('Search loaded prompt resources').fill('Other project')
  await surface.getByRole('button', { name: 'Save revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.writtenId)).toBe('project')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.finish())
})


test('text entered during a save remains an unsaved draft after its acknowledgement', async ({ page }) => {
  await openPromptResources(page)
  const surface = page.getByRole('region', { name: 'Prompt resources', exact: true })
  await expect(surface.getByRole('heading', { name: 'project prompt', exact: true })).toBeVisible()
  await surface.getByLabel('More actions', { exact: true }).click()
  await surface.getByRole('button', { name: 'Edit', exact: true }).click()
  await surface.getByLabel('Prompt body').fill('saved project body')
  await surface.getByRole('button', { name: 'Save revision', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.writes)).toBe(1)
  await expect(surface.getByRole('button', { name: 'Save revision', exact: true })).toBeDisabled()
  await surface.getByLabel('Prompt body').fill('newer unsaved personal edits')
  await page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.finish())
  await expect(surface.getByLabel('Prompt body')).toHaveValue('newer unsaved personal edits')
  await expect(surface.getByRole('button', { name: 'Save revision', exact: true })).toBeEnabled()
})
