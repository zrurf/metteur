import type { OversightReports } from './oversight'
import type { ConciergeState, ConciergeEvent } from './concierge'
import type { Blackboard, BoardQuery } from './blackboard'
import { ref } from 'vue'
import type { DaemonGateway } from './gateway'
import type {
  AddonInfo,
  ChatContextStats,
  ApprovalRequest,
  Blueprint,
  NodeCatalog,
  BlueprintEdge,
  BlueprintNode,
  BlueprintPin,
  ChatMessage,
  ChatOptions,
  ChatSessionInfo,
  ChatSessionSnapshot,
  ChatUsage,
  DaemonConfig,
  ConfigState,
  ExecStatus,
  ExecutionEvent,
  ExecutionInfo,
  FileContent,
  FileHistoryEntry,
  FileInfo,
  FileTreeNode,
  FunctionItem,
  McpServerInfo,
  Result,
  SnapshotInfo,
  TodoItem,
  UsageSummary,
  JobInfo,
  JobNotice,
  WatchEvent,
  WorkspaceInfo,
} from './types'
import { err, ok } from './types'
import { configScopeOf, configToToml, isConfigDoc, tryParseToml } from '@/lib/toml'

import catalogFixture from './node-catalog.fixture.json'
import { fromWireCatalog } from './node-catalog'

/** Builds a pin list: exec pins first, then data pins, matching UE pin slots. */
function pins(
  outs: Array<[string, string]>,
  ins: Array<[string, string]>,
  execOut = true,
  execIn = true,
): { inputs: BlueprintPin[]; outputs: BlueprintPin[] } {
  const outputs: BlueprintPin[] = []
  if (execOut) outputs.push({ id: 'x-out', name: '', kind: 'exec-out' })
  for (const [id, name] of outs) outputs.push({ id: `${id}-out`, name, kind: 'data-out', type: id })
  const inputs: BlueprintPin[] = []
  if (execIn) inputs.push({ id: 'x-in', name: '', kind: 'exec-in' })
  for (const [id, name] of ins) inputs.push({ id: `${id}-in`, name, kind: 'data-in', type: id })
  return { inputs, outputs }
}

/** Demo workspace the MockGateway manages in memory. */
const DEMO_WS = 'D:/playground/metrics'

/** Demo blueprint modelled on an Unreal-style action graph. */
const DEMO_BLUEPRINT: Blueprint = {
  id: 'bp-build-feature',
  name: 'Build Feature',
  entryNodeId: 'n-start',
  nodes: [
    {
      id: 'n-start',
      type: 'Start',
      category: 'event',
      title: 'Start',
      position: { x: 40, y: 220 },
      ...pins([], [], true, false),
    },
    {
      id: 'n-llm',
      type: 'CallLLM',
      category: 'module',
      title: 'Call LLM',
      position: { x: 320, y: 140 },
      inputs: [
        { id: 'x-in', name: '', kind: 'exec-in' },
        {
          id: 'reasoning_effort-in',
          name: 'Reasoning Effort',
          kind: 'data-in',
          type: 'choice',
          choices: ['none', 'low', 'medium', 'high'],
        },
        { id: 'model-in', name: 'Model', kind: 'data-in', type: 'string' },
        { id: 'prompt-in', name: 'Prompt', kind: 'data-in', type: 'string' },
        { id: 'system-in', name: 'System', kind: 'data-in', type: 'string' },
        { id: 'temperature-in', name: 'Temperature', kind: 'data-in', type: 'number' },
        { id: 'top_p-in', name: 'Top P', kind: 'data-in', type: 'number' },
        { id: 'max_tokens-in', name: 'Max Tokens', kind: 'data-in', type: 'number' },
        { id: 'max_iterations-in', name: 'Max Iterations', kind: 'data-in', type: 'number' },
        { id: 'seed-in', name: 'Seed', kind: 'data-in', type: 'number' },
      ],
      outputs: [
        { id: 'x-out', name: '', kind: 'exec-out' },
        { id: 'result-out', name: 'Result', kind: 'data-out', type: 'string' },
        { id: 'context-out', name: 'Context', kind: 'data-out', type: 'object' },
      ],
      values: {
        'reasoning_effort-in': 'medium',
        'temperature-in': '0.7',
        'max_tokens-in': '2048',
      },
    },
    {
      id: 'n-tool',
      type: 'Tool',
      category: 'action',
      title: 'Git Commit',
      position: { x: 620, y: 80 },
      ...pins([['out', 'Output']], [['command', 'Command']], true, true),
    },
    {
      id: 'n-valid',
      type: 'Validator',
      category: 'flow',
      title: 'Validate Diff',
      position: { x: 620, y: 300 },
      ...pins([], [['target', 'Target']], true, true),
    },
    {
      id: 'n-judge',
      type: 'Judge',
      category: 'flow',
      title: 'Judge Output',
      position: { x: 320, y: 380 },
      ...pins([], [['result', 'Result']], true, true),
    },
    {
      id: 'n-end',
      type: 'End',
      category: 'event',
      title: 'End',
      position: { x: 940, y: 220 },
      ...pins([], [], false, true),
    },
  ],
  edges: [
    { id: 'e1', source: 'n-start', sourceHandle: 'x-out', target: 'n-llm', targetHandle: 'x-in' },
    {
      id: 'e2',
      source: 'n-llm',
      sourceHandle: 'x-out',
      target: 'n-tool',
      targetHandle: 'x-in',
      label: 'then',
    },
    {
      id: 'e3',
      source: 'n-llm',
      sourceHandle: 'result-out',
      target: 'n-judge',
      targetHandle: 'result-in',
    },
    { id: 'e4', source: 'n-tool', sourceHandle: 'x-out', target: 'n-valid', targetHandle: 'x-in' },
    {
      id: 'e5',
      source: 'n-valid',
      sourceHandle: 'x-out',
      target: 'n-end',
      targetHandle: 'x-in',
    },
  ],
}

