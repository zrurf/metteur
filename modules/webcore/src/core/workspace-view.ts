import { RecoveryCopy, type RecoveryRecord } from './recovery.ts'
import type { DaemonGateway } from './gateway'

export const VIEW_SURFACES = [
  'chat',
  'version',
  'settings',
  'execution',
  'memory',
  'prompts',
  'tasks',
] as const
export type ViewSurface = (typeof VIEW_SURFACES)[number]
export type ViewTab = { file: string } | { surface: ViewSurface }
export type DocumentPosition =
  | { kind: 'text'; selections: { anchor: number; head: number }[]; top: number; left: number }
  | { kind: 'canvas'; x: number; y: number; zoom: number }
export interface WorkspaceView {
  schema: 1
  tabs: ViewTab[]
  active: string
  split: string | null
  ratio: number
  session: string | null
  positions: Record<string, DocumentPosition>
}
export const emptyWorkspaceView = (): WorkspaceView => ({
  schema: 1,
  tabs: [],
  active: 'home',
  split: null,
  ratio: 0.5,
  session: null,
  positions: Object.create(null),
})
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
export const isSessionId = (value: unknown): value is string =>
  typeof value === 'string' && uuid.test(value)
export function viewPath(value: unknown): value is string {
  if (
    typeof value !== 'string' ||
    !value ||
    value.length > 2048 ||
    [...value].some((char) => char.charCodeAt(0) < 32 || char === '\\')
  )
    return false
  if (value === 'metteur://config/user.toml' || value === 'metteur://config/workspace.toml')
    return true
  if (/^@prompt\/(personal|project)\/[0-9a-f-]{36}\.md$/i.test(value)) return true
  return (
    !value.startsWith('/') &&
    !value.includes(':') &&
    value.split('/').every((part) => part && part !== '.' && part !== '..')
  )
}
const plain = (value: unknown): value is Record<string, unknown> =>
  !!value && typeof value === 'object' && !Array.isArray(value)
const finite = (value: unknown, bound: number) =>
  typeof value === 'number' && Number.isFinite(value) && Math.abs(value) <= bound
