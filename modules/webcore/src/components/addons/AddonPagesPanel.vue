<script setup lang="ts">
import InlineHelp from '@/components/resources/InlineHelp.vue'
import PageHeader from '@/components/PageHeader.vue'
import { ref, computed, watch, onUnmounted } from 'vue'
import { Puzzle, ShieldCheck, LockKeyhole, PanelTop, ArrowRight, RefreshCw } from '@lucide/vue'
import { gateway } from '@/core'
import { useThemeStore } from '@/stores/theme'
import { useWorkspaceStore } from '@/stores/workspace'
const capabilityLabels: Record<string, string> = {
  theme: 'Match the application theme',
  resize: 'Adjust the page height',
  'selected-preview': 'Read the preview you select',
  'inspect-preview': 'Inspect the selected offline preview',
  'own-reports': 'Read this page’s inspection report',
}
interface PackageIdentity {
  id: string
  version: string
  fingerprint: string
  scope: string
}
interface PageEntry {
  package: PackageIdentity
  page: { id: string; title: string; capabilities: string[] }
}
interface PageInstance {
  instance_id: string
  nonce: string
  page_id: string
  package: PackageIdentity
  title: string
  html: string
  csp: string
  capabilities: string[]
  lease_seconds: number
}
defineProps<{ embedded?: boolean }>()
const theme = useThemeStore()
const workspace = useWorkspaceStore(),
  pages = ref<PageEntry[]>([]),
  selected = ref(''),
  instance = ref<PageInstance | null>(null),
  busy = ref(false),
  error = ref(''),
  details = ref(false),
  frame = ref<HTMLIFrameElement | null>(null),
  frameHeight = ref(520),
  invalid = ref(false)
interface Target {
  id: string
  title: string
  revision: number
}
interface InspectionReport {
  id: string
  digest: string
  target: Target
  human_acceptance: null | { accepted: boolean }
  scope: string
}
const targets = ref<Target[]>([]),
  targetId = ref(''),
  report = ref<InspectionReport | null>(null),
  consent = ref(false),
  reviewBusy = ref(false),
  bridgeBusy = ref(false)
interface PageContext {
  page: PageInstance
  root: string
  generation: number
}
function pageContext(): PageContext | null {
  return instance.value && instanceRoot
    ? { page: instance.value, root: instanceRoot, generation }
    : null
}
function currentPage(context: PageContext) {
  return (
    instance.value === context.page &&
    instanceRoot === context.root &&
    workspace.active?.path === context.root &&
    generation === context.generation
  )
}
function pageRequest(context: PageContext, payload: Record<string, unknown>) {
  return request(
    { ...payload, instance_id: context.page.instance_id, nonce: context.page.nonce },
    context.root,
  )
}
async function selectTarget() {
  const context = pageContext(),
    target = targetId.value
  if (!context || !target) return
  report.value = null
  consent.value = false
  try {
    await pageRequest(context, { operation: 'select_target', target_id: target })
    if (currentPage(context) && targetId.value === target)
      port?.postMessage({ type: 'target-changed' })
  } catch (e) {
    if (currentPage(context) && targetId.value === target) error.value = String(e)
  }
}
async function review(accepted: boolean) {
  const context = pageContext(),
    selectedReport = report.value
  if (!context || !selectedReport || reviewBusy.value || (accepted && !consent.value)) return
  reviewBusy.value = true
  try {
    const result = (await pageRequest(context, {
      operation: 'review',
      report_id: selectedReport.id,
      expected_digest: selectedReport.digest,
      accepted,
    })) as InspectionReport
    if (currentPage(context) && report.value === selectedReport) {
      report.value = result
      consent.value = false
    }
  } catch (e) {
    if (currentPage(context) && report.value === selectedReport) error.value = String(e)
  } finally {
    if (currentPage(context)) reviewBusy.value = false
  }
}
let instanceRoot = ''
let generation = 0,
  port: MessagePort | null = null,
  heartbeat: ReturnType<typeof setInterval> | undefined,
  loadingFrame = false
