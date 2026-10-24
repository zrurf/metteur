// Read-only visual audit of the isolated demo, never the user's working instance.
import { chromium, expect } from '@playwright/test'
import { mkdir, writeFile } from 'node:fs/promises'
import { resolve, join } from 'node:path'

const [baseURL, destination, mode = 'after'] = process.argv.slice(2)
if (new URL(baseURL).hostname !== '127.0.0.1') throw Error('Owned loopback preview required')
const output = resolve(destination)
await mkdir(output, { recursive: false })
const browser = await chromium.launch({ headless: true })
const result = { mode, baseURL, started: new Date().toISOString(), pages: [], failures: [] }
const variants =
  mode === 'before' || mode === 'smoke'
    ? [['light', 1280]]
    : mode === 'workspace'
      ? [
          ['light', 800],
          ['dark', 800],
        ]
      : [
          ['light', 1280],
          ['dark', 1280],
          ['light', 800],
          ['dark', 800],
        ]
try {
  for (const [theme, width] of variants) {
    const context = await browser.newContext({
      viewport: { width, height: 900 },
      serviceWorkers: 'block',
    })
    await context.addInitScript((value) => localStorage.setItem('metteur.theme', value), theme)
    const page = await context.newPage()
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    await page.goto(baseURL)
    await expect(page.locator('meta[name="metteur-gateway"]')).toHaveAttribute('content', 'mock')
    await page.getByLabel('Workspace path').fill('D:/metteur-demo/metrics')
    await page.getByRole('button', { name: 'Open', exact: true }).click()
    await expect(page).toHaveURL(/\/work\//)
    const workspaceURL = new URL(page.url()).pathname.split('/').slice(0, 3).join('/')
    const routes = [
      ['chat', `${workspaceURL}/chat`, '.chat-header'],
      ['execution', `${workspaceURL}/execution`, '.run-header'],
      ['tasks', `${workspaceURL}/tasks`, '.surface-page'],
      ['schedules', `${workspaceURL}/tasks?view=schedules`, '.surface-page'],
      ['memory', `${workspaceURL}/memory`, '.surface-page'],
      ['prompts', `${workspaceURL}/prompts`, '.surface-page'],
      ['version', `${workspaceURL}/version`, '.surface-page'],
      ...[
        'general',
        'llm',
        'sandbox',
        'mcp',
        'lsp',
        'addons',
        'addon-pages',
        'versioning',
        'billing',
        'daemon',
        'toml',
      ].map((section) => [`settings-${section}`, `/settings?section=${section}`, '.surface-page']),
    ]
    for (const [name, path, container] of routes.filter(([surface]) =>
      mode === 'workspace'
        ? surface.startsWith('settings-') || ['version', 'prompts'].includes(surface)
        : mode !== 'smoke' || ['chat', 'execution', 'tasks', 'settings-llm'].includes(surface),
    )) {
      try {
        if (mode === 'workspace') {
          const nav = page.getByRole('navigation', { name: 'Main navigation' })
          if (name === 'version')
            await nav.getByRole('button', { name: 'Version Flow', exact: true }).click()
          else if (name === 'prompts') {
            if (!(await page.getByRole('button', { name: 'Prompt Library', exact: true }).count()))
              await nav.getByRole('button', { name: 'Explorer', exact: true }).click()
            await page.getByRole('button', { name: 'Prompt Library', exact: true }).click()
          } else {
            if (new URL(page.url()).pathname !== '/settings') {
              await page.getByRole('button', { name: 'Settings', exact: true }).click()
              await expect(page).toHaveURL(/\/settings(?:\?|$)/)
              await expect(page.locator('.surface-page h1')).not.toHaveText('Version Flow')
            }
            const section = name.slice('settings-'.length)
            const labels = {
              general: 'General',
              llm: 'LLM & Models',
              sandbox: 'Sandbox',
              mcp: 'MCP Servers',
              lsp: 'LSP Servers',
              addons: 'Addons',
              'addon-pages': 'Addon Pages',
              versioning: 'Versioning',
              billing: 'Billing',
              daemon: 'Daemon',
              toml: 'Settings TOML',
            }
            await page.getByRole('button', { name: labels[section], exact: true }).click()
            await expect(page.locator('.surface-page h1')).toHaveText(labels[section])
          }
          if (name === 'prompts') await expect(page).toHaveURL(/\/prompts$/)
          if (name === 'version') await expect(page).toHaveURL(/\/version$/)
        } else await page.goto(baseURL + path)
        const pane = page.locator(container).first()
        await expect(pane).toBeVisible()
        const title =
          mode === 'before'
            ? pane.locator('h1, .chat-header-title, .blueprint-select').first()
            : pane.getByRole('heading', { level: 1 }).first()
        await expect(title).toBeVisible()
        if (mode === 'workspace' && ['prompts', 'version'].includes(name))
          await expect(title).toHaveText(name === 'prompts' ? 'Prompt Library' : 'Version Flow')
        const geometry = await title.evaluate((element, selector) => {
          const style = getComputedStyle(element),
            box = element.getBoundingClientRect()
          const parent = element.closest(selector).getBoundingClientRect()
          return {
            text: element.textContent.trim(),
            size: style.fontSize,
            weight: style.fontWeight,
            x: box.x - parent.x,
            y: box.y - parent.y,
            lineHeight: style.lineHeight,
          }
        }, container)
        const overflow = await pane.evaluate((el) => el.scrollWidth > el.clientWidth + 1)
        const entry = { name, theme, width, ...geometry, overflow }
        result.pages.push(entry)
        if (mode !== 'before') {
          expect(geometry.size).toBe('18px')
          expect(geometry.weight).toBe('600')
          expect(geometry.y).toBeGreaterThanOrEqual(12)
          expect(geometry.y).toBeLessThanOrEqual(16)
          expect(overflow).toBe(false)
          await expect(pane.getByRole('heading', { level: 1 })).toHaveCount(1)
        }
        if (
          (width === 1280 && theme === 'light') ||
          ['chat', 'execution', 'tasks', 'settings-llm'].includes(name)
        )
          await page.screenshot({ path: join(output, `${theme}-${width}-${name}.png`) })
        if (name === 'tasks' && mode !== 'before') {
          // The disconnected demo must keep controls disabled. The existing
          // connected task-panel fixture verifies the moved action separately.
          await expect(page.getByRole('button', { name: 'New task', exact: true })).toBeDisabled()
          await page.getByRole('tab', { name: 'Tasks', exact: true }).focus()
          await page.keyboard.press('ArrowRight')
          await expect(page.getByRole('tab', { name: 'Schedules', exact: true })).toBeFocused()
          await expect(page).toHaveURL(/view=schedules$/)
        }
      } catch (error) {
        result.failures.push({ name, theme, width, error: String(error) })
        await page
          .screenshot({ path: join(output, `${theme}-${width}-${name}-failure.png`) })
          .catch(() => {})
      }
    }
    result.failures.push(...errors.map((error) => ({ theme, width, error })))
    await context.close()
  }
} finally {
  await browser.close()
  result.finished = new Date().toISOString()
  await writeFile(join(output, 'audit.json'), JSON.stringify(result, null, 2))
}
console.log(JSON.stringify({ output, pages: result.pages.length, failures: result.failures }))
if (result.failures.length) process.exitCode = 1