const NOW = Date.now()

const DEMO_SNAPSHOTS: SnapshotInfo[] = [
  { id: 's3', alias: 'v3', createdAt: NOW - 1_000, message: 'Tool: commit validated diff' },
  { id: 's2', alias: 'v2', createdAt: NOW - 4_000, message: 'CallLLM: refine prompt' },
  { id: 's1', createdAt: NOW - 9_000, message: 'Initial graph' },
]

const DEMO_HISTORY: FileHistoryEntry[] = [
  {
    path: 'src/api.ts',
    snapshotId: 's3',
    op: 'modify',
    at: NOW - 1_000,
    diff: '-2 +4 lines\n-const url = ""\n+const url = "/v2/metrics"',
  },
  { path: 'src/parser.ts', snapshotId: 's3', op: 'modify', at: NOW - 1_000 },
  { path: 'src/index.ts', snapshotId: 's2', op: 'add', at: NOW - 4_000 },
]

const DEMO_ADDONS: AddonInfo[] = [
  { id: 'shell', name: 'shell', version: '1.4.0', description: '', enabled: true, scope: 'global', toolCount: 3, fragmentCount: 1 },
  { id: 'lsp-hints', name: 'lsp-hints', version: '0.9.2', description: '', enabled: true, scope: 'global', toolCount: 2, fragmentCount: 0 },
  { id: 'sky-tools', name: 'sky-tools', version: '0.2.0', description: '', enabled: false, scope: 'workspace', toolCount: 1, fragmentCount: 0 },
]

const DEMO_USAGE: UsageSummary = {
  currency: 'USD',
  totalCostMicros: 3_210_000,
  models: [
    { model: 'claude-4', calls: 18, inputTokens: 1_100_000, outputTokens: 550_000, reasoningTokens: 0, costMicros: 2_400_000, cachedInputTokens: 820_000, cacheWriteInputTokens: 140_000 },
    { model: 'deepseek-v4', calls: 24, inputTokens: 668_500, outputTokens: 100_000, reasoningTokens: 0, costMicros: 810_000, cachedInputTokens: 410_000, cacheWriteInputTokens: 0 },
  ],
}

/** Demo task list shown by the mock gateway. */
const DEMO_TODOS: TodoItem[] = [
  { content: 'Read the existing parser', status: 'completed' },
  { content: 'Add the fallback branch', status: 'in_progress', activeForm: 'Adding the fallback branch' },
  { content: 'Run the test suite', status: 'pending' },
]

const DEMO_MCP: McpServerInfo[] = [
  { name: 'filesystem', status: 'Connected', toolCount: 2, error: '' },
  { name: 'fetch', status: 'Connected', toolCount: 1, error: '' },
]

/** Demo workspace file tree backing the explorer/editors. */
const DEMO_TREE: FileTreeNode[] = [
  {
    name: 'metrics',
    path: 'metrics',
    kind: 'dir',
    children: [
      {
        name: 'blueprints',
        path: 'metrics/blueprints',
        kind: 'dir',
        children: [
          {
            name: 'build-feature.blueprint',
            path: 'metrics/blueprints/build-feature.blueprint',
            kind: 'file',
          },
          { name: 'deploy.blueprint', path: 'metrics/blueprints/deploy.blueprint', kind: 'file' },
        ],
      },
      {
        name: 'src',
        path: 'metrics/src',
        kind: 'dir',
        children: [
          { name: 'api.ts', path: 'metrics/src/api.ts', kind: 'file' },
          { name: 'parser.ts', path: 'metrics/src/parser.ts', kind: 'file' },
          { name: 'collect.mbp', path: 'metrics/src/collect.mbp', kind: 'file' },
          { name: 'report.mbp', path: 'metrics/src/report.mbp', kind: 'file' },
        ],
      },
      { name: 'package.json', path: 'metrics/package.json', kind: 'file' },
      { name: 'README.md', path: 'metrics/README.md', kind: 'file' },
    ],
  },
]

/**
 * Demo "recorded" file contents for the snapshot diff view.
 *
 * The working tree holds the current text (see `DEMO_FILES`); these are the
 * versions a snapshot would carry, so a comparison shows real changes without
 * a daemon behind the mock.
 */
const DEMO_SNAPSHOT_FILES = new Map<string, string>([
  [
    // An older revision than the working copy, so the comparison shows changes.
    'metrics/src/api.ts',
    `const url = "/v1/metrics";
export async function fetchMetrics() {
  return fetch(url).then((r) => r.json())
}
`,
  ],
  [
    'metrics/README.md',
    `# Metrics

A tiny telemetry workspace.
`,
  ],
])

/**
 * Mutable demo file contents keyed by path so edits persist for the session.
 *
 * The keys mirror the tree paths (workspace-relative, including the demo
 * folder): the explorer hands back exactly those paths, so a key without the
 * folder prefix would make every demo file unopenable.
 */
/**
 * Demo blueprint DSL source.
 *
 * Shaped like the real DSL so tokenization, completion and the "to blueprint"
 * action have something to work with in the mock.
 */
function MBP_SOURCE(name: string): string {
  return [
    `# ${name}`,
    `blueprint "${name}"`,
    '',
    'entry start: Start()',
    '  -> load: ReadFile(path="metrics/src/api.ts")',
    '  -> done: CallLLM(prompt="Summarize the file")',
    '',
    '$done.result -> start.summary',
    '',
  ].join('\n')
}