const entry = computed(
  () =>
    pages.value.find((p) => `${p.package.id}/${p.page.id}` === selected.value) || pages.value[0],
)
const source = computed(() =>
  instance.value
    ? `<!doctype html><meta http-equiv="Content-Security-Policy" content="${instance.value.csp}"><meta name="referrer" content="no-referrer">${instance.value.html}`
    : '',
)
async function request(payload: unknown, root = workspace.active?.path) {
  if (!root) throw new Error('Open a workspace first.')
  const response = await gateway.resourceRequest('addonPages', root, payload)
  if (!response.ok) throw new Error(response.error)
  return response.data
}
function disconnect() {
  port?.close()
  port = null
  if (heartbeat) clearInterval(heartbeat)
  heartbeat = undefined
  loadingFrame = false
}
function selectPage(item: PageEntry) {
  selected.value = `${item.package.id}/${item.page.id}`
  void close()
}
async function close(invalidate = true) {
  if (invalidate) {
    generation++
    busy.value = false
  }
  reviewBusy.value = bridgeBusy.value = false
  const previous = instance.value,
    root = instanceRoot
  instanceRoot = ''
  disconnect()
  instance.value = null
  report.value = null
  consent.value = false
  targetId.value = ''
  if (previous && root) {
    try {
      await request(
        { operation: 'close', instance_id: previous.instance_id, nonce: previous.nonce },
        root,
      )
    } catch {
      /*Already revoked or disconnected.*/
    }
  }
}
async function load() {
  const ticket = ++generation,
    root = workspace.active?.path
  busy.value = true
  error.value = ''
  await close(false)
  if (ticket !== generation || workspace.active?.path !== root) return
  if (!root) {
    pages.value = []
    targets.value = []
    busy.value = false
    return
  }
  try {
    const data = (await request({ operation: 'list' }, root)) as { pages: PageEntry[] }
    if (ticket === generation) {
      pages.value = data.pages
      invalid.value = false
      const available = (await request({ operation: 'targets' }, root)) as { targets: Target[] }
      if (ticket === generation) targets.value = available.targets
    }
  } catch (e) {
    if (ticket === generation) error.value = String(e)
  } finally {
    if (ticket === generation) busy.value = false
  }
}
async function open() {
  const selectedEntry = entry.value,
    root = workspace.active?.path
  if (!selectedEntry || !root || busy.value) return
  const ticket = ++generation
  busy.value = true
  error.value = ''
  await close(false)
  if (ticket !== generation || workspace.active?.path !== root) return
  try {
    const next = (await request(
      {
        operation: 'open',
        package_id: selectedEntry.package.id,
        page_id: selectedEntry.page.id,
        expected_digest: selectedEntry.package.fingerprint,
      },
      root,
    )) as PageInstance
    if (ticket !== generation || workspace.active?.path !== root) {
      // Release an admitted lease even when its response lost the UI race.
      await request(
        { operation: 'close', instance_id: next.instance_id, nonce: next.nonce },
        root,
      ).catch(() => {})
      return
    }
    instanceRoot = root
    instance.value = next
    invalid.value = false
    loadingFrame = true
  } catch (e) {
    if (ticket === generation) error.value = String(e)
  } finally {
    if (ticket === generation) busy.value = false
  }
}
function revoke(reason: string) {
  void close()
  invalid.value = true
  error.value = reason
}
async function bridge(message: unknown) {
  const context = pageContext()
  if (!context) throw new Error('Page access ended.')
  return pageRequest(context, { operation: 'message', message })
}
function connected() {
  if (!loadingFrame || !instance.value) {
    if (instance.value) revoke('The addon navigated away. Open a fresh instance.')
    return
  }
  loadingFrame = false
  const current = instance.value,
    context = pageContext()!,
    channel = new MessageChannel()
  port = channel.port1
  const session = port
  let inflight = 0
  session.onmessage = async (event) => {
    if (port !== session || instance.value !== current) return
    const message = event.data
    let size = Infinity
    try {
      size = JSON.stringify(message).length
    } catch {
      /*Reject cyclic or unserializable data.*/
    }
    if (!message || typeof message !== 'object' || size > 16384 || inflight >= 4) {
      revoke('Invalid or excessive addon messages.')
      return
    }
    inflight++
    if (message.method === 'inspect_selected') bridgeBusy.value = true
    try {
      const response = await bridge(message)
      if (port !== session || instance.value !== current) return
      if (
        message.method === 'resize' &&
        typeof (response as { result?: { height?: number } }).result?.height === 'number'
      )
        frameHeight.value = (response as { result: { height: number } }).result.height
      if (message.method === 'inspect_selected') {
        report.value = (response as { result: InspectionReport }).result
        consent.value = false
      }
      session.postMessage(response)
    } catch (e) {
      if (port === session) {
        session.postMessage({ bridge_version: 1, request_id: message.request_id, error: String(e) })
        revoke('Page access ended or the bridge rejected this request.')
      }
    } finally {
      inflight--
      if (port === session && message.method === 'inspect_selected') bridgeBusy.value = false
    }
  }
  session.start()
  // Opaque iframe origin: bind the dedicated port to this exact contentWindow.
  // Never pass the RPC client, nonce, workspace path or user identity to it.
  session.postMessage({
    type: 'theme',
    mode: document.documentElement.classList.contains('dark') ? 'dark' : 'light',
  })
  frame.value?.contentWindow?.postMessage(
    { type: 'metteur-addon-connect', bridge_version: 1 },
    '*',
    [channel.port2],
  )
  heartbeat = setInterval(async () => {
    if (instance.value !== current) return
    try {
      await pageRequest(context, { operation: 'heartbeat' })
    } catch {
      if (currentPage(context))
        revoke('The addon was disabled, changed, disconnected or its lease expired.')
    }
  }, 2000)
}
watch(
  () => theme.mode,
  (mode) => port?.postMessage({ type: 'theme', mode }),
)
watch(
  () => workspace.active?.path,
  () => {
    void load()
  },
  { immediate: true },
)
onUnmounted(() => {
  generation++
  void close()
})
</script>
<template>
  <section class="addon-panel">
    <PageHeader v-if="!embedded" title="Addon Pages"
      ><template #help
        ><InlineHelp
          text="Signed workspace extensions. Page access shows each page’s permissions." /></template
      ><button class="btn btn-outline" :disabled="busy" @click="load">
        <RefreshCw :size="15" />Refresh
      </button></PageHeader
    >
    <div v-else class="surface-actions mb-4">
      <button class="btn btn-outline" :disabled="busy" @click="load">
        <RefreshCw :size="15" />Refresh
      </button>
    </div>
    <div v-if="busy && !instance" class="empty" role="status">Loading addon pages…</div>
    <div v-else-if="!pages.length && !error" class="empty">
      <Puzzle :size="36" />
      <h2>No Addon Pages Installed</h2>
      <p>Install a signed addon that declares a page to open it here.</p>
    </div>
    <div v-else class="shell">
      <aside>
        <button
          v-for="item in pages"
          :key="`${item.package.id}/${item.page.id}`"
          :class="{ active: entry === item }"
          @click="selectPage(item)"
        >
          <Puzzle :size="17" /><span>{{ item.page.title }}</span>
        </button>
      </aside>
      <div class="content">
        <div class="bar">
          <div>
            <Puzzle :size="17" /><strong>{{ entry?.page.title || 'Addon page' }}</strong
            ><span>{{ entry?.package.version }}</span>
          </div>
          <button class="btn btn-outline" @click="details = !details" :aria-expanded="details">
            <ShieldCheck :size="15" />Page access
          </button>
        </div>
        <div v-if="details" class="access">
          <strong>Bound to this package and workspace</strong>
          <p>{{ entry?.package.id }} · {{ entry?.package.fingerprint.slice(0, 12) }}…</p>
          <ul>
            <li v-for="cap in entry?.page.capabilities" :key="cap">
              {{ capabilityLabels[cap] || cap }}
            </li>
          </ul>
          <p>No shell, personal library or permission decisions.</p>
        </div>
        <div v-if="error" class="empty" role="alert">
          <LockKeyhole :size="32" />
          <h2>{{ invalid ? 'This Page’s Access Has Ended' : 'Unable to Open Page' }}</h2>
          <p>{{ error }}</p>
          <button class="btn btn-outline" :disabled="busy" @click="load">
            Refresh available pages
          </button>
        </div>
        <div v-else-if="instance" class="live-page">
          <div class="target-selector">
            <label for="addon-target">Preview target</label
            ><select
              id="addon-target"
              :disabled="bridgeBusy || reviewBusy"
              v-model="targetId"
              class="input"
              @change="selectTarget"
            >
              <option value="" disabled>Choose a registered preview</option>
              <option v-for="target in targets" :key="target.id" :value="target.id">
                {{ target.title }} · revision {{ target.revision }}
              </option>
            </select>
            <p v-if="!targets.length">No registered local preview targets.</p>
          </div>
          <iframe
            ref="frame"
            :srcdoc="source"
            :title="instance.title"
            sandbox="allow-scripts"
            referrerpolicy="no-referrer"
            :style="{ height: `${frameHeight}px` }"
            @load="connected"
          />
          <div v-if="report" class="human-review">
            <div class="surface-heading">
              <h2>Your Review</h2>
              <InlineHelp
                text="Acceptance records this revision’s result. It grants no permissions."
              />
            </div>
            <p v-if="report.human_acceptance" role="status">
              {{
                report.human_acceptance.accepted
                  ? 'Accepted for this revision'
                  : 'Changes requested for this revision'
              }}
            </p>
            <template v-else
              ><label
                ><input v-model="consent" type="checkbox" />I reviewed this revision and its
                evidence</label
              >
              <div>
                <button class="btn btn-outline" :disabled="reviewBusy" @click="review(false)">
                  Needs changes</button
                ><button
                  class="btn btn-primary"
                  :disabled="reviewBusy || !consent"
                  @click="review(true)"
                >
                  Accept this revision
                </button>
              </div></template
            >
          </div>
        </div>
        <div v-else class="intro">
          <div class="symbol"><PanelTop :size="34" /></div>
          <h2>{{ entry?.page.title }}</h2>
          <button class="btn btn-primary" :disabled="busy || !entry" @click="open">
            Open addon page <ArrowRight :size="15" />
          </button>
        </div>
      </div>
    </div>
  </section>
