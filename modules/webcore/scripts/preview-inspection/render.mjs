// Operator-owned worker, never a plugin-selected executable, URL or path.
// Input is one bounded HTML snapshot on stdin. No workspace/profile/credentials.
import { chromium } from '@playwright/test'
import { createHash } from 'node:crypto'
let browser
const deadline = setTimeout(async () => { try { await browser?.close() } finally { process.exit(124) } }, 12000)
try {
  let bytes = 0, chunks = []
  for await (const c of process.stdin) { bytes += c.length; if (bytes > 600000) throw Error('Input exceeds limit'); chunks.push(c) }
  const input = JSON.parse(Buffer.concat(chunks).toString('utf8'))
  if (input.schema_version !== 1 || typeof input.html !== 'string' || Buffer.byteLength(input.html) > 262144) throw Error('Invalid snapshot')
  const digest = createHash('sha256').update(input.html).digest('hex')
  if (digest !== input.digest) throw Error('Snapshot digest mismatch')
  const executablePath = process.env.METTEUR_PREVIEW_CHROMIUM || undefined
  browser = await chromium.launch({ headless: true, chromiumSandbox: true, executablePath, timeout: 6000 })
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, offline: true, acceptDownloads: false, permissions: [], reducedMotion: 'reduce' })
  let blockedRequests = 0
  const page = await context.newPage()
  context.on('page', p => { if (p !== page) void p.close() })
  page.on('dialog', d => void d.dismiss())
  const logs = []; let dropped = 0
  const capture = (level, message) => { if (logs.length < 100) logs.push({ level, message: String(message).slice(0, 512) }); else dropped++ }
  page.on('console', m => capture(m.type(), m.text()))
  page.on('pageerror', e => capture('error', e.message))
  const policy = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; form-action 'none'; object-src 'none'; base-uri 'none'; sandbox allow-scripts"
  const target = 'https://metteur-preview.invalid/snapshot'
  let served = false
  await context.route('**/*', route => {
    if (!served && route.request().isNavigationRequest() && route.request().url() === target) {
      served = true
      return route.fulfill({ status: 200, headers: { 'Content-Type': 'text/html; charset=utf-8', 'Content-Security-Policy': policy, 'Cache-Control': 'no-store' }, body: input.html })
    }
    blockedRequests++; return route.abort('blockedbyclient')
  })
  await page.goto(target, { waitUntil: 'load', timeout: 4000 })
  // Fixed settling budget, not an assertion that arbitrary apps finished loading.
  await page.waitForTimeout(250)
  if (page.url() !== target) throw Error('Preview navigated outside the snapshot')
  const metrics = await page.evaluate(() => ({ viewportWidth: innerWidth, viewportHeight: innerHeight, scrollWidth: document.documentElement.scrollWidth, scrollHeight: document.documentElement.scrollHeight, elements: document.querySelectorAll('*').length, imagesWithoutAlt: [...document.images].filter(i => !i.hasAttribute('alt')).length }))
  const picture = await page.screenshot({ type: 'jpeg', quality: 75, fullPage: false, animations: 'disabled', timeout: 3000 })
  if (picture.length > 1500000) throw Error('Screenshot exceeds limit')
  const output = { schema_version: 1, digest, browser_checks: 'completed', engine: 'chromium', viewport: { width: 1280, height: 800 }, metrics, blocked_requests: blockedRequests, console: { entries: logs, dropped, scope: 'bounded isolated snapshot capture' }, screenshot: { mime: 'image/jpeg', base64: picture.toString('base64'), sha256: createHash('sha256').update(picture).digest('hex'), width: 1280, height: 800 }, scope: 'Offline self-contained HTML snapshot; external assets, API data, nested frames and navigation are blocked. Viewport screenshot, not full application acceptance.' }
  await browser.close(); browser = undefined
  process.stdout.write(JSON.stringify(output))
} catch (e) {
  try { await browser?.close() } catch {}
  process.stderr.write(String(e?.message || e).slice(-2000)); process.exitCode = 1
} finally { clearTimeout(deadline) }
