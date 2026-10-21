import { defineConfig, devices } from '@playwright/test'

// Deterministic fixture suites. The production daemon flow has its own config
// and owned local provider: use `corepack pnpm test:live`.
const port = Number(process.env.E2E_PORT ?? 5317)
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid E2E_PORT')
const baseURL = `http://127.0.0.1:${port}`
const production = process.env.E2E_PRODUCTION === '1'
const browser = {
  ...devices['Desktop Chrome'],
  channel: process.env.E2E_BROWSER_CHANNEL === 'chromium' ? 'chromium' : undefined,
}

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  maxFailures: 1,
  outputDir: process.env.E2E_OUTPUT_DIR || 'test-results',
  reporter: [['list']],
  use: {
    baseURL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'e2e',
      testMatch: '**/*.spec.ts',
      testIgnore: ['**/monkey/**', '**/live/**'],
      use: browser,
    },
    {
      name: 'monkey',
      testMatch: '**/monkey/**',
      timeout: 180_000,
      use: browser,
    },
  ],
  webServer: {
    command: production
      ? `corepack pnpm exec vite build --outDir .e2e-dist && corepack pnpm exec vite preview --outDir .e2e-dist --host 127.0.0.1 --port ${port} --strictPort`
      : `corepack pnpm dev --host 127.0.0.1 --port ${port} --strictPort`,
    url: baseURL,
    reuseExistingServer: false,
    timeout: 60_000,
    env: { VITE_MOCK: '1', VITE_GW_URL: '' },
  },
})