</template>
<style scoped>
.addon-panel {
  width: 100%;
  min-width: 0;
}
.eyebrow {
  font-size: 10px;
  letter-spacing: 0.13em;
  color: var(--muted-foreground);
}
h2 {
  font-size: 21px;
  font-weight: 600;
}
.shell {
  display: grid;
  grid-template-columns: 220px minmax(0, 1fr);
  border: 1px solid var(--border);
  border-radius: 14px;
  overflow: hidden;
  min-height: 560px;
  background: var(--surface);
}
aside {
  display: flex;
  flex-direction: column;
  gap: 18px;
  padding: 24px 16px;
  background: var(--sidebar);
  border-right: 1px solid var(--border);
}
aside button {
  display: flex;
  align-items: center;
  gap: 10px;
  text-align: left;
  font-size: 12px;
  padding: 12px;
  border: 1px solid transparent;
  border-radius: 8px;
}
aside button.active {
  border-color: var(--border);
  background: var(--surface);
}
.isolated {
  margin-top: auto;
  display: flex;
  gap: 8px;
  font-size: 11px;
  color: var(--muted-foreground);
}
.content {
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.bar {
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 15px 20px;
  border-bottom: 1px solid var(--border);
}
.bar > div {
  display: flex;
  gap: 9px;
  align-items: center;
  font-size: 13px;
}
.bar span {
  font-size: 11px;
  color: var(--muted-foreground);
}
.intro,
.empty {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 20px;
  text-align: center;
  padding: 55px 24px;
}
.intro p,
.empty p {
  font-size: 13px;
  line-height: 1.7;
  color: var(--muted-foreground);
  max-width: 440px;
  overflow-wrap: anywhere;
}
.symbol {
  width: 76px;
  height: 76px;
  display: grid;
  place-items: center;
  border: 1px solid var(--border);
  border-radius: 22px;
  background: var(--primary-soft);
}
.access {
  padding: 24px;
  font-size: 12px;
  line-height: 1.8;
  border-bottom: 1px solid var(--border);
}
.access p {
  color: var(--muted-foreground);
  overflow-wrap: anywhere;
}
.access ul {
  padding: 15px 20px;
  list-style: disc;
}
.target-selector {
  padding: 22px 30px 0;
  display: grid;
  gap: 9px;
  font-size: 12px;
}
.target-selector p {
  color: var(--muted-foreground);
}
.human-review {
  padding: 24px 30px;
  border-top: 1px solid var(--border);
}
.human-review h2 {
  font-size: 14px;
}
.human-review p {
  font-size: 11px;
  color: var(--muted-foreground);
  margin: 10px 0 18px;
}
.human-review label {
  display: flex;
  gap: 9px;
  font-size: 12px;
}
.human-review > div {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 16px;
}
.live-page {
  min-width: 0;
}
iframe {
  width: 100%;
  border: 0;
  background: white;
  min-height: 240px;
}
footer {
  margin-top: auto;
  display: flex;
  justify-content: center;
  align-items: center;
  gap: 7px;
  padding: 12px;
  font-size: 10px;
  color: var(--muted-foreground);
  border-top: 1px solid var(--border);
}
@media (max-width: 760px) {
  .addon-panel {
    width: 100%;
    min-width: 0;
  }
  .shell {
    grid-template-columns: 1fr;
  }
  aside {
    border-right: 0;
    border-bottom: 1px solid var(--border);
    padding: 14px;
  }
  .isolated {
    display: none;
  }
  .bar {
    flex-wrap: wrap;
  }
}
button:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}
</style>