const DEMO_FILES = new Map<string, string>([
  [
    'metrics/package.json',
    `{\n  "name": "metrics",\n  "scripts": {\n    "build": "tsc && vite build"\n  },\n  "dependencies": {}\n}\n`,
  ],
  [
    'metrics/README.md',
    `# Metrics\n\nA tiny Playwright-driven telemetry workspace used to exercise Metteur.\n\n## Blueprints\n\nOpen a \`.blueprint\` file to design an agent graph.\n`,
  ],
  [
    'metrics/src/api.ts',
    `const url = "/v2/metrics";\nexport async function fetchMetrics() {\n  return fetch(url).then((r) => r.json());\n}\n`,
  ],
  [
    'metrics/src/parser.ts',
    `export function parseLine(line: string): number {\n  return Number.parseInt(line.trim(), 10);\n}\n`,
  ],
  ['metrics/src/collect.mbp', MBP_SOURCE('Collect Samples')],
  ['metrics/src/report.mbp', MBP_SOURCE('Report Samples')],
])

const BLUEPRINT_SOURCE = (id: string): string =>
  `{
  "id": "${id}",
  "name": "Build Feature",
  "entryNodeId": "n-start",
  "nodes": [
    { "id": "n-start", "type": "Start", "title": "Start" },
    { "id": "n-llm", "type": "CallLLM", "title": "Call LLM" }
  ]
}`

/** Simulation state for a workspace so a demo run can be paused/cancelled/approved. */
interface RunState {
  status: ExecStatus
  onEvent?: (e: ExecutionEvent) => void
  aborted: boolean
  paused: boolean
  pendingApproval: boolean
  approvalId: string | null
}

const STEPS: Array<
  Pick<ExecutionEvent, 'kind' | 'message' | 'detail'> & { nodeId: string; delay: number }
> = [
  { nodeId: 'n-start', kind: 'started', message: 'start', delay: 200 },
  { nodeId: 'n-llm', kind: 'started', message: 'calling claude-4', delay: 160 },
  {
    nodeId: 'n-llm',
    kind: 'message',
    message: 'produced diff for feature/x',
    detail: { model: 'claude-4', tokens: 12_300 },
    delay: 400,
  },
  {
    nodeId: 'n-llm',
    kind: 'context',
    message: 'context snapshot',
    detail: {
      regions: [
        { region: 'system', chars: 6800, tokens: 1700 },
        { region: 'user', chars: 2400, tokens: 600 },
        { region: 'assistant', chars: 5200, tokens: 1300 },
        { region: 'tool', chars: 3600, tokens: 900 },
      ],
    },
    delay: 100,
  },
  { nodeId: 'n-llm', kind: 'finished', message: 'llm done', delay: 120 },
  { nodeId: 'n-tool', kind: 'started', message: 'requires approval', delay: 60 },
  {
    nodeId: 'n-tool',
    kind: 'approval_request',
    message: 'git commit -m "feat: x"',
    detail: { approvalId: 'a1', tool: 'Git', command: 'git commit -m "feat: x"' },
    delay: 0,
  },
  { nodeId: 'n-tool', kind: 'finished', message: 'commit ok', delay: 700 },
  { nodeId: 'n-valid', kind: 'started', message: 'validating diff', delay: 240 },
  { nodeId: 'n-valid', kind: 'message', message: 'diff validated (12 lines)', delay: 120 },
  { nodeId: 'n-valid', kind: 'finished', message: 'validated', delay: 60 },
  { nodeId: 'n-end', kind: 'started', message: 'done', delay: 100 },
]

const delay = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** Approximate size of one streamed chunk (a real provider sends a few tokens). */
const STREAM_CHUNK = 24

/**
 * Splits streamed text into chunks without breaking surrogate pairs.
 *
 * Streaming per character would be closer to "typing", but a hidden or busy
 * tab throttles timers hard enough to stretch a short answer into minutes —
 * chunked delivery keeps the demo (and its e2e checks) responsive.
 */
function* streamChunks(text: string | undefined): Generator<string> {
  if (!text) return
  const chars = Array.from(text)
  for (let index = 0; index < chars.length; index += STREAM_CHUNK) {
    yield chars.slice(index, index + STREAM_CHUNK).join('')
  }
}

/** Deep clone a JSON-able value so config layers stay immutable per read. */
function deepClone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T
}

/** Monotonic counter so every streamed chat message gets a unique id. */
let msgSeq = 0
const msgId = () => `m-${Date.now()}-${msgSeq++}`

/**
 * Dev/demo gateway returning deterministic fixtures.
 *
 * No network is involved; every call resolves locally so the full UI can be
 * built and validated before the real Connect transport lands.
 */
export class MockGateway implements DaemonGateway {
  readonly connected = ref(false)
  readonly demo = true
  private runs = new Map<string, RunState>()
  private approved = new Map<string, boolean>()
  /** Workspaces with an abort requested for an in-flight chat response. */
  private chatAbort = new Set<string>()
  /** Conversations the demo has streamed, keyed by workspace. */
  private chatLog = new Map<string, ChatMessage[]>()
  /** Demo jobs, so the job panel has something to list outside a real run. */
  private jobs = new Map<string, JobInfo>()
  /** Open workspaces in memory so the tab strip can switch/close real entries. */
  private openWs = new Set<string>([DEMO_WS])

  async connect(): Promise<Result<void>> {
    return ok(undefined)
  }

  // Workspaces ----------------------------------------------------------------
  async listWorkspaces(): Promise<Result<WorkspaceInfo[]>> {
    await delay(150)
    return ok([...this.openWs].map((path) => ({ path, locked: true })))
  }

  async openWorkspace(path: string): Promise<Result<WorkspaceInfo>> {
    await delay(200)
    if (!path) return err('workspace path is required')
    this.openWs.add(path)
    return ok({ path, locked: true })
  }

