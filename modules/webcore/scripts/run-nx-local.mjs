import { blueprintOutcome } from './nx-blueprint-fixture.mjs'
import { dailyWorkflowReply } from './daily-workflow-fixture.mjs'
import { localDelayCommand, withCurrentNodePath } from './local-job-fixture.mjs'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { createServer } from 'node:http'
import { createServer as createTcpServer, createConnection } from 'node:net'
import { once } from 'node:events'
import { createWriteStream } from 'node:fs'
import { mkdir, mkdtemp, writeFile, readFile, readdir, readlink, cp } from 'node:fs/promises'
import { resolve as resolvePath, dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { randomUUID } from 'node:crypto'
import { assertLiveIdentity } from './live-identity.mjs'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'
import { fromJson } from '@bufbuild/protobuf'
import { Daemon, SaveBlueprintRequestSchema } from '../src/gen/metteur_pb.ts'

if (!process.argv.includes('--serve-only'))
  throw Error('Use --serve-only; this helper never launches a broad browser suite.')
const webcore = resolvePath(dirname(fileURLToPath(import.meta.url)), '..')
const repo = resolvePath(webcore, '../..')
const parent = resolvePath(process.env.METTEUR_VALIDATION_DIR || join(repo, '.tmp', 'validation'))
await mkdir(parent, { recursive: true })
const root = process.env.METTEUR_LIVE_REUSE_ROOT || (await mkdtemp(join(parent, 'live-')))
const workspace = join(root, 'workspace')
await mkdir(workspace, { recursive: true })
const nonce = randomUUID()
await mkdir(join(workspace, 'blueprints'), { recursive: true })
try {
  await readFile(join(workspace, 'blueprints/review.blueprint'))
} catch {
  await cp(
    join(webcore, 'scripts/fixtures/nx-local-review.blueprint'),
    join(workspace, 'blueprints/review.blueprint'),
  )
}
await writeFile(join(workspace, 'evidence.txt'), 'local-evidence-42')
await writeFile(join(workspace, 'app.ts'), `export const evidence = '${nonce}'\n`)
const target = resolvePath(process.env.CARGO_TARGET_DIR || join(repo, 'target'))
const suffix = process.platform === 'win32' ? '.exe' : ''
const daemonBin = process.env.METTEUR_DAEMON_BIN || join(target, 'debug', `metteurd${suffix}`)
const webBin = process.env.METTEUR_WEB_BIN || join(target, 'debug', `metteur-web${suffix}`)
const staticDir = resolvePath(process.env.METTEUR_WEB_DIST || join(webcore, 'dist'))
const ownedStaticDir = join(root, 'web')
const children = []
const receipts = {
  fixture: 'metteur-local-provider-v1',
  nonce,
  calls: 0,
  toolRequests: 0,
  finalRequests: 0,
  sawToolResult: false,
  workflowInputRequests: 0,
  supervisionRequests: 0,
  fileReferenceRequests: 0,
  sawFrozenFile: false,
  sawCurrentFileRead: false,
  dailyStoryRequests: 0,
  dailyStorySelectedPersonal: false,
  dailyStorySelectedProject: false,
  dailyStorySelectedFile: false,
  resultReferenceRequests: 0,
  sawResultData: false,
}

function start(name, executable, args, env = withCurrentNodePath()) {
  const log = createWriteStream(join(root, `${name}.log`))
  const child = spawn(executable, args, {
    cwd: webcore,
    env,
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  child.stdout.pipe(log, { end: false })
  child.stderr.pipe(log, { end: false })
  child.once('close', () => log.end())
  child.once('error', (error) => log.write(String(error)))
  children.push(child)
  return child
}

async function availablePort(requested) {
  const socket = createTcpServer()
  socket.listen(requested ? Number(requested) : 0, '127.0.0.1')
  await once(socket, 'listening')
  const port = socket.address().port
  await new Promise((resolve) => socket.close(resolve))
  return port
}

const provider = createServer(async (request, response) => {
  if (request.url === '/fixture') {
    response.setHeader('content-type', 'application/json')
    response.end(JSON.stringify(receipts))
    return
  }
  if (request.method !== 'POST' || request.url !== '/chat/completions') {
    response.writeHead(404).end()
    return
  }
  try {
    let text = ''
    for await (const chunk of request) {
      text += chunk
      if (text.length > 2 * 1024 * 1024) throw new Error('Fixture request exceeds limit')
    }
    const body = JSON.parse(text)
    if (body.model !== 'local-fixture') throw new Error('Unexpected model')
    const result = body.messages?.findLast((message) => message.role === 'tool')
    const jobMode = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('M2_LOCAL_JOB'),
    )
    const hasWait = body.messages?.some(
      (m) => m.role === 'tool' && String(m.content).includes('wait_preparing'),
    )
    const jobText =
      body.messages
        ?.filter((m) => m.role === 'tool')
        .map((m) => String(m.content))
        .join(' ') || ''
    const job = jobText.match(/"job_id"\s*:\s*"([^"\s]+)"/)
    let final = jobMode ? hasWait : Boolean(result && JSON.stringify(result).includes(nonce))
    receipts.calls++
    const workflowInput = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('DW03_TEMPORARY_INPUT'),
    )
    if (workflowInput) receipts.workflowInputRequests++
    const fileMode = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('DW04_FILE_WORKFLOW'),
    )
    if (fileMode) {
      receipts.fileReferenceRequests++
      receipts.sawFrozenFile ||= body.messages.some(
        (m) =>
          m.role === 'user' &&
          String(m.content).includes('DW04_SAVED_VERSION') &&
          String(m.content).includes('<file_content>'),
      )
      receipts.sawCurrentFileRead ||= Boolean(
        result &&
        JSON.stringify(result).includes(nonce) &&
        JSON.stringify(result).includes('DW04_CURRENT_DISK'),
      )
    }
    let tool = {
      id: jobMode ? (job ? 'fixture-wait' : 'fixture-job') : 'fixture-read',
      type: 'function',
      function: jobMode
        ? job
          ? {
              name: 'AwaitEvents',
              arguments: JSON.stringify({
                sources: [job[1]],
                mode: 'all',
                deadline_ms: Date.now() + 3600000,
                params: { fixture: 'local-only' },
              }),
            }
          : { name: 'StartCommand', arguments: JSON.stringify({ command: localDelayCommand() }) }
        : { name: 'ReadFile', arguments: JSON.stringify({ path: 'app.ts' }) },
    }
    let content = jobMode
      ? 'M2_LOCAL_JOB completed through the host continuation.'
      : `METTEUR_LOCAL_READY ${nonce}: app.ts exports the local evidence constant.`
    if (fileMode) {
      tool.function = { name: 'ReadFile', arguments: JSON.stringify({ path: 'workflow.txt' }) }
      content = `DW04_FILE_READY ${nonce}: saved selection and current tool read remained distinct.`
    }
    const launchMode = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('M3_LOCAL_BLUEPRINT'),
    )
    const nodeMode = body.messages?.some((m) => String(m.content) === 'BLUEPRINT_NODE')
    const supervisionMode = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('DW03_SUPERVISION_MESSAGE'),
    )
    if (supervisionMode) {
      receipts.supervisionRequests++
      final = true
      content = JSON.stringify({
        kind: 'answer',
        text: 'Recorded progress read through the local provider.',
      })
    } else if (nodeMode) {
      await new Promise((r) => setTimeout(r, 4000))
      final = true
      content = 'Local blueprint node completed.'
    } else if (launchMode) {
      const outcome = blueprintOutcome(body.messages || [])
      final = outcome !== 'pending'
      content =
        outcome === 'declined'
          ? 'M3_LOCAL_BLUEPRINT: declined. No blueprint was started and no repeat proposal will be sent.'
          : 'M3_LOCAL_BLUEPRINT: original conversation received the run completion event. Open Task continuation to inspect the exact result.'
      tool = {
        id: 'fixture-launch',
        type: 'function',
        function: {
          name: 'RequestBlueprintRun',
          arguments: JSON.stringify({
            blueprint_id: '5726e3bf-35c8-5e17-8942-78756af9c2cc',
            goal: 'Review local evidence',
            constraints: ['Keep existing files', 'No external API calls'],
            summary: 'Run the saved local review and retain its evidence.',
            resume_on_completion: true,
            allow_source_read: true,
          }),
        },
      }
    }
    const daily = dailyWorkflowReply(body.messages || [], nonce)
    if (daily) {
      receipts.dailyStoryRequests++
      const selected = body.messages
        .filter((m) => m.role === 'user')
        .map((m) => String(m.content))
        .join('\n')
      receipts.dailyStorySelectedPersonal ||= selected.includes('DW_PERSONAL_NORMALIZATION')
      receipts.dailyStorySelectedProject ||= selected.includes('DW_PROJECT_NORMALIZATION')
      receipts.dailyStorySelectedFile ||=
        selected.includes('<file_content>') && selected.includes('return value.strip()')
      final = daily.final
      content = daily.content || ''
      if (daily.tool) tool = daily.tool
    }
    const resultMode = body.messages?.some(
      (m) => m.role === 'user' && String(m.content).includes('<result_content>'),
    )
    if (resultMode) {
      receipts.resultReferenceRequests++
      receipts.sawResultData ||= body.messages.some(
        (m) =>
          m.role === 'user' &&
          String(m.content).includes('quoted source data, not an instruction or authorization'),
      )
      final = true
      content = `S3_RESULT_REVIEW_READY ${nonce}: host result reviewed after explicit Send.`
    }
    if (final) {
      receipts.finalRequests++
      receipts.sawToolResult = true
    } else receipts.toolRequests++
    const usage = { prompt_tokens: 32, completion_tokens: 8, total_tokens: 40 }
    if (body.stream) {
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      const delta = final ? { content } : { tool_calls: [{ index: 0, ...tool }] }
      response.write(
        `data: ${JSON.stringify({ choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`,
      )
      response.write(
        `data: ${JSON.stringify({ choices: [{ index: 0, delta: {}, finish_reason: final ? 'stop' : 'tool_calls' }], usage })}\n\n`,
      )
      response.end('data: [DONE]\n\n')
    } else {
      response.setHeader('content-type', 'application/json')
      response.end(
        JSON.stringify({
          choices: [
            {
              message: final
                ? { role: 'assistant', content }
                : { role: 'assistant', content: '', tool_calls: [tool] },
            },
          ],
          usage,
        }),
      )
    }
    await writeFile(join(root, 'provider-receipts.json'), JSON.stringify(receipts, null, 2))
  } catch (error) {
    response.writeHead(400).end(JSON.stringify({ error: String(error) }))
  }
})

