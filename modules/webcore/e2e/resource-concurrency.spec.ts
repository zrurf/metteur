import { expect, test, type Page } from '@playwright/test'

test('returning to the same workspace cannot revive an obsolete selection request', async ({
  page,
}) => {
  await page.goto('/')
  const outcome = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      workspacePath = '/src/stores/workspace.ts'
    const promptPath = '/src/stores/prompt-resources.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const { nextTick } = await import('/node_modules/.vite/deps/vue.js')
    const pinia = createPinia()
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/project/a', locked: true }
    const prompts = (await import(promptPath)).usePromptResourcesStore(pinia)
    let release: (value: unknown) => void = () => {}
    gateway.resourceRequest = () =>
      new Promise((resolve) => {
        release = resolve
      })
    const reference = {
      scope: { scope: 'project', owner: '/project/a' },
      id: 'old',
      revision: 1,
      content_digest: 'old',
    }
    const pending = prompts.addChat(reference).then(
      () => 'accepted',
      () => 'discarded',
    )
    workspace.active = { path: '/project/b', locked: true }
    await nextTick()
    workspace.active = { path: '/project/a', locked: true }
    await nextTick()
    release({
      ok: true,
      data: {
        id: 'obsolete',
        reference,
        name: 'Old request',
        body: 'old',
        purpose: { kind: 'chat' },
      },
    })
    return { result: await pending, selected: prompts.selections.length }
  })
  expect(outcome).toEqual({ result: 'discarded', selected: 0 })
})

test('replacing selections during validation cannot certify an unchecked new selection', async ({
  page,
}) => {
  await page.goto('/')
  const outcome = await page.evaluate(async () => {
    const corePath = '/src/core/index.ts',
      workspacePath = '/src/stores/workspace.ts'
    const promptPath = '/src/stores/prompt-resources.ts'
    const { gateway } = await import(corePath)
    const { createPinia } = await import('/node_modules/.vite/deps/pinia.js')
    const pinia = createPinia()
    const workspace = (await import(workspacePath)).useWorkspaceStore(pinia)
    workspace.active = { path: '/project/a', locked: true }
    const prompts = (await import(promptPath)).usePromptResourcesStore(pinia)
    const snapshot = (id: string) => ({
      id,
      name: id,
      body: id,
      purpose: { kind: 'chat' },
      reference: {
        scope: { scope: 'project', owner: '/project/a' },
        id,
        revision: 1,
        content_digest: id,
      },
    })
    prompts.selections = [{ snapshot: snapshot('original'), status: 'current' }]
    let release: (value: unknown) => void = () => {}
    gateway.resourceRequest = () =>
      new Promise((resolve) => {
        release = resolve
      })
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
    const corePath = '/src/core/index.ts',
      routerPath = '/src/router.ts',
      urlPath = '/src/lib/workspace-url.ts'
    const { gateway } = await import(corePath)
    gateway.connected.value = true
    const entry = (
      scope: string,
      body = `${scope} body`,
      id = scope,
      name = `${scope} prompt`,
      revision = 1,
    ) => ({
      reference: {
        scope: { scope, owner: scope === 'project' ? 'D:/metteur-demo/resource-race' : 'user' },
        id,
        revision,
        content_digest: `${id}-${revision}`,
      },
      content: {
        header: { schema_version: 1, id, name, tags: [] },
        body,
        raw_digest: scope,
        body_digest: scope,
        deleted: false,
        copied_from: null,
      },
    })
    const entries = new Map([
      ['project', entry('project')],
      ['project-other', entry('project', 'other body', 'project-other', 'Other project prompt')],
      ['personal', entry('personal')],
    ])
    const fixture = (gateway.resourceRace = {
      writes: 0,
      writtenId: '',
      writtenRevision: 0,
      finish: () => {},
    })
    gateway.resourceRequest = async (
      method: string,
      _workspace: string,
      payload: {
        operation?: string
        id?: string
        scope: string
        draft?: { id: string; name: string; body: string }
      },
      revision = 0,
    ) => {
      if (method === 'queryPromptResources')
        return {
          ok: true,
          data:
            payload.operation === 'read'
              ? entries.get(payload.id!)
              : [...entries.values()].filter((e) => e.reference.scope.scope === payload.scope),
        }
      if (method === 'writePromptResource') {
        if ('schema_version' in payload.draft!)
          return { ok: false, error: 'Unexpected draft field schema_version' }
        fixture.writes++
        fixture.writtenId = payload.draft!.id
        fixture.writtenRevision = revision
        const updated = entry(
          payload.scope,
          payload.draft!.body,
          payload.draft!.id,
          payload.draft!.name,
          revision + 1,
        )
        return new Promise((resolve) => {
          fixture.finish = () => {
            entries.set(updated.reference.id, updated)
            resolve({ ok: true, data: updated })
          }
        })
      }
      return { ok: false, error: 'Unexpected fixture operation' }
    }
    await (await import(routerPath)).router.push((await import(urlPath)).wurl('/prompts'))
  })
}

async function editPrompt(page: Page, body: string) {
  await page.getByLabel('More actions', { exact: true }).click()
  await page.getByRole('button', { name: 'Open in Editor', exact: true }).click()
  await expect(page.locator('.monaco-editor .view-lines')).toBeVisible()
  await replaceEditor(page, body)
}
async function replaceEditor(page: Page, body: string) {
  await page.locator('.monaco-editor .view-lines').click()
  await page.keyboard.press('ControlOrMeta+a')
  await page.keyboard.type(body)
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(body)
}
async function library(page: Page) {
  await page
    .getByTestId('file-tree')
    .getByRole('button', { name: 'Prompt Library', exact: true })
    .click()
  return page.getByRole('region', { name: 'Prompt Library', exact: true })
}

test('a late Project save cannot close a new Personal draft', async ({ page }) => {
  await openPromptResources(page)
  await editPrompt(page, 'saved project body')
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.writes),
    )
    .toBe(1)
  const surface = await library(page)
  await surface.getByRole('tab', { name: 'Personal', exact: true }).click()
  await expect(surface.getByRole('heading', { name: 'personal prompt', exact: true })).toBeVisible()
  await editPrompt(page, 'keep this personal draft')
  await page.evaluate(async () =>
    (await import('/src/core/index.ts')).gateway.resourceRace.finish(),
  )
  await expect(page.locator('.monaco-editor .view-lines')).toContainText('keep this personal draft')
  await expect(page.getByTestId('tab-active')).toContainText('personal prompt.md')
})