  async closeWorkspace(path: string): Promise<Result<void>> {
    await delay(80)
    this.openWs.delete(path)
    this.runs.delete(path)
    return ok(undefined)
  }

  // Configuration -------------------------------------------------------------
  /** In-memory config layers keyed by `''` (global) or workspace path. */
  private configLayers = new Map<string, DaemonConfig>([
    // Demo models so the mock build has a usable chat out of the box; the real
    // gateway never falls back to sample models.
    [
      '',
      {
        llm: {
          default_model: 'demo-chat',
          models: {
            'demo-chat': {
              display_name: 'Demo Chat',
              api_type: 'openai-chat',
              api_endpoint: 'https://api.example.com/v1',
              model_id: 'demo-chat-v1',
              api_key: 'sk-demo',
            },
            'demo-reasoner': {
              display_name: 'Demo Reasoner',
              api_type: 'openai-chat',
              api_endpoint: 'https://api.example.com/v1',
              model_id: 'demo-reasoner-v1',
              api_key: 'sk-demo',
            },
          },
        },
      },
    ],
  ])

  async getConfigState(workspacePath = ''): Promise<Result<ConfigState>> {
    const r = await this.getConfig(workspacePath)
    if (!r.ok) return r
    const raw = r.data
    return ok({ defaults: {}, raw, overrides: { ...raw, config_version: 2 }, effective: raw, legacy: false })
  }

  async getConfig(workspacePath = ''): Promise<Result<DaemonConfig>> {
    await delay(60)
    return ok(deepClone(this.configLayers.get(workspacePath) ?? {}))
  }

  async setConfig(config: DaemonConfig, workspacePath = ''): Promise<Result<void>> {
    await delay(80)
    this.configLayers.set(workspacePath, deepClone(config))
    return ok(undefined)
  }

  // File explorer / editors ---------------------------------------------------
  async listFiles(_ws: string, _dir: string): Promise<Result<FileTreeNode[]>> {
    await delay(80)
    return ok(DEMO_TREE)
  }

  async readFile(ws: string, filePath: string): Promise<Result<FileContent>> {
    if (isConfigDoc(filePath)) {
      const scopePath = configScopeOf(filePath) === 'user' ? '' : ws
      const cfg = await this.getConfig(scopePath)
      return cfg.ok ? ok({ content: configToToml(cfg.data), language: 'toml' }) : cfg
    }
    await delay(90)
    const key = filePath.replace(/^[\\/]+/i, '')
    if (key.endsWith('.blueprint')) {
      const name =
        key
          .split(/[\\/]/)
          .pop()
          ?.replace(/\.blueprint$/, '') ?? 'blueprint'
      return ok({ content: BLUEPRINT_SOURCE(name), language: 'blueprint' })
    }
    const content = DEMO_FILES.get(key)
    if (content === undefined) return err(`file not found: ${filePath}`)
    return ok({ content, language: key.endsWith('.json') ? 'json' : 'text' })
  }

  async writeFile(ws: string, filePath: string, content: string): Promise<Result<void>> {
    if (isConfigDoc(filePath)) {
      const parsed = tryParseToml(content)
      if (!parsed) return err('Saved content is not valid TOML')
      const scopePath = configScopeOf(filePath) === 'user' ? '' : ws
      return this.setConfig(parsed, scopePath)
    }
    await delay(120)
    const key = filePath.replace(/^[\\/]+/i, '')
    if (!key.endsWith('.blueprint')) DEMO_FILES.set(key, content)
    return ok(undefined)
  }

  async createDir(_ws: string, _dirPath: string): Promise<Result<void>> {
    await delay(80)
    return ok(undefined)
  }

  async removeFile(_ws: string, filePath: string): Promise<Result<void>> {
    await delay(80)
    DEMO_FILES.delete(filePath.replace(/^[\\/]+/i, ''))
    return ok(undefined)
  }

  async revealInExplorer(_ws: string, _path: string): Promise<Result<void>> {
    await delay(80)
    return ok(undefined)
  }

  async renameFile(_ws: string, from: string, to: string): Promise<Result<void>> {
    await delay(80)
    const key = from.replace(/^[\\/]+/i, '')
    const content = DEMO_FILES.get(key)
    if (content !== undefined) {
      DEMO_FILES.delete(key)
      DEMO_FILES.set(to.replace(/^[\\/]+/i, ''), content)
    }
    return ok(undefined)
  }

  async statFile(_ws: string, path: string): Promise<Result<FileInfo>> {
    await delay(60)
    const key = path.replace(/^[\\/]+/i, '')
    const isDir = !key.includes('.')
    return ok({ path, isDir, len: isDir ? 0 : (DEMO_FILES.get(key)?.length ?? 0) })
  }

  /** Demo jobs, oldest first, as the real daemon reports them. */
  private jobList(): JobInfo[] {
    return [...this.jobs.values()].sort((a, b) => a.startedAt - b.startedAt)
  }

  // Background commands (jobs) ---------------------------------------------------
  async listJobs(_ws: string): Promise<Result<JobInfo[]>> {
    await delay(60)
    return ok(this.jobList())
  }

