import { expect, test } from '@playwright/test'

for (const theme of ['light', 'dark'])
  for (const width of [1280, 800]) {
    test(`shared page structure, controls and keyboard tabs in ${theme} at ${width}px`, async ({
      page,
    }, info) => {
      const errors: string[] = []
      page.on('pageerror', (error) => errors.push(error.message))
      await page.setViewportSize({ width, height: 850 })
      await page.goto('/')
      await page.getByLabel('Workspace path').fill('D:/metteur-demo/design')
      await page.getByRole('button', { name: 'Open', exact: true }).click()
      await expect(page.getByLabel('Message', { exact: true })).toBeVisible()
      await page.evaluate(async (selectedTheme) => {
        const path = '/src/stores/theme.ts'
        ;(await import(path)).useThemeStore().mode = selectedTheme
      }, theme)
      const geometry: {
        size: string
        weight: string
        paddingLeft: string
        paddingRight: string
        offset: number
        paneWidth: number
      }[] = []
      for (const [route, title] of [
        ['tasks', 'Tasks'],
        ['memory', 'Project Memory'],
        ['prompts', 'Prompt Library'],
        ['addon-pages', 'Addon Pages'],
      ]) {
        await page.evaluate(async (destination) => {
          const r = '/src/router.ts',
            u = '/src/lib/workspace-url.ts'
          await (await import(r)).router.push((await import(u)).wurl('/' + destination))
        }, route)
        const heading = page.getByRole('heading', { name: title, level: 1, exact: true })
        await expect(heading).toBeVisible()
        await expect(page.locator('.surface-page h1')).toHaveCount(1)
        geometry.push(
          await heading.evaluate((element) => {
            const style = getComputedStyle(element),
              shell = element.closest('.surface-page')!,
              shellStyle = getComputedStyle(shell)
            return {
              weight: style.fontWeight,
              paneWidth: shell.getBoundingClientRect().width,
              size: style.fontSize,
              paddingLeft: shellStyle.paddingLeft,
              paddingRight: shellStyle.paddingRight,
              offset: Math.round(
                element.getBoundingClientRect().left - shell.getBoundingClientRect().left,
              ),
            }
          }),
        )
        await expect(page.locator('.surface-page')).not.toHaveJSProperty('scrollWidth', 0)
        const overflow = await page
          .locator('.surface-page')
          .evaluate((el) => el.scrollWidth > el.clientWidth + 1)
        expect(overflow).toBe(false)
        await page.screenshot({ path: info.outputPath(`${route}.png`) })
        if (route === 'tasks') {
          await page.getByRole('tab', { name: 'Tasks', exact: true }).focus()
          await page.keyboard.press('ArrowRight')
          await expect(page.getByRole('tab', { name: 'Schedules', exact: true })).toBeFocused()
          await expect(page).toHaveURL(/\/tasks\?view=schedules$/)
          await page.keyboard.press('Home')
          await expect(page.getByRole('tab', { name: 'Tasks', exact: true })).toBeFocused()
        }
        if (route === 'addon-pages')
          await expect(page).toHaveURL(/\/settings\?section=addon-pages$/)
      }
      for (const value of geometry) {
        expect(value.size).toBe('18px')
        expect(value.weight).toBe('600')
        const gutter = value.paneWidth <= 600 ? 16 : 24
        expect(value.paddingLeft).toBe(`${gutter}px`)
        expect(value.paddingRight).toBe(`${gutter}px`)
        expect(value.offset).toBe(gutter)
      }
      expect(errors).toEqual([])
      await expect(
        page.locator(
          '[data-nav-key="addon-pages"], [data-nav-key="schedules"], [data-nav-key="prompts"]',
        ),
      ).toHaveCount(0)
      await expect(page.locator('[title*="Drag to reorder"]')).toHaveCount(0)
    })
  }
