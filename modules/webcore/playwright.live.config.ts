import { defineConfig, devices } from '@playwright/test'

for (const variable of ['LIVE_BASE_URL', 'LIVE_WORKSPACE', 'LIVE_FIXTURE_URL', 'LIVE_NONCE']) {
  if (!process.env[variable]) throw new Error(`${variable} is required; use pnpm test:live`)
}

export default defineConfig({
  testDir: './e2e/live',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 60_000,
  outputDir: process.env.LIVE_OUTPUT_DIR || 'test-results/live',
  reporter: [['list']],
  use: {
    ...devices['Desktop Chrome'],
    channel: process.env.E2E_BROWSER_CHANNEL === 'chromium' ? 'chromium' : undefined,
    baseURL: process.env.LIVE_BASE_URL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  // The owned production stack is started by scripts/run-live.mjs. There is
  // deliberately no Vite server and no reuse of any existing application.
})