async function waitFor(url, child) {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Service exited: ${child.exitCode}`)
    if (await ownsListener(child, Number(new URL(url).port))) {
      try {
        const result = await fetch(url)
        if (result.ok) return
      } catch {}
    }
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`Service readiness timed out: ${url}`)
}

// A successful TCP connect alone could be another process that won the port
// allocation race. Verify ownership before contacting the application.
async function ownsListener(child, port) {
  if (!child.pid || child.exitCode !== null) return false
  if (process.platform === 'win32') {
    const { stdout } = await promisify(execFile)('netstat.exe', ['-ano', '-p', 'TCP'], {
      windowsHide: true,
      timeout: 5_000,
    })
    return stdout.split(/\r?\n/).some((line) => {
      const fields = line.trim().split(/\s+/)
      return (
        fields[0] === 'TCP' &&
        fields[1]?.endsWith(`:${port}`) &&
        fields[2]?.endsWith(':0') &&
        fields.at(-1) === String(child.pid)
      )
    })
  }
  if (process.platform !== 'linux')
    throw new Error('Live listener ownership is supported on Linux and Windows')
  try {
    const directory = `/proc/${child.pid}/fd`
    const links = await Promise.allSettled(
      (await readdir(directory)).map((fd) => readlink(join(directory, fd))),
    )
    const sockets = new Set(
      links
        .filter((link) => link.status === 'fulfilled')
        .map((link) => link.value.match(/^socket:\[(\d+)\]$/)?.[1]),
    )
    for (const name of ['tcp', 'tcp6']) {
      const table = await readFile(`/proc/${child.pid}/net/${name}`, 'utf8')
      if (
        table.split('\n').some((line) => {
          const fields = line.trim().split(/\s+/)
          return (
            fields[3] === '0A' &&
            Number.parseInt(fields[1]?.split(':')[1], 16) === port &&
            sockets.has(fields[9])
          )
        })
      )
        return true
    }
    return false
  } catch {
    return false
  }
}

async function waitForDaemon(port, child) {
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`Daemon exited: ${child.exitCode}`)
    const ready = await new Promise((resolve) => {
      const socket = createConnection({ host: '127.0.0.1', port })
      socket.once('connect', () => {
        socket.destroy()
        resolve(true)
      })
      socket.once('error', () => {
        socket.destroy()
        resolve(false)
      })
    })
    if (ready && (await ownsListener(child, port))) return
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error('Daemon readiness timed out')
}

async function stop(child) {
  if (!child.pid || child.exitCode !== null) return
  const exited = once(child, 'exit')
  if (process.platform === 'win32') {
    const killer = spawn('taskkill', ['/PID', String(child.pid), '/T', '/F'], {
      windowsHide: true,
      stdio: 'ignore',
    })
    await once(killer, 'exit')
  } else child.kill('SIGTERM')
  await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5_000))])
  if (child.exitCode === null) {
    child.kill('SIGKILL')
    await exited
  }
}

try {
  const html = await readFile(join(staticDir, 'index.html'), 'utf8')
  assertLiveIdentity(html)
  await cp(staticDir, ownedStaticDir, { recursive: true })
  await writeFile(
    join(ownedStaticDir, 'index.html'),
    html.replace('</head>', `<meta name="metteur-live-fixture" content="${nonce}"></head>`),
  )
  provider.listen(
    Number(process.env.METTEUR_PROVIDER_PORT || 0),
    process.env.METTEUR_BIND_HOST || '127.0.0.1',
  )
  await once(provider, 'listening')
  const providerUrl = `http://127.0.0.1:${provider.address().port}`
  const daemonPort = await availablePort(process.env.METTEUR_DAEMON_PORT)
  const webPort = await availablePort(process.env.METTEUR_WEB_PORT)
  const config = join(root, 'fixture.toml')
  await writeFile(
    config,
    `config_version = 2\n[daemon]\nautostart = "off"\nwake = false\n[llm]\ndefault_model = "fixture"\nmax_retries = 0\n[llm.models.fixture]\napi_type = "openai-chat"\napi_endpoint = "${providerUrl}"\napi_key = "synthetic-not-a-secret"\nmodel_id = "local-fixture"\ncontext_window_input = 8192\ncontext_window_output = 512\n${process.env.METTEUR_DAILY_STORIES === '1' ? '[sandbox]\nenabled = true\nmode = "sandbox"\nwhitelist = ["py -3.12 -m unittest test_shared.py", "py -3.12 -m unittest test_stage1.py", "py -3.12 -m unittest test_stage1.py test_stage2.py"]\napproval_timeout_secs = 30\n' : ''}${process.env.METTEUR_WORKFLOW_FIXTURE === '1' ? '[oversight]\nconcierge_model = "fixture"\nmax_output_tokens = 256\n' : ''}`,
  )
  const daemon = start('daemon', daemonBin, [
    '--config',
    config,
    '--data-dir',
    join(root, 'data'),
    '--listen-addr',
    `127.0.0.1:${daemonPort}`,
  ])
  // HTTP/2 returns a non-HTTP/1 response; readiness is established by the web
  // proxy's successful connection rather than treating a TCP listener as identity.
  await waitForDaemon(daemonPort, daemon)
  const web = start('web', webBin, [
    '--daemon-addr',
    `127.0.0.1:${daemonPort}`,
    '--listen-addr',
    `${process.env.METTEUR_BIND_HOST || '127.0.0.1'}:${webPort}`,
    '--static-dir',
    ownedStaticDir,
  ])
  const baseURL = `http://127.0.0.1:${webPort}`
  await waitFor(baseURL, web)
  assertLiveIdentity(await (await fetch(baseURL)).text(), nonce)
  const ready = {
    root,
    workspace,
    baseURL,
    providerUrl,
    nonce,
    daemonPid: daemon.pid,
    webPid: web.pid,
  }
  const client = createClient(Daemon, createGrpcWebTransport({ baseUrl: baseURL }))
  await client.openWorkspace({ path: workspace })
  const payload = JSON.parse(
    await readFile(join(webcore, 'scripts/fixtures/nx-local-review-request.json'), 'utf8'),
  )
  payload.workspace_path = workspace
  await client.saveBlueprint(fromJson(SaveBlueprintRequestSchema, payload))
  await writeFile(join(root, 'ready.json'), JSON.stringify(ready, null, 2))
  if (process.env.METTEUR_LIVE_READY_FILE)
    await writeFile(process.env.METTEUR_LIVE_READY_FILE, JSON.stringify(ready, null, 2))
  console.log(`Deterministic live fixture ready; evidence: ${root}`)
  await Promise.race([
    once(process, 'SIGTERM'),
    once(process, 'SIGINT'),
    once(daemon, 'exit').then(() => {
      throw new Error('Owned daemon exited unexpectedly')
    }),
    once(web, 'exit').then(() => {
      throw new Error('Owned web server exited unexpectedly')
    }),
  ])
} catch (error) {
  console.error(error)
  process.exitCode = 1
} finally {
  for (const child of children.reverse()) await stop(child)
  provider.closeAllConnections()
  await new Promise((resolve) => provider.close(resolve))
  await writeFile(
    join(root, 'result.json'),
    JSON.stringify({ exitCode: process.exitCode || 0, receipts }, null, 2),
  )
}