test('filtering the resource list cannot retarget an existing edit draft', async ({ page }) => {
  await openPromptResources(page)
  await editPrompt(page, 'edited original project resource')
  const surface = await library(page)
  await surface.getByLabel('Search loaded prompt resources').fill('Other project')
  await page.getByTestId('tab').filter({ hasText: 'project prompt.md' }).click()
  await page.getByRole('button', { name: 'Save', exact: true }).click()
  await expect
    .poll(() =>
      page.evaluate(
        async () => (await import('/src/core/index.ts')).gateway.resourceRace.writtenId,
      ),
    )
    .toBe('project')
  await page.evaluate(async () =>
    (await import('/src/core/index.ts')).gateway.resourceRace.finish(),
  )
})

test('text entered during a save remains an unsaved draft after its acknowledgement', async ({
  page,
}) => {
  await openPromptResources(page)
  await editPrompt(page, 'saved project body')
  const save = page.getByRole('button', { name: 'Save', exact: true })
  await save.click()
  await expect
    .poll(() =>
      page.evaluate(async () => (await import('/src/core/index.ts')).gateway.resourceRace.writes),
    )
    .toBe(1)
  await expect(save).toBeDisabled()
  await replaceEditor(page, 'newer unsaved personal edits')
  await page.evaluate(async () =>
    (await import('/src/core/index.ts')).gateway.resourceRace.finish(),
  )
  await expect(page.locator('.monaco-editor .view-lines')).toContainText(
    'newer unsaved personal edits',
  )
  await expect(save).toBeEnabled()
  await save.click()
  await expect
    .poll(() =>
      page.evaluate(
        async () => (await import('/src/core/index.ts')).gateway.resourceRace.writtenRevision,
      ),
    )
    .toBe(2)
  await page.evaluate(async () =>
    (await import('/src/core/index.ts')).gateway.resourceRace.finish(),
  )
})