  async watchJobs(
    _ws: string,
    onEvent: (e: JobNotice) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>> {
    // A demo job that runs once per subscription so the job panel has live
    // content to show: a short build that prints lines and then exits.
    const id = `demo${Math.floor(Math.random() * 9000 + 1000)}`
    const started = Date.now()
    const script = [
      '$ pnpm build',
      'vite v8 building for production...',
      'transforming...',
      '✓ 191 modules transformed.',
      'dist/assets/index.js  148.20 kB │ gzip: 46.10 kB',
      '✓ built in 2.41s',
    ]
    const push = (notice: JobNotice) => {
      const job = this.jobs.get(id)
      if (job) {
        if (notice.kind === 'output') job.tail += notice.chunk
        job.state = notice.state
        job.exitCode = notice.exitCode
      }
      onEvent(notice)
    }
    this.jobs.set(id, {
      id,
      command: 'pnpm build',
      cwd: 'metrics',
      state: 'running',
      exitCode: -1,
      runId: 'demo-run',
      startedAt: started,
      finishedAt: 0,
      outputBytes: 0,
      tail: '',
    })
    push({
      jobId: id,
      kind: 'started',
      chunk: '',
      state: 'running',
      exitCode: -1,
      summary: `${id} | running | 0.0s | 0 bytes | pnpm build`,
    })
    for (const line of script) {
      if (signal?.aborted) return ok(undefined)
      await delay(900)
      push({
        jobId: id,
        kind: 'output',
        chunk: `${line}
`,
        state: 'running',
        exitCode: -1,
        summary: '',
      })
    }
    await delay(600)
    if (signal?.aborted) return ok(undefined)
    const job = this.jobs.get(id)
    if (job) {
      job.finishedAt = Date.now()
      job.outputBytes = job.tail.length
    }
    push({
      jobId: id,
      kind: 'finished',
      chunk: '',
      state: 'exited',
      exitCode: 0,
      summary: `${id} | exit 0 | 6.0s | ${job?.tail.length ?? 0} bytes | pnpm build`,
    })
    return new Promise((resolve) => {
      signal?.addEventListener('abort', () => resolve(ok(undefined)), { once: true })
      if (signal?.aborted) resolve(ok(undefined))
    })
  }

  async killJob(_ws: string, jobId: string): Promise<Result<{ killed: boolean; state: string }>> {
    await delay(60)
    const job = this.jobs.get(jobId)
    if (!job) return err(`job ${jobId} not found`)
    const killed = job.state === 'running'
    if (killed) {
      job.state = 'killed'
      job.exitCode = -1
      job.finishedAt = Date.now()
      // The streaming loop notices the state change and stops.
      void this.jobKillers
    }
    return ok({ killed, state: job.state })
  }

  async getFileAtSnapshot(
    _ws: string,
    path: string,
    snapshotId?: string,
  ): Promise<Result<{ found: boolean; content: string; snapshotId: string }>> {
    await delay(60)
    // Demo history: the snapshot holds the "before" text, the working tree the
    // current one, so the diff view always has something to show.
    const id = snapshotId || 'demo-snapshot'
    const recorded = DEMO_SNAPSHOT_FILES.get(path.replace(/^[/]+/, ''))
    if (recorded === undefined) return ok({ found: false, content: '', snapshotId: id })
    return ok({ found: true, content: recorded, snapshotId: id })
  }

  async watchWorkspace(
    _ws: string,
    _onEvent: (e: WatchEvent) => void,
    signal?: AbortSignal,
  ): Promise<Result<void>> {
    // The demo gateway never pushes changes; the subscription stays open until
    // the caller aborts, mirroring the real server stream.
    return new Promise((resolve) => {
      signal?.addEventListener('abort', () => resolve(ok(undefined)), { once: true })
    })
  }

  // ReAct chat ----------------------------------------------------------------
  async sendChat(
    ws: string,
    content: string,
    history: ChatMessage[],
    onMessage: (m: ChatMessage) => void,
    options?: ChatOptions,
    onSession?: (sessionId: string, checkpointId?: string) => void,
    _sessionId?: string,
    onUsage?: (usage: ChatUsage) => void,
    onTodos?: (todos: TodoItem[]) => void,
    signal?: AbortSignal,
    onProgress?: (progress: { name: string; bytes: number }) => void,
    onContext?: (stats: ChatContextStats) => void,
    onApproval?: (request: { requestId: string; detail: string }) => void,
  ): Promise<Result<void>> {
    this.chatAbort.delete(ws)
    // The demo never blocks on a decision; the parameter exists so a caller can
    // pass the callback without the signature diverging from the real gateway.
    void onApproval
    const aborted = () => this.chatAbort.has(ws) || signal?.aborted === true
    const log = this.chatLog.get(ws) ?? []
    this.chatLog.set(ws, log)
    const checkpoint = `chat-checkpoint-${crypto.randomUUID()}`
    log.push({ id: `u-${Date.now()}`, role: 'user', content, createdAt: Date.now(), checkpoint })
    onSession?.('demo', checkpoint)
    const model = options?.model ?? 'demo-chat'
    const wantBlueprint = /blueprint|agent|graph|自动化|蓝图/i.test(content)
    const steps: Array<{
      role: ChatMessage['role']
      text: string
      /** Streamed before the answer, shown as a collapsible thinking block. */
      reasoning?: string
      actor?: string
      detail?: Record<string, unknown>
    }> = [
      {
        role: 'assistant',
        reasoning: wantBlueprint
          ? 'The request mentions a blueprint, so the answer should describe the graph shape rather than act on files. Keep the exec path single-output and branch only where a decision changes the flow.'
          : 'Break the request into steps, run the read-only tools first, then report what changed. Keep the answer short and point at the files that matter.',
        text: wantBlueprint
          ? `I'll wire up a blueprint that ${content.trim() || 'runs a small agent'}.\n\n**Plan**\n\n1. Read the target files\n2. Draft the graph\n3. Verify with a \`Validator\`\n\nEach step is a node; the LLM only runs inside \`CallLLM\`.`
          : `Got it — ${content.trim() || 'tell me more about what to build.'}\n\nI'll break this into steps:\n\n- explore the workspace\n- run the relevant tools in the sandbox\n- report what changed\n\nThe shape of a call looks like this:\n\n\`\`\`ts\nexport async function fetchMetrics() {\n  return fetch('/v2/metrics').then((r) => r.json())\n}\n\`\`\`\n\nAsk me to run a step and I'll keep you updated here.`,
        detail: {
          model,
          temperature: options?.temperature,
          top_p: options?.top_p,
          max_tokens: options?.max_tokens,
          reasoning_effort: options?.reasoning_effort,
        },
      },
    ]
    if (wantBlueprint) {
      steps.push({
        role: 'tool',
        actor: 'EditFile',
        // A unified diff, so the transcript exercises diff colouring.
        text: '--- a/metrics/src/api.ts\n+++ b/metrics/src/api.ts\n@@ -1,4 +1,6 @@\n const url = "/v2/metrics";\n+const retries = 3;\n export async function fetchMetrics() {\n   return fetch(url).then((r) => r.json())\n }\n',
        detail: { tool: 'EditFile', file: 'metrics/src/api.ts' },
      })
      steps.push({
        role: 'tool',
        actor: 'blueprint.write',
        text: 'write build-feature.blueprint (start → call-llm → branch → end)',
        detail: { tool: 'blueprint.write', file: 'blueprints/build-feature.blueprint' },
      })
      steps.push({
        role: 'assistant',
        text: `Done — opened \`build-feature.blueprint\` in the editor. The exec flow is single-output from Start, branches on the LLM result, and re-joins at End. Adjust the prompts or add tools from here.`,
        detail: { model, tokens: history.length * 120 },
      })
    }
    for (const step of steps) {
      if (aborted()) return ok(undefined)
      const base = { actor: step.actor, detail: step.detail }
      const id = msgId()
      if (step.role === 'tool') {
        // Arguments are composed before the call exists; the demo reports the
        // same progress the daemon sends for a large edit.
        onProgress?.({ name: step.actor ?? '', bytes: 0 })
        await delay(80)
        onProgress?.({ name: step.actor ?? '', bytes: (step.text?.length ?? 0) * 4 })
        await delay(80)
        // A tool call is visible while it runs, not only when it returns.
        onMessage({
          id,
          role: 'tool',
          ...base,
          content: '',
          detail: { ...step.detail, callId: id, running: true, summary: step.actor },
          createdAt: Date.now(),
          pending: true,
        })
        await delay(120)
        if (aborted()) return ok(undefined)
        onMessage({
          id,
          role: 'tool',
          ...base,
          content: (step.text.split('\n').slice(-3).join('\n') || '').trim(),
          detail: { ...step.detail, callId: id, progress: true },
          createdAt: Date.now(),
          pending: true,
        })
        await delay(120)
      }
      // Reasoning streams first on its own channel, then the answer text.
      for (const chunk of streamChunks(step.reasoning)) {
        if (aborted()) return ok(undefined)
        onMessage({
          id,
          role: step.role,
          ...base,
          content: '',
          reasoning: chunk,
          reasoningPending: true,
          createdAt: Date.now(),
          pending: true,
        })
        await delay(16)
      }
      // Stream the answer in small chunks so the bubble visibly fills in.
      for (const chunk of streamChunks(step.text)) {
        if (aborted()) return ok(undefined)
        onMessage({
          id,
          role: step.role,
          ...base,
          content: chunk,
          createdAt: Date.now(),
          pending: true,
        })
        await delay(16)
      }
      // Settle the bubble with the complete text, exercising the replace path.
      const settled: ChatMessage = {
        id,
        role: step.role,
        ...base,
        content: step.text,
        reasoning: step.reasoning,
        reasoningPending: false,
        detail: { ...step.detail, ok: true, elapsedMs: 240 },
        createdAt: Date.now(),
      }
      onMessage(settled)
      log.push(settled)
    }
    onTodos?.(DEMO_TODOS)
    onContext?.({
      tokens: 4200 + content.length * 3,
      limit: 128_000,
      assumedLimit: true,
      regions: [
        { region: 'system', tokens: 3100 },
        { region: 'user', tokens: 600 + content.length * 3 },
        { region: 'assistant', tokens: 900 },
        { region: 'tool', tokens: 1400 },
      ],
    })
    onUsage?.({
      inputTokens: history.length * 120,
      outputTokens: content.length * 2,
      totalTokens: history.length * 120 + content.length * 2,
      cachedInputTokens: Math.round(history.length * 120 * 0.6),
      cacheWriteInputTokens: 0,
    })
    this.chatAbort.delete(ws)
    return ok(undefined)
  }

  /** The demo workspace path, standing in for the OS folder dialog. */
  async pickDirectory(): Promise<Result<string | null>> {
    await delay(120)
    return ok('D:/metteur-demo/metrics')
  }

  async abortChat(ws: string): Promise<Result<void>> {
    this.chatAbort.add(ws)
    return ok(undefined)
  }

  async sendInterrupt(
    _ws: string,
    _message: string,
    priority: 'Normal' | 'Urgent' | 'Emergency' = 'Normal',
  ): Promise<Result<void>> {
    // The demo has no engine to inject into; reporting success is what lets the
    // queue UI be exercised.
    void priority
    return ok(undefined)
  }

  async listChatSessions(ws: string): Promise<Result<ChatSessionInfo[]>> {
    const transcript = this.chatLog.get(ws) ?? []
    if (!transcript.length) return ok([])
    const turns = transcript.filter((m) => m.role === 'user' || m.role === 'assistant')
    const at = (index: number) => transcript.at(index)?.createdAt ?? Date.now()
    return ok([
      {
        sessionId: 'demo',
        createdAt: at(0),
        updatedAt: at(-1),
        turns: transcript.filter((m) => m.role === 'user').length,
        title: (transcript.find((m) => m.role === 'user')?.content ?? 'Demo chat').slice(0, 80),
        messageCount: turns.length,
      },
    ])
  }

  /** Returns a demo snapshot so the mock exercises session restore, including
   *  the task list the chat surface renders above the composer. */
  async getChatSession(ws: string, _sessionId?: string): Promise<Result<ChatSessionSnapshot>> {
    const transcript = structuredClone(this.chatLog.get(ws) ?? [])
    return ok({
      sessionId: 'demo',
      createdAt: transcript[0]?.createdAt ?? Date.now(),
      // The demo transcript holds every role, so it doubles as the fallback
      // history a pre-transcript session would offer.
      transcript,
      history: transcript.filter((m) => m.role === 'user' || m.role === 'assistant'),
      todos: DEMO_TODOS,
    })
  }

  async deleteChatSession(_ws: string, _sessionId?: string): Promise<Result<void>> {
    return ok(undefined)
  }

  async rewindChat(ws: string, _sessionId: string, snapshotId: string): Promise<Result<ChatSessionSnapshot>> {
    const log = this.chatLog.get(ws) ?? []
    const index = log.findIndex((message) => message.role === 'user' && message.checkpoint === snapshotId)
    if (index < 0) return err('Paired chat checkpoint not found')
    this.chatAbort.add(ws)
    const transcript = structuredClone(log.slice(0, index))
    // A real RPC returns independent decoded objects, not the storage buffer.
    this.chatLog.set(ws, structuredClone(transcript))
    return ok({ sessionId: 'demo', createdAt: transcript[0]?.createdAt ?? Date.now(), transcript,
      history: transcript.filter((message) => message.role === 'user' || message.role === 'assistant'), todos: index ? DEMO_TODOS : [] })
  }

  // Blueprints ----------------------------------------------------------------
  async listNodeKinds(): Promise<Result<NodeCatalog>> {
    // Mirrors the daemon's built-in registry so the palette filter behaves the
    // same in the demo as against a real daemon.
    return ok(fromWireCatalog(catalogFixture))
  }

  async listFunctions(): Promise<Result<FunctionItem[]>> {
    return ok([
      {
        id: 'mock-chain',
        name: 'ChainOfThought',
        description: 'Ask the LLM to reason step by step.',
        source: 'builtin',
        inputs: [],
        outputs: [{ name: 'Result', type: 'string' }],
      },
    ])
  }

  /**
   * Compiles DSL text.
   *
   * The mock accepts the canonical blueprint header and reports an error with
   * the real compiler's position suffix otherwise, so the editor's diagnostics
   * path is exercised instead of throwing.
   */
  async compileDsl(source: string): Promise<Result<Blueprint>> {
    await delay(120)
    const header = /^\s*blueprint\s+"([^"]+)"/m.exec(source)
    if (!header) return err('expected a blueprint header (line 1, column 1)')
    return ok({ name: header[1], nodes: [], edges: [] } as unknown as Blueprint)
  }

  async importFunction(): Promise<Result<{ name: string; filePath: string }>> {
    return err('Connect a daemon to import addon functions')
  }

  async decompileBlueprint(_ws: string, blueprintOrId: Blueprint | string): Promise<Result<string>> {
    // When given the live canvas, produce a compact textual skeleton so the
    // export flow stays usable without a real DSL compiler.
    const bp = typeof blueprintOrId === 'string' ? undefined : blueprintOrId
    if (bp) {
      const body = bp.nodes
        .filter((n) => n.type !== 'FileReference')
        .map((n) => `  ${n.id.slice(0, 8)}: ${n.type}`)
        .join('\n')
      return ok(`blueprint "${bp.name}"\n${body}`)
    }
    return ok(`blueprint "${blueprintOrId}"\nentry start: Start(A = 4, B = 3)`)
  }

  async saveBlueprint(_ws: string, bp: Blueprint): Promise<Result<void>> {
    await delay(150)
    DEMO_BLUEPRINT.nodes = bp.nodes
    DEMO_BLUEPRINT.edges = bp.edges
    return ok(undefined)
  }

  async loadBlueprint(_ws: string, _id: string): Promise<Result<Blueprint>> {
    await delay(180)
    return ok({
      ...DEMO_BLUEPRINT,
      nodes: DEMO_BLUEPRINT.nodes.map((n) => ({ ...n })),
      edges: DEMO_BLUEPRINT.edges.map((e) => ({ ...e })),
    })
  }

  // Execution ----------------------------------------------------------------
  async executeBlueprint(
    ws: string,
    _id: string,
    onEvent: (e: ExecutionEvent) => void,
    _blueprint?: Blueprint,
  ): Promise<Result<void>> {
    const state = this.state(ws)
    state.onEvent = onEvent
    state.status = 'running'
    state.aborted = false
    state.paused = false
    for (const step of STEPS) {
      if (state.aborted) break
      while (state.paused) {
        await delay(120)
        if (state.aborted) return ok(undefined)
      }
      onEvent({ nodeId: step.nodeId, kind: step.kind, message: step.message, detail: step.detail })
      if (step.kind === 'approval_request') {
        state.pendingApproval = true
        const id = String(step.detail?.approvalId ?? '')
        state.approvalId = id
        if (id) this.approved.delete(id)
        while (state.pendingApproval) {
          await delay(120)
          if (state.aborted) return ok(undefined)
        }
      }
      await delay(step.delay)
    }
    state.status = state.aborted ? 'cancelled' : 'finished'
    return ok(undefined)
  }

  async continueExecution(ws: string, _runId: string, onEvent: (e: ExecutionEvent) => void): Promise<Result<void>> {
    const state = this.state(ws)
    state.onEvent = onEvent
    onEvent({ nodeId: 'n-valid', kind: 'message', message: 'resumed pipeline', detail: {} })
    if (!state.aborted) state.status = 'running'
    return ok(undefined)
  }

  async cancel(ws: string): Promise<Result<void>> {
    const state = this.state(ws)
    state.aborted = true
    state.pendingApproval = false
    state.status = 'cancelled'
    return ok(undefined)
  }

  async pause(ws: string): Promise<Result<void>> {
    const state = this.state(ws)
    state.paused = true
    state.status = 'paused'
    return ok(undefined)
  }

  async resume(ws: string): Promise<Result<void>> {
    const state = this.state(ws)
    state.paused = false
    state.status = 'running'
    return ok(undefined)
  }

  // Approvals ----------------------------------------------------------------
  async respondApproval(_ws: string, requestId: string, allow: boolean): Promise<Result<void>> {
    this.approved.set(requestId, allow)
    for (const state of this.runs.values()) {
      if (state.approvalId === requestId) state.pendingApproval = false
    }
    return ok(undefined)
  }

  // Versioning ---------------------------------------------------------------
  async listSnapshots(_ws: string): Promise<Result<SnapshotInfo[]>> {
    await delay(120)
    return ok(DEMO_SNAPSHOTS)
  }

  async createSnapshot(
    _ws: string,
    description: string,
    alias?: string,
  ): Promise<Result<SnapshotInfo>> {
    await delay(120)
    const snap: SnapshotInfo = {
      id: `s-${DEMO_SNAPSHOTS.length + 1}`,
      alias,
      createdAt: Date.now(),
      message: description,
    }
    DEMO_SNAPSHOTS.unshift(snap)
    return ok(snap)
  }

  async rollback(_ws: string, _snapshotId: string, _alias?: string): Promise<Result<void>> {
    await delay(140)
    return ok(undefined)
  }

  async listExecutions(ws: string): Promise<Result<ExecutionInfo[]>> {
    const state = this.runs.get(ws)
    if (!state) return ok([])
    const status =
      state.status === 'paused' ? 'Suspended' : state.status === 'finished' ? 'Completed' : 'Running'
    return ok([{ runId: `run-${ws.length}`, blueprintId: 'bp-build-feature', status, startedAt: Date.now() - 5_000, updatedAt: Date.now(), executedNodes: 6 }])
  }

  async getExecutionTree(_ws: string, _runId: string): Promise<Result<ExecTreeData>> {
    return err('no execution tree in mock mode')
  }

  async listFileHistory(_ws: string, _path: string): Promise<Result<FileHistoryEntry[]>> {
    await delay(120)
    return ok(DEMO_HISTORY)
  }

  // Addons / usage -----------------------------------------------------------
  async listAddons(): Promise<Result<AddonInfo[]>> {
    await delay(120)
    return ok(DEMO_ADDONS)
  }

  async setAddonEnabled(id: string, enabled: boolean, _workspacePath = ''): Promise<Result<void>> {
    const addon = DEMO_ADDONS.find((a) => a.id === id)
    if (!addon) return err(`addon not found: ${id}`)
    addon.enabled = enabled
    return ok(undefined)
  }

  async listMcpServers(_workspacePath?: string): Promise<Result<McpServerInfo[]>> {
    await delay(80)
    return ok(DEMO_MCP)
  }

  async getBlackboard(_ws: string, _runId: string, _query?: BoardQuery): Promise<Result<Blackboard>> {
    return { ok: false, error: 'Blackboard requires a recorded daemon run.' }
  }

  async listOversightReports(_ws: string, _run: string): Promise<Result<OversightReports>> { return err('Supervisor reports require a daemon connection.') }
  async getConciergeState(_ws: string, _run: string, _conversation: string): Promise<Result<ConciergeState>> {
    return err('Concierge requires a live daemon and a recorded blueprint run')
  }
  async sendConciergeMessage(_ws: string, _run: string, _conversation: string, _id: string, _message: string, _onEvent: (event: ConciergeEvent) => void, _signal?: AbortSignal): Promise<Result<void>> {
    return err('Concierge requires a live daemon; no request was accepted')
  }
  async getExecutionUsage(_ws: string, _runId: string): Promise<Result<UsageSummary>> {
    await delay(150)
    return ok(DEMO_USAGE)
  }

  // Internals -----------------------------------------------------------------
  private state(ws: string): RunState {
    let s = this.runs.get(ws)
    if (!s) {
      s = {
        status: 'idle',
        aborted: false,
        paused: false,
        pendingApproval: false,
        approvalId: null,
      }
      this.runs.set(ws, s)
    }
    return s
  }
}

/** Creates a fresh demo gateway; edges demo export for the monkey/e2e suites. */
export function createGateway(): DaemonGateway {
  return new MockGateway()
}

/** Re-exported fixtures so tests share the same demo shapes. */
export const demo = {
  workspace: DEMO_WS,
  blueprint: DEMO_BLUEPRINT,
  approvals: (): ApprovalRequest[] => [
    {
      id: 'a1',
      title: 'Approve shell command',
      tool: 'Git',
      command: 'git commit -m "feat: x"',
      detail: 'Runs in the workspace sandbox. Review the command before allowing.',
    },
  ],
  snapshots: DEMO_SNAPSHOTS,
  history: DEMO_HISTORY,
  addons: DEMO_ADDONS,
  usage: DEMO_USAGE,
  edges: Object.freeze(DEMO_BLUEPRINT.edges) as readonly BlueprintEdge[],
  nodes: Object.freeze(DEMO_BLUEPRINT.nodes) as readonly BlueprintNode[],
}
