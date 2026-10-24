// Owned, synthetic continuous-edit observation. No model calls or business replay.
import { chromium, expect } from '@playwright/test'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { Daemon } from '../src/gen/metteur_pb.ts'
import { readFile, writeFile, mkdir, appendFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

const [readyPath, destination, hours = '8'] = process.argv.slice(2)
const duration = Number(hours) * 3600000
if (
  !readyPath ||
  !destination ||
  !Number.isFinite(duration) ||
  duration < 60000 ||
  duration > 24 * 3600000
)
  throw Error('ready.json, new output directory and 1 minute–24 hour duration required')
const ready = JSON.parse(await readFile(readyPath, 'utf8')),
  output = resolve(destination)
assert.equal(new URL(ready.baseURL).hostname, '127.0.0.1')
await mkdir(output)
const workspace = join(output, 'workspace')
await mkdir(workspace)
const saved = 'Synthetic continuous edit sample 中文\n'
await writeFile(join(workspace, 'observation.txt'), saved)
const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: ready.baseURL }))
const browser = await chromium.launch({ headless: true }),
  context = await browser.newContext({
    viewport: { width: 1360, height: 900 },
    serviceWorkers: 'block',
  }),
  pages = [await context.newPage(), await context.newPage()]
const report = {
  mode: 'two-window synthetic editing/host ACK/storage/heap observation, no paid model or actual machine sleep',
  readyPath,
  workspace,
  pid: process.pid,
  requestedMs: duration,
  started: new Date().toISOString(),
  status: 'RUNNING',
  samples: 0,
  errors: [],
  heapGuardBytes: [],
  nativePrivateGuards: {},
  maxSampleGapMs: 0,
  longSamplingGaps: [],
}
const started = Date.now()
for (const page of pages) page.on('pageerror', (e) => report.errors.push(e.message))
async function processMemory() {
  if (process.platform !== 'win32') return []
  const ids = [ready.daemonPid, ready.webPid]
  assert(ids.every((id) => Number.isSafeInteger(id) && id > 0))
  const result = await promisify(execFile)(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      `Get-Process -Id ${ids.join(',')} | Select-Object Id,ProcessName,WorkingSet64,PrivateMemorySize64,Handles | ConvertTo-Json -Compress`,
    ],
    { windowsHide: true },
  )
  return JSON.parse(result.stdout)
}
async function resource(action) {
  const response = await client.recovery({
    schemaVersion: 1,
    requestId: crypto.randomUUID(),
    workspacePath: workspace,
    payloadJson: JSON.stringify({ scope: 'project', action }),
  })
  assert.equal(response.errorJson, '')
  return JSON.parse(response.payloadJson)
}
async function status() {
  report.elapsedMs = Date.now() - started
  report.updated = new Date().toISOString()
  await writeFile(join(output, 'status.json'), JSON.stringify(report, null, 2))
}
try {
  for (const page of pages) {
    await page.goto(`${ready.baseURL}/?open=${encodeURIComponent(workspace)}`)
    await page
      .getByTestId('file-tree')
      .getByRole('button', { name: 'observation.txt', exact: true })
      .click()
    await expect(page.locator('.monaco-editor .view-lines')).toContainText(
      'Synthetic continuous edit sample',
    )
    await page.locator('.monaco-editor .view-lines').click()
    await page.keyboard.press('ControlOrMeta+End')
  }
  const cdps = await Promise.all(pages.map((page) => context.newCDPSession(page)))
  let stableRows = null,
    lastSampleAt = Date.now()
  await status()
  while (Date.now() - started < duration) {
    let stop
    try {
      stop = JSON.parse(await readFile(join(output, 'stop-request.json'), 'utf8'))
    } catch (error) {
      if (error.code !== 'ENOENT') throw error
    }
    if (stop) {
      report.status = 'STOPPED'
      report.stopReason = String(stop.reason || 'Requested stop')
      break
    }
    report.samples++
    let rows
    for (let index = 0; index < pages.length; index++) {
      const page = pages[index],
        expected = saved + (index ? 'y' : 'x').repeat(report.samples)
      await page.bringToFront()
      await page.keyboard.insertText(index ? 'y' : 'x')
      await expect
        .poll(
          async () => {
            rows = (await resource({ operation: 'list', after: null })).items
            const texts = rows.filter((r) => !r.discarded && r.kind === 'text')
            for (const text of texts) {
              const actual = (await resource({ operation: 'read', id: text.id })).record
              if (actual.input.raw === expected) return true
            }
            return false
          },
          { timeout: 10000 },
        )
        .toBe(true)
    }
    assert.equal(rows.filter((row) => !row.discarded && row.kind === 'text').length, 2)
    if (report.samples === 3) stableRows = rows.length
    if (stableRows !== null)
      assert(rows.length <= stableRows, 'Recovery metadata rows grew without a new copy')
    const sample = {
      index: report.samples,
      at: new Date().toISOString(),
      elapsedMs: Date.now() - started,
      records: rows.length,
      rawBytesPerCopy: Buffer.byteLength(saved) + report.samples,
      heap: [],
      processes: [],
      sampleGapMs: Date.now() - lastSampleAt,
    }
    if (report.samples > 1) {
      report.maxSampleGapMs = Math.max(report.maxSampleGapMs, sample.sampleGapMs)
      if (sample.sampleGapMs > 90000)
        report.longSamplingGaps.push({ at: sample.at, gapMs: sample.sampleGapMs })
    }
    lastSampleAt = Date.now()
    if (report.samples === 1 || report.samples % 10 === 0) {
      for (let index = 0; index < cdps.length; index++) {
        await cdps[index].send('HeapProfiler.collectGarbage')
        const heap = await cdps[index].send('Runtime.getHeapUsage')
        sample.heap.push(heap)
        report.heapGuardBytes[index] ??= Math.max(64 * 1024 * 1024, heap.usedSize * 1.25)
        assert(
          heap.usedSize <= report.heapGuardBytes[index],
          'Post-GC JS heap exceeded frozen observation guard',
        )
      }
      sample.processes = await processMemory()
      for (const process of sample.processes) {
        report.nativePrivateGuards[process.Id] ??= Math.max(
          256 * 1024 * 1024,
          process.PrivateMemorySize64 * 1.25,
        )
        assert(
          process.PrivateMemorySize64 <= report.nativePrivateGuards[process.Id],
          'Native private bytes exceeded frozen observation guard',
        )
      }
    }
    assert.deepEqual(report.errors, [])
    await appendFile(join(output, 'samples.jsonl'), JSON.stringify(sample) + '\n')
    await status()
    await pages[0].waitForTimeout(Math.min(60000, Math.max(0, duration - (Date.now() - started))))
  }
  assert.equal(await readFile(join(workspace, 'observation.txt'), 'utf8'), saved)
  await pages[0].screenshot({ path: join(output, 'final.png') })
  report.continuousWindowCompleted =
    report.status !== 'STOPPED' &&
    Date.now() - started >= duration &&
    report.longSamplingGaps.length === 0
  if (report.status !== 'STOPPED')
    report.status = report.continuousWindowCompleted ? 'PASSED' : 'COMPLETED_WITH_SAMPLING_GAPS'
} catch (error) {
  report.status = 'FAILED'
  report.error = String(error)
  process.exitCode = 1
} finally {
  await status()
  await browser.close()
  console.log(JSON.stringify(report))
}