export function parsePosition(value: unknown): DocumentPosition | null {
  if (!plain(value)) return null
  if (
    value.kind === 'canvas' &&
    finite(value.x, 1e8) &&
    finite(value.y, 1e8) &&
    typeof value.zoom === 'number' &&
    value.zoom >= 0.25 &&
    value.zoom <= 2
  )
    return { kind: 'canvas', x: value.x as number, y: value.y as number, zoom: value.zoom }
  if (
    value.kind !== 'text' ||
    !Array.isArray(value.selections) ||
    !value.selections.length ||
    value.selections.length > 32 ||
    !finite(value.top, 1e9) ||
    !finite(value.left, 1e9) ||
    (value.top as number) < 0 ||
    (value.left as number) < 0
  )
    return null
  const selections: { anchor: number; head: number }[] = []
  for (const item of value.selections) {
    if (
      !plain(item) ||
      !Number.isSafeInteger(item.anchor) ||
      !Number.isSafeInteger(item.head) ||
      (item.anchor as number) < 0 ||
      (item.head as number) < 0 ||
      (item.anchor as number) > 1e9 ||
      (item.head as number) > 1e9
    )
      return null
    selections.push({ anchor: item.anchor as number, head: item.head as number })
  }
  return { kind: 'text', selections, top: value.top as number, left: value.left as number }
}
/** View data is never a route, file body, model selection or authorization. */
export function parseWorkspaceView(value: unknown): WorkspaceView | null {
  if (
    !plain(value) ||
    value.schema !== 1 ||
    Object.keys(value).some(
      (key) =>
        !['schema', 'tabs', 'active', 'split', 'ratio', 'session', 'positions'].includes(key),
    ) ||
    !Array.isArray(value.tabs) ||
    value.tabs.length > 64 ||
    typeof value.active !== 'string' ||
    value.active.length > 2053 ||
    !(value.split === null || viewPath(value.split)) ||
    typeof value.ratio !== 'number' ||
    !Number.isFinite(value.ratio) ||
    value.ratio < 0.15 ||
    value.ratio > 0.85 ||
    !(value.session === null || value.session === '' || isSessionId(value.session)) ||
    !plain(value.positions) ||
    Object.keys(value.positions).length > 128
  )
    return null
  const tabs: ViewTab[] = []
  for (const tab of value.tabs) {
    if (!plain(tab) || Object.keys(tab).length !== 1) return null
    if (viewPath(tab.file)) tabs.push({ file: tab.file })
    else if (VIEW_SURFACES.includes(tab.surface as ViewSurface))
      tabs.push({ surface: tab.surface as ViewSurface })
    else return null
  }
  const positions: Record<string, DocumentPosition> = Object.create(null)
  for (const [path, raw] of Object.entries(value.positions)) {
    const position = parsePosition(raw)
    if (!viewPath(path) || !position) return null
    positions[path] = position
  }
  return {
    schema: 1,
    tabs,
    active: value.active,
    split: value.split,
    ratio: value.ratio,
    session: value.session,
    positions,
  }
}
type Transport = Pick<DaemonGateway, 'resourceRequest'>
/** A fresh copy per window incarnation; an inherited source ID is read only. */
export class WorkspaceViewCopy {
  value = emptyWorkspaceView()
  error = ''
  loaded = false
  readonly ready: Promise<void>
  readonly copy: RecoveryCopy
  private touched = new Set<keyof WorkspaceView>()
  private touchedPositions = new Set<string>()
  private alive = true
  constructor(
    transport: Transport,
    readonly workspace: string,
    windowId: string,
    source: string | null,
    private changed: () => void = () => {},
    acknowledged: (id: string) => void = () => {},
  ) {
    this.copy = new RecoveryCopy(
      transport,
      workspace,
      'project',
      'view_state',
      'workspace',
      windowId,
      () => {
        if (this.copy.state === 'backed_up') acknowledged(this.copy.input.id)
        this.changed()
      },
    )
    this.ready = this.load(transport, source)
  }
  private async load(transport: Transport, source: string | null) {
    try {
      if (source && !uuid.test(source))
        throw new Error('The saved view identity is invalid. Select your view explicitly.')
      const result = await transport.resourceRequest('recovery', this.workspace, {
        scope: 'project',
        action:
          source && uuid.test(source)
            ? { operation: 'read', id: source }
            : { operation: 'latest_view' },
      })
      if (!this.alive) return
      if (!result.ok) throw new Error(result.error)
      const record = (result.data as { record?: RecoveryRecord | null })?.record
      if (source && !record)
        throw new Error('The selected saved view is missing. Other windows were not substituted.')
      if (record) {
        const input = record.input
        const view = parseWorkspaceView(input?.view)
        if (
          record.schema_version !== 1 ||
          record.discarded ||
          input?.kind !== 'view_state' ||
          input.entity !== 'workspace' ||
          input.raw !== '' ||
          input.pending_submission !== null ||
          input.selected_model !== null ||
          input.selections.length !== 0 ||
          !view
        )
          throw new Error(
            'Saved view has an unsupported or invalid format. It was retained without applying it.',
          )
        for (const key of ['tabs', 'active', 'split', 'ratio', 'session'] as const)
          if (!this.touched.has(key)) Object.assign(this.value, { [key]: view[key] })
        this.value.positions = Object.assign(
          Object.create(null),
          view.positions,
          Object.fromEntries(
            [...this.touchedPositions].map((path) => [path, this.value.positions[path]]),
          ),
        )
      }
    } catch (error) {
      if (this.alive) this.error = String(error)
    } finally {
      if (this.alive) {
        this.loaded = true
        if (this.touched.size) this.backup()
        this.changed()
      }
    }
  }
  edit(update: Partial<Omit<WorkspaceView, 'schema' | 'positions'>>) {
    for (const [key, value] of Object.entries(update)) {
      if (JSON.stringify(this.value[key as keyof WorkspaceView]) === JSON.stringify(value)) continue
      this.touched.add(key as keyof WorkspaceView)
      Object.assign(this.value, { [key]: JSON.parse(JSON.stringify(value)) })
    }
    if (this.loaded) this.backup()
    this.changed()
  }
  position(path: string, position: DocumentPosition) {
    if (
      !viewPath(path) ||
      !parsePosition(position) ||
      JSON.stringify(this.value.positions[path]) === JSON.stringify(position)
    )
      return
    if (!(path in this.value.positions) && Object.keys(this.value.positions).length >= 128) {
      this.error = 'View position limit reached. Existing positions were kept.'
      this.changed()
      return
    }
    this.value.positions[path] = JSON.parse(JSON.stringify(position))
    this.touchedPositions.add(path)
    this.touched.add('positions')
    if (this.loaded) this.backup()
    this.changed()
  }
  private backup() {
    const view = parseWorkspaceView(this.value)
    if (!view) {
      this.error = 'View limit or format is unsupported. Current input stays in this window.'
      return
    }
    if (JSON.stringify(this.copy.input.view) !== JSON.stringify(view)) this.copy.edit({ view })
  }
  async flush() {
    await this.ready
    return this.copy.flush()
  }
  dispose() {
    this.alive = false
    this.copy.dispose()
  }
}
