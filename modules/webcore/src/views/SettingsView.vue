<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Check, FileCog, Moon, Plus, RotateCcw, Sun, Trash2, UserCog, Wifi, WifiOff } from '@lucide/vue'
import type { DaemonConfig } from '@/core'
import { useAddonStore } from '@/stores/addon'
import { useConfigStore } from '@/stores/config'
import { useThemeStore } from '@/stores/theme'
import { useSettingsStore, SETTINGS_SCOPES } from '@/stores/settings'
import { useWorkspaceStore } from '@/stores/workspace'
import { useTabsStore } from '@/stores/tabs'
import { usePanelStore } from '@/stores/panel'
import { useFeedbackStore } from '@/stores/feedback'
import { gateway } from '@/core'
import { projectName } from '@/lib/path'
import { USER_CONFIG_PATH, WORKSPACE_CONFIG_PATH } from '@/lib/toml'
import { fileRoute } from '@/lib/file-token'
import SettingsNav from '@/components/SettingsNav.vue'
import AddonFunctions from '@/components/settings/AddonFunctions.vue'
import SettingRow from '@/components/settings/SettingRow.vue'
import ConfigModal, { type ModalField } from '@/components/settings/ConfigModal.vue'
import Combobox from '@/components/settings/Combobox.vue'
import Toggle from '@/components/settings/Toggle.vue'

/**
 * Settings content pane (VSCode-style layered preferences).
 *
 * The nav rail ({@link SettingsNav}) picks a group; this view renders it. Every
 * config-backed setting binds to the *layer being edited* (User = global,
 * Workspace) and shows which layer the displayed value comes from; the
 * workspace layer overrides the user layer per key, matching the daemon's
 * `.metteur/config.toml` merging. Saving persists the active layer.
 */

interface SettingDef {
  section: string
  key: string
  label: string
  desc?: string
  type: 'text' | 'number' | 'switch' | 'select' | 'list'
  options?: string[]
  placeholder?: string
}

const addons = useAddonStore()
const config = useConfigStore()
const theme = useThemeStore()
const settings = useSettingsStore()
const workspace = useWorkspaceStore()
const panel = usePanelStore()
const feedback = useFeedbackStore()
const router = useRouter()
const tabs = useTabsStore()

/* Setting definitions for the scalar groups ----------------------------------- */

const GROUPS: Record<string, SettingDef[]> = {
  llm: [
    {
      section: 'llm',
      key: 'default_model',
      label: 'Default model',
      desc: 'Model used when a node or chat does not pick one.',
      type: 'text',
      placeholder: 'e.g. claude-sonnet-4-5',
    },
    {
      section: 'llm',
      key: 'temperature',
      label: 'Temperature',
      desc: 'Sampling temperature for LLM calls (0–2).',
      type: 'number',
      placeholder: '0.7',
    },
    {
      section: 'llm',
      key: 'subagent_default_model',
      label: 'SubAgent model',
      desc: 'Preferred model for sub-agent tool invocations.',
      type: 'text',
      placeholder: 'e.g. claude-haiku-4-5',
    },
  ],
  sandbox: [
    {
      section: 'sandbox',
      key: 'enabled',
      label: 'Sandbox enabled',
      desc: 'Run commands inside the workspace sandbox.',
      type: 'switch',
    },
    {
      section: 'sandbox',
      key: 'approval_timeout_secs',
      label: 'Approval timeout (s)',
      desc: 'Seconds before an unanswered approval request is denied (0 = default).',
      type: 'number',
    },
    {
      section: 'sandbox',
      key: 'command_timeout_secs',
      label: 'Command timeout (s)',
      desc: 'Seconds before a spawned command is killed (0 = default).',
      type: 'number',
    },
    {
      section: 'sandbox',
      key: 'whitelist',
      label: 'Whitelist',
      desc: 'Commands allowed without approval — one glob per line.',
      type: 'list',
    },
    {
      section: 'sandbox',
      key: 'blacklist',
      label: 'Blacklist',
      desc: 'Commands always requiring approval — one glob per line.',
      type: 'list',
    },
  ],
  mcp: [
    {
      section: 'mcp',
      key: 'call_timeout_secs',
      label: 'Call timeout (s)',
      desc: 'Per-call timeout for MCP tool invocations (0 = 30).',
      type: 'number',
    },
  ],
  lsp: [
    {
      section: 'lsp',
      key: 'enabled',
      label: 'LSP integration',
      desc: 'Serve diagnostics from configured language servers.',
      type: 'switch',
    },
  ],
  versioning: [
    {
      section: 'versioning',
      key: 'auto_snapshot',
      label: 'Auto snapshot',
      desc: 'Create a version snapshot after each execution finishes.',
      type: 'switch',
    },
  ],
  // Billing only drives cost *display*: currency + timezone. Peak/off-peak
  // pricing is configured per model (in the LLM & Models group) and persisted
  // under `llm.models`.
  billing: [],
  daemon: [
    {
      section: 'daemon',
      key: 'autostart',
      label: 'Autostart',
      desc: 'How the daemon registers itself with the host.',
      type: 'select',
      options: ['off', 'login', 'service'],
    },
    {
      section: 'daemon',
      key: 'wake',
      label: 'Stay quiet until WAKE',
      desc: 'Daemon sleeps until a client sends WAKE.',
      type: 'switch',
    },
    {
      section: 'daemon',
      key: 'listen_addr',
      label: 'Listen address',
      desc: 'Overrides the CLI default listen address when set.',
      type: 'text',
      placeholder: 'e.g. 127.0.0.1:8787',
    },
    {
      section: 'execution',
      key: 'circuit_break_after',
      label: 'Circuit breaker',
      desc: 'Validator/judge failures before the run trips (0 = disabled).',
      type: 'number',
    },
  ],
}

/* Layer-aware value plumbing ------------------------------------------------ */

/** A pinia setup store exposes its refs *unwrapped* on the store instance at
 *  runtime (`config.user` is the object, not a ref), while `vue-tsc` types the
 *  same property as `Ref<DaemonConfig>`. `cfgValue` accepts either shape so the
 *  aliases below always resolve to the actual layer object. */
function cfgValue(v: DaemonConfig | { value?: DaemonConfig }): DaemonConfig {
  if (v && typeof v === 'object' && 'value' in v && v.value !== undefined) {
    return (v as { value: DaemonConfig }).value
  }
  return v as DaemonConfig
}

const eff = computed<DaemonConfig>(() => cfgValue(config.effective))
const userCfg = computed<DaemonConfig>(() => cfgValue(config.user))
const wsCfg = computed<DaemonConfig>(() => cfgValue(config.ws))

const edition = computed<DaemonConfig>(() =>
  settings.layer === 'workspace' ? wsCfg.value : userCfg.value,
)

function sectionOf(cfg: DaemonConfig, name: string): Record<string, unknown> {
  const sec = cfg[name]
  return sec && typeof sec === 'object' && !Array.isArray(sec) ? (sec as Record<string, unknown>) : {}
}

/** Value shown for a setting: the layer being edited, falling back up. */
function liveValue(def: SettingDef): unknown {
  return sectionOf(eff.value, def.section)[def.key] ?? undefined
}

/** Which layer the displayed value ultimately comes from. */
function sourceOf(def: SettingDef): 'User' | 'Workspace' | 'Inherit' | 'Default' {
  const wsSec = sectionOf(wsCfg.value, def.section)
  const userSec = sectionOf(userCfg.value, def.section)
  const override = wsSec[def.key]
  const provided = override !== undefined && override !== null &&
    (!Array.isArray(override) || override.length > 0)
  if (provided && (def.section !== 'addon' || def.key === 'call_timeout_ms')) return 'Workspace'
  if (def.key in userSec) return settings.layer === 'workspace' ? 'Inherit' : 'User'
  return 'Default'
}

function setValue(def: SettingDef, v: unknown) {
  if (!(def.section in edition.value)) edition.value[def.section] = {}
  sectionOf(edition.value, def.section)[def.key] = v
}

function resetValue(def: SettingDef) {
  config.resetKey(settings.layer, def.section, def.key)
}

function resettable(def: SettingDef): boolean {
  return def.key in sectionOf(edition.value, def.section)
}

/* Complex groups: model pricing, MCP servers, LSP languages ------------------- */

/* Model modal (Add/Edit) ------------------------------------------------------ */

const MODEL_FIELDS: ModalField[] = [
  { key: 'name', label: 'Model ID', type: 'text', required: true, placeholder: 'e.g. deepseek-v4-pro-202606' },
  { key: 'display_name', label: 'Display name', type: 'text', placeholder: 'e.g. DeepSeek V4 Pro' },
  { key: 'api_type', label: 'API type', type: 'select', required: true, options: ['openai-chat', 'openai-responses', 'anthropic'] },
  { key: 'api_endpoint', label: 'API endpoint', type: 'text', required: true, placeholder: 'https://api.example.com/v1' },
  { key: 'model_id', label: 'Model ID (sent in requests)', type: 'text', required: true, placeholder: 'e.g. deepseek-v4-pro-202606' },
  { key: 'api_key', label: 'API key', type: 'password', required: true, placeholder: 'sk-…' },
  {
    key: 'advanced',
    label: 'Advanced',
    type: 'group',
    children: [
      { key: 'context_window_input', label: 'Context window (input)', type: 'number', placeholder: 'e.g. 200000' },
      { key: 'context_window_output', label: 'Context window (output)', type: 'number', placeholder: 'e.g. 64000' },
      { key: 'supports_vision', label: 'Supports vision', type: 'switch' },
    ],
  },
  {
    key: 'pricing',
    label: 'Pricing',
    type: 'group',
    children: [
      { key: 'pricing_kind', label: 'Kind', type: 'select', options: ['default', 'tiered', 'peak'] },
      { key: 'prices', label: 'Default prices', type: 'prices', showIf: (v) => String(v.pricing_kind ?? 'default') === 'default' },
      { key: 'tiers', label: 'Tiers', type: 'tiers', showIf: (v) => String(v.pricing_kind) === 'tiered' },
      { key: 'default_prices', label: 'Off-peak prices', type: 'prices', showIf: (v) => String(v.pricing_kind) === 'peak' },
      { key: 'windows', label: 'Peak / off-peak windows', type: 'windows', showIf: (v) => String(v.pricing_kind) === 'peak' },
    ],
  },
]

const modelModal = ref(false)
/** Name of the model being edited; empty = add new. */
const modelEditName = ref('')

function llmModelsOf(cfg: DaemonConfig): Record<string, unknown> {
  const sec = cfg.llm && typeof cfg.llm === 'object' ? (cfg.llm as Record<string, unknown>) : {}
  const m = sec.models
  return m && typeof m === 'object' && !Array.isArray(m) ? (m as Record<string, unknown>) : {}
}
/** Models of the layer being edited, falling back to inherited rows.
 *
 * The list must reflect the *edited* layer: the daemon merge replaces a
 * non-empty collection wholesale, so a row written to the User layer would be
 * invisible (appearing to vanish) whenever the workspace layer also defines
 * models. With no rows in the edited layer, inherited rows are shown so they
 * stay discoverable and can be overridden by editing. */
function llmModels(): Record<string, unknown> {
  const own = llmModelsOf(edition.value)
  return Object.keys(own).length ? own : llmModelsOf(eff.value)
}
function modelKeys(): string[] {
  return Object.keys(llmModels())
}
function modelOf(name: string): Record<string, unknown> {
  return (llmModels()[name] ?? {}) as Record<string, unknown>
}
function modelPricingOf(name: string): Record<string, unknown> {
  const p = modelOf(name).pricing
  return p && typeof p === 'object' && !Array.isArray(p) ? (p as Record<string, unknown>) : {}
}
function modelDisplay(name: string): string {
  return String(modelOf(name).display_name ?? name)
}
function modelApiType(name: string): string {
  return String(modelOf(name).api_type ?? 'openai-chat')
}
function modelPricingKind(name: string): string {
  return String(modelPricingOf(name).kind ?? 'default')
}
function modelWindowCount(name: string): number {
  const w = modelPricingOf(name).windows
  return Array.isArray(w) ? (w as unknown[]).length : 0
}
function modelDefaultPrice(name: string, field: 'input_per_mtok' | 'output_per_mtok'): number {
  const p = modelPricingOf(name)
  const src = (p.prices ?? p.default_prices) as Record<string, unknown> | undefined
  return Number(src?.[field] ?? 0)
}
function openModelModal(name?: string) {
  modelEditName.value = name ?? ''
  modelModal.value = true
}
function numOrEmpty(v: unknown): number | '' {
  return typeof v === 'number' ? v : ''
}
function modelModalInitial(): Record<string, unknown> {
  const name = modelEditName.value
  if (!name) return { pricing: { pricing_kind: 'default' } }
  const m = modelOf(name)
  const p = modelPricingOf(name)
  return {
    name,
    display_name: typeof m.display_name === 'string' ? m.display_name : '',
    api_type: String(m.api_type ?? 'openai-chat'),
    api_endpoint: typeof m.api_endpoint === 'string' ? m.api_endpoint : '',
    model_id: typeof m.model_id === 'string' ? m.model_id : '',
    api_key: typeof m.api_key === 'string' ? m.api_key : '',
    advanced: {
      context_window_input: numOrEmpty(m.context_window_input),
      context_window_output: numOrEmpty(m.context_window_output),
      supports_vision: m.supports_vision === true,
    },
    pricing: {
      pricing_kind: String(p.kind ?? 'default'),
      prices: (p.prices ?? {}) as Record<string, unknown>,
      tiers: Array.isArray(p.tiers) ? (p.tiers as Array<Record<string, unknown>>) : [],
      default_prices: (p.default_prices ?? {}) as Record<string, unknown>,
      windows: Array.isArray(p.windows) ? (p.windows as Array<Record<string, unknown>>) : [],
    },
  }
}
function numOrNull(v: unknown): number | null {
  if (v === '' || v === undefined || v === null) return null
  const n = Number(v)
  return Number.isNaN(n) ? null : n
}
/** Assemble the `pricing` strategy object from the Pricing group values. */
function pricingFromValues(values: Record<string, unknown>): Record<string, unknown> | undefined {
  const group = (values.pricing ?? {}) as Record<string, unknown>
  const kind = String(group.pricing_kind ?? 'default')
  const out: Record<string, unknown> = {}
  if (kind !== 'default') out.kind = kind
  if (kind === 'default') {
    const p = group.prices
    if (p && typeof p === 'object' && Object.keys(p as object).length) out.prices = p
  } else if (kind === 'tiered') {
    if (Array.isArray(group.tiers) && (group.tiers as unknown[]).length) out.tiers = group.tiers
  } else if (kind === 'peak') {
    const dp = group.default_prices
    if (dp && typeof dp === 'object' && Object.keys(dp as object).length) out.default_prices = dp
    if (Array.isArray(group.windows) && (group.windows as unknown[]).length) out.windows = group.windows
  }
  return Object.keys(out).length ? out : undefined
}
function submitModel(values: Record<string, unknown>) {
  const name = String(values.name ?? modelEditName.value).trim()
  if (!name) return
  // Editing an inherited row must pin the whole inherited set into this layer
  // first; otherwise a same-named row of this layer would replace it and every
  // other inherited model would vanish from the merged config. Runs before the
  // collection is captured below, because it may replace the object.
  materializeModels()
  const layer = edition.value
  if (!('llm' in layer)) layer.llm = {}
  const llm = layer.llm as Record<string, unknown>
  if (!('models' in llm)) llm.models = {}
  const models = llm.models as Record<string, unknown>
  const prev = (models[name] ?? {}) as Record<string, unknown>
  const advanced = (values.advanced ?? {}) as Record<string, unknown>
  const entry: Record<string, unknown> = {
    ...prev,
    ...(values.display_name ? { display_name: String(values.display_name) } : {}),
    api_type: String(values.api_type ?? 'openai-chat'),
    api_endpoint: String(values.api_endpoint ?? ''),
    model_id: String(values.model_id ?? ''),
    api_key: String(values.api_key ?? ''),
    context_window_input: numOrNull(advanced.context_window_input),
    context_window_output: numOrNull(advanced.context_window_output),
    supports_vision: advanced.supports_vision === true,
  }
  const pricing = pricingFromValues(values)
  if (pricing) entry.pricing = pricing
  models[name] = entry
  // Renaming a model: drop the old key in both llm and (legacy) billing.
  if (modelEditName.value && modelEditName.value !== name) {
    delete models[modelEditName.value]
    const bm = sectionOf(edition.value, 'billing').models as Record<string, unknown> | undefined
    if (bm) delete bm[modelEditName.value]
  }
  modelModal.value = false
  void persistNow()
}
function removeModel(name: string) {
  materializeModels()
  const lm = sectionOf(edition.value, 'llm').models as Record<string, unknown> | undefined
  if (lm) delete lm[name]
  // Legacy cleanup: billing.models no longer stores pricing.
  const bm = sectionOf(edition.value, 'billing').models as Record<string, unknown> | undefined
  if (bm) delete bm[name]
  void persistNow()
}

/** Pin the effective model rows into the edited layer, so edits and deletes
 *  apply to inherited rows instead of silently doing nothing. */
function materializeModels() {
  const own = llmModelsOf(edition.value)
  if (Object.keys(own).length) return
  const layer = edition.value
  if (!('llm' in layer)) layer.llm = {}
  const llm = layer.llm as Record<string, unknown>
  llm.models = { ...llmModelsOf(eff.value) }
}

const mcpModal = ref(false)
function openMcpModal() {
  mcpModal.value = true
}
const lspModal = ref(false)
function openLspModal() {
  lspModal.value = true
}
const MCP_FIELDS: ModalField[] = [
  { key: 'name', label: 'Server alias', type: 'text', required: true, placeholder: 'e.g. filesystem' },
  { key: 'transport', label: 'Transport', type: 'select', required: true, options: ['stdio', 'http'] },
  { key: 'command', label: 'Command (executable)', type: 'text', placeholder: 'e.g. npx' },
  { key: 'args', label: 'Args (one per line)', type: 'textarea', placeholder: 'e.g. -y\n@modelcontextprotocol/server-filesystem' },
  { key: 'url', label: 'Server URL (http)', type: 'text', placeholder: 'https://…' },
  { key: 'enabled', label: 'Connected', type: 'switch' },
]
function makeMcp() {
  const layer = edition.value
  if (!('mcp' in layer)) layer.mcp = {}
  if (!('servers' in (layer.mcp as Record<string, unknown>))) (layer.mcp as Record<string, unknown>).servers = {}
}
/** Servers of the edited layer, falling back to inherited rows (see `llmModels`). */
function mcpServers(): Record<string, Record<string, unknown>> {
  const own = (sectionOf(edition.value, 'mcp').servers ?? {}) as Record<string, Record<string, unknown>>
  if (Object.keys(own).length) return own
  return (sectionOf(eff.value, 'mcp').servers ?? {}) as Record<string, Record<string, unknown>>
}
function mcpKeys(): string[] {
  return Object.keys(mcpServers())
}
function mcpField(name: string, field: string): unknown {
  return mcpServers()[name]?.[field]
}
function mcpCommandText(name: string): string {
  const cmd = mcpField(name, 'command')
  return Array.isArray(cmd) ? (cmd as string[]).join(' ') : ''
}
function setMcpField(name: string, field: string, v: unknown) {
  materializeMcp()
  const s = sectionOf(edition.value as DaemonConfig, 'mcp').servers as Record<string, Record<string, unknown>>
  s[name] = { ...s[name], [field]: v }
  void persistNow()
}

/** Pin the effective MCP servers into the edited layer (see `materializeModels`). */
function materializeMcp() {
  const own = (sectionOf(edition.value, 'mcp').servers ?? {}) as Record<string, unknown>
  if (Object.keys(own).length) return
  makeMcp()
  const s = sectionOf(edition.value as DaemonConfig, 'mcp').servers as Record<string, unknown>
  Object.assign(s, sectionOf(eff.value, 'mcp').servers ?? {})
}
function setMcpCommand(name: string, text: string) {
  setMcpField(name, 'command', text.split(/\s+/).filter(Boolean))
}
function submitMcp(values: Record<string, unknown>) {
  const name = String(values.name).trim()
  if (!name) return
  // Materialize inherited servers first so editing one keeps its siblings.
  materializeMcp()
  makeMcp()
  const s = sectionOf(edition.value as DaemonConfig, 'mcp').servers as Record<string, Record<string, unknown>>
  const prev = s[name] ?? {}
  // command argv = executable word(s) + whitespace-split args, merged.
  s[name] = {
    ...(prev as Record<string, unknown>),
    transport: String(values.transport ?? 'stdio'),
    command: [
      ...String(values.command ?? '').split(/\s+/).filter(Boolean),
      ...String(values.args ?? '').split(/\s+/).filter(Boolean),
    ],
    url: values.url ? String(values.url) : undefined,
    enabled: values.enabled === true,
  }
  mcpModal.value = false
  void persistNow()
}
function removeMcp(name: string) {
  materializeMcp()
  const s = sectionOf(edition.value as DaemonConfig, 'mcp').servers as Record<string, unknown> | undefined
  if (s) delete s[name]
  void persistNow()
}

const LSP_FIELDS: ModalField[] = [
  { key: 'id', label: 'Language id', type: 'text', required: true, placeholder: 'e.g. rust' },
  { key: 'extensions', label: 'Extensions', type: 'text', placeholder: 'e.g. rs, toml' },
  { key: 'command', label: 'Command (executable)', type: 'text', placeholder: 'e.g. rust-analyzer' },
  { key: 'args', label: 'Args (one per line)', type: 'textarea', placeholder: 'e.g. --stdio' },
]
function submitLsp(values: Record<string, unknown>) {
  const id = String(values.id).trim()
  if (!id) return
  // Materialize inherited languages first so adding one keeps the others.
  materializeLsp()
  makeLsp()
  const list = sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>>
  // command argv = executable word(s) + whitespace-split args, merged.
  list.push({
    id,
    extensions: String(values.extensions ?? '').split(',').map((s) => s.trim()).filter(Boolean),
    command: [
      ...String(values.command ?? '').split(/\s+/).filter(Boolean),
      ...String(values.args ?? '').split(/\s+/).filter(Boolean),
    ],
  })
  lspModal.value = false
  void persistNow()
}

function makeLsp() {
  const layer = edition.value
  if (!('lsp' in layer)) layer.lsp = {}
  if (!('languages' in (layer.lsp as Record<string, unknown>))) (layer.lsp as Record<string, unknown>).languages = []
}
/** Languages of the edited layer, falling back to inherited rows. */
function lspLanguages(): Array<Record<string, unknown>> {
  const own = (sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>>) ?? []
  if (own.length) return own
  return (sectionOf(eff.value, 'lsp').languages as Array<Record<string, unknown>>) ?? []
}
function lspList(): Array<Record<string, unknown>> {
  return lspLanguages()
}
function lspField(idx: number, field: string): unknown {
  return lspLanguages()[idx]?.[field]
}
function lspCommandText(idx: number): string {
  const cmd = lspField(idx, 'command')
  return Array.isArray(cmd) ? (cmd as string[]).join(' ') : ''
}
function setLspField(idx: number, field: string, v: unknown) {
  materializeLsp()
  const list = sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>>
  list[idx] = { ...(list[idx] ?? {}), [field]: v }
  void persistNow()
}

/** Pin the effective LSP languages into the edited layer (see `materializeModels`). */
function materializeLsp() {
  const own = (sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>>) ?? []
  if (own.length) return
  makeLsp()
  const list = sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>>
  list.push(...(((sectionOf(eff.value, 'lsp').languages as Array<Record<string, unknown>>) ?? [])))
}
function setLspCommand(idx: number, text: string) {
  setLspField(idx, 'command', text.split(/\s+/).filter(Boolean))
}
function setLspExts(idx: number, text: string) {
  setLspField(idx, 'extensions', text.split(',').map((s) => s.trim()).filter(Boolean))
}
function removeLsp(idx: number) {
  materializeLsp()
  const list = sectionOf(edition.value, 'lsp').languages as Array<Record<string, unknown>> | undefined
  list?.splice(idx, 1)
  void persistNow()
}

/* Open config.toml in the main editor ---------------------------------------- */

function openConfigDoc(scope: 'user' | 'workspace') {
  const path = scope === 'user' ? USER_CONFIG_PATH : WORKSPACE_CONFIG_PATH
  tabs.openConfigDoc(scope)
  router.push(fileRoute(path))
}

/* Billing display options ----------------------------------------------------- */

const CURRENCIES = [
  'USD', 'CNY', 'EUR', 'GBP', 'JPY', 'HKD', 'SGD', 'TWD', 'KRW', 'INR',
  'AUD', 'CAD', 'NZD', 'CHF', 'SEK', 'NOK', 'DKK', 'PLN', 'CZK', 'HUF',
  'TRY', 'ZAR', 'BRL', 'MXN', 'RUB', 'AED', 'SAR', 'THB', 'IDR', 'MYR',
  'PHP', 'VND',
]

const TIMEZONES: string[] = (Intl as unknown as { supportedValuesOf?: (key: 'timeZone') => string[] })
  .supportedValuesOf?.('timeZone') ?? [
  'UTC', 'Asia/Shanghai', 'Asia/Hong_Kong', 'Asia/Tokyo', 'Asia/Singapore',
  'America/New_York', 'America/Los_Angeles', 'Europe/London', 'Europe/Berlin',
  'Australia/Sydney', 'Europe/Moscow',
]

function billingValue(key: 'currency' | 'timezone'): string {
  return String(liveValue({ section: 'billing', key, label: '', type: 'text' }) ?? '')
}
function setBilling(key: 'currency' | 'timezone', v: string) {
  setValue({ section: 'billing', key, label: '', type: 'text' }, v)
}

/* Persistence ---------------------------------------------------------------- */

async function saveLayer() {
  const e = await config.save(settings.layer)
  if (e) feedback.toast('error', 'Failed to save settings', e)
  else feedback.toast('success', 'Settings saved')
}

/** Persist the edited layer right away.
 *
 * Adding or removing a model / server / language is a discrete, deliberate
 * action: it must reach disk immediately instead of waiting for the header
 * Save button, or a refresh silently discards it. */
async function persistNow(): Promise<void> {
  const e = await config.save(settings.layer)
  if (e) feedback.toast('error', 'Failed to save settings', e)
}

async function reload() {
  await config.load()
  addons.refresh()
  feedback.toast('info', 'Settings reloaded from daemon')
}

onMounted(async () => {
  void addons.refresh()
  void config.load()
  panel.show('settings', SettingsNav, 'Settings')
})

const connectedLabel = computed(() => (gateway.connected.value ? 'Connected' : 'Offline'))
const modelsText = computed(() => {
  const keys = modelKeys()
  return keys.length ? keys.join(', ') : '—'
})
</script>

<template>
  <div class="h-full min-w-0 flex-1 overflow-auto">
    <!-- Sticky header: group title, layer tabs, save/reload -->
    <div class="sticky top-0 z-10 flex items-center gap-3 border-b border-divider bg-background/90 px-5 py-3 backdrop-blur">
      <div class="min-w-0">
        <h1 class="text-[15px] font-semibold capitalize">{{ settings.active }}</h1>
        <p class="mt-0.5 text-[11.5px] text-muted-foreground">
          {{ config.hasWorkspace ? `Workspace: ${projectName(workspace.active!.path)}` : 'No workspace open' }}
        </p>
      </div>
      <div class="ml-auto flex items-center gap-1">
        <div
          class="flex items-center gap-1 rounded-lg bg-input p-0.5"
          style="box-shadow: inset 0 0 0 1px var(--border)"
        >
          <button
            v-for="s in SETTINGS_SCOPES"
            :key="s.key"
            class="rounded-md px-2.5 py-1 text-[12px] transition-colors duration-150"
            :class="settings.layer === s.key ? 'bg-surface text-foreground shadow-card' : 'text-muted-foreground hover:text-foreground'"
            type="button"
            :disabled="s.key === 'workspace' && !config.hasWorkspace"
            :title="s.key === 'workspace' && !config.hasWorkspace ? 'Open a workspace to edit its layer' : undefined"
            @click="settings.layer = s.key"
          >
            {{ s.label }}
          </button>
        </div>
        <button
          class="btn-icon h-7! w-7!"
          type="button"
          title="Open the user config.toml in the editor"
          :aria-label="'Open user config.toml'"
          @click="openConfigDoc('user')"
        >
          <UserCog class="h-4 w-4" />
        </button>
        <button
          class="btn-icon h-7! w-7!"
          type="button"
          :title="config.hasWorkspace ? 'Open the workspace config.toml in the editor' : 'Open a workspace first'"
          :aria-label="'Open workspace config.toml'"
          :disabled="!config.hasWorkspace"
          @click="openConfigDoc('workspace')"
        >
          <FileCog class="h-4 w-4" />
        </button>
        <button class="btn btn-outline h-7! px-2.5! text-[12px]!" type="button" :disabled="config.saving" @click="reload">
          <RotateCcw class="h-3.5 w-3.5" />
          Reload
        </button>
        <button class="btn btn-primary h-7! px-2.5! text-[12px]!" type="button" :disabled="config.saving" @click="saveLayer">
          <Check class="h-3.5 w-3.5" />
          Save
        </button>
      </div>
    </div>
    <p class="mx-5 mt-2 text-[12px] text-muted-foreground">
      LSP reloads for subsequent operations; language servers connect on first use and report connection errors there. Model and permission defaults apply to the next request or turn. Process/addon settings need a daemon restart; automatic snapshots need a workspace reopen. Save reports any pending application or connection failure.
    </p>
    <p v-if="settings.layer === 'user' ? config.legacyUser : config.legacyWorkspace" class="mx-5 mt-2 text-[12px]" role="status">
      Legacy configuration: default values previously meant inherit. Saving this form explicitly migrates this layer to presence-based overrides while preserving its effective values. Afterwards false and 0 are explicit; Reset restores inheritance. The TOML editor keeps the original format unless you set config_version = 2.
    </p>
    <p v-if="config.lastError" class="mx-5 mt-2 rounded-md px-3 py-2 text-[12px]" style="background: var(--danger-soft); color: var(--danger)" role="alert">
      {{ config.lastError }}
    </p>

    <!-- General: theme, daemon connection, workspace -->
    <section v-if="settings.active === 'general'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <p class="text-[13px] font-medium">Appearance</p>
            <p class="text-[12px] text-muted-foreground">Switch between light and dark mode.</p>
          </div>
          <div class="flex shrink-0 items-center gap-1 rounded-lg bg-input p-0.5" style="box-shadow: inset 0 0 0 1px var(--border)">
            <button
              class="flex items-center gap-1 rounded-md px-2.5 py-1 text-[12px] transition-colors duration-150"
              :class="theme.mode === 'light' ? 'bg-surface text-foreground shadow-card' : 'text-muted-foreground hover:text-foreground'"
              type="button"
              @click="theme.mode = 'light'"
            >
              <Sun class="h-3.5 w-3.5" /> Light
            </button>
            <button
              class="flex items-center gap-1 rounded-md px-2.5 py-1 text-[12px] transition-colors duration-150"
              :class="theme.mode === 'dark' ? 'bg-surface text-foreground shadow-card' : 'text-muted-foreground hover:text-foreground'"
              type="button"
              @click="theme.mode = 'dark'"
            >
              <Moon class="h-3.5 w-3.5" /> Dark
            </button>
          </div>
        </div>

        <SettingRow label="Daemon connection" :description="connectedLabel" source="Default" :resettable="false">
          <span
            class="flex h-8 w-8 items-center justify-center rounded-lg"
            :class="gateway.connected.value ? 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400' : 'bg-surface-muted text-muted-foreground'"
          >
            <Wifi v-if="gateway.connected.value" class="h-4 w-4" />
            <WifiOff v-else class="h-4 w-4" />
          </span>
        </SettingRow>

        <SettingRow label="Current workspace" :description="workspace.active ? `${workspace.active.path} (locked: ${workspace.active.locked})` : 'None — open a folder from the title bar.'" source="Default" :resettable="false" />
      </div>
    </section>

    <!-- LLM -->
    <section v-else-if="settings.active === 'llm'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.llm"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <input
            v-if="def.type === 'text'"
            class="input h-7! w-44! text-[12px]!"
            :placeholder="def.placeholder"
            :value="liveValue(def) as string | undefined ?? ''"
            @change="setValue(def, ($event.target as HTMLInputElement).value)"
          />
          <input
            v-else
            class="input h-7! w-24! text-[12px]!"
            type="number"
            step="any"
            :placeholder="def.placeholder"
            :value="liveValue(def) as number | undefined ?? ''"
            @change="setValue(def, Number(($event.target as HTMLInputElement).value))"
          />
        </SettingRow>
      </div>

      <!-- Model definitions (llm.models) -->
      <div class="panel divide-y divide-divider">
        <div class="flex items-center gap-3 px-4 py-2.5">
          <div class="min-w-0 flex-1">
            <p class="text-[13px] font-medium">Models</p>
            <p class="text-[12px] text-muted-foreground">Full model definitions (connection, advanced options, pricing); configured models appear in the Chat picker.</p>
          </div>
          <span class="chip">{{ modelsText }}</span>
        </div>
        <div v-for="name in modelKeys()" :key="name" class="flex items-center gap-2 px-4 py-2">
          <span class="min-w-0 flex-1 truncate text-[13px] font-medium">{{ modelDisplay(name) }}</span>
          <span class="text-[11.5px] text-subtle">{{ modelApiType(name) }}</span>
          <span class="text-[11.5px] text-subtle">
            ${{ modelDefaultPrice(name, 'input_per_mtok') }} in / ${{ modelDefaultPrice(name, 'output_per_mtok') }} out
          </span>
          <span class="chip">{{ modelPricingKind(name) }}</span>
          <span class="chip">{{ modelWindowCount(name) }} window(s)</span>
          <button class="btn btn-outline h-7! px-2! text-[12px]!" type="button" @click="openModelModal(name)">
            Edit
          </button>
          <button class="btn btn-danger-outline h-7! px-2! text-[12px]!" type="button" @click="removeModel(name)">
            Delete
          </button>
        </div>
        <div class="flex items-center gap-2 px-4 py-2.5">
          <button class="btn btn-primary" type="button" @click="openModelModal()">
            <Plus class="h-4 w-4" /> Add model
          </button>
        </div>
        <p v-if="!modelKeys().length" class="px-4 py-3 text-center text-[12px] text-muted-foreground">
          No models configured — connection and pricing fall back to the daemon defaults.
        </p>
      </div>
    </section>

    <!-- Sandbox -->
    <section v-else-if="settings.active === 'sandbox'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.sandbox"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <Toggle
            v-if="def.type === 'switch'"
            :model-value="!!liveValue(def)"
            @update:model-value="setValue(def, $event)"
          />
          <textarea
            v-else-if="def.type === 'list'"
            class="input w-52! resize-none text-[11.5px]! leading-relaxed!"
            rows="3"
            :value="((liveValue(def) as string[] | undefined) ?? []).join('\n')"
            @change="setValue(def, ($event.target as HTMLTextAreaElement).value.split('\n').map((s) => s.trim()).filter(Boolean))"
          />
          <input
            v-else
            class="input h-7! w-24! text-[12px]!"
            type="number"
            :value="liveValue(def) as number | undefined ?? ''"
            @change="setValue(def, Number(($event.target as HTMLInputElement).value))"
          />
        </SettingRow>
      </div>
    </section>

    <!-- MCP -->
    <section v-else-if="settings.active === 'mcp'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider" data-testid="mcp-status">
        <div class="flex items-center justify-between px-4 py-3">
          <h3 class="text-[13px] font-medium">Live connections</h3>
          <button class="btn btn-outline" type="button" @click="addons.refresh()">Refresh status</button>
        </div>
        <div v-for="server in addons.mcpServers" :key="`${server.scopeRoot}:${server.owner}:${server.name}`" class="space-y-1 px-4 py-3">
          <p class="text-[13px] font-medium">{{ server.name }} · {{ server.status }}</p>
          <p class="break-all text-[12px] text-muted-foreground">{{ server.owner ? `Addon ${server.owner}` : 'User configuration' }} · {{ server.scopeRoot || 'Global' }} · {{ server.toolCount }} tools</p>
          <p v-if="server.error" role="alert" class="text-[12px] text-destructive">{{ server.error }}</p>
        </div>
        <p v-if="addons.mcpError" role="alert" class="px-4 py-3 text-[12px] text-destructive">{{ addons.mcpError }}</p>
      </div>
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.mcp"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <input
            class="input h-7! w-24! text-[12px]!"
            type="number"
            :value="liveValue(def) as number | undefined ?? ''"
            @change="setValue(def, Number(($event.target as HTMLInputElement).value))"
          />
        </SettingRow>
        <div v-for="name in mcpKeys()" :key="name" class="flex items-center gap-2 px-4 py-2">
          <span class="min-w-0 flex-1 truncate text-[13px] font-medium">{{ name }}</span>
          <select
            class="input h-7! w-20! px-1.5! text-[11.5px]!"
            :value="mcpField(name, 'transport')"
            @change="setMcpField(name, 'transport', ($event.target as HTMLSelectElement).value)"
          >
            <option value="stdio">stdio</option>
            <option value="http">http</option>
          </select>
          <input
            class="input h-7! w-40! text-[11.5px]!"
            :value="mcpCommandText(name)"
            placeholder="command args…"
            @change="setMcpCommand(name, ($event.target as HTMLInputElement).value)"
          />
          <button
            class="btn h-7! px-2! text-[12px]!"
            :class="mcpField(name, 'enabled') ? 'btn-primary' : 'btn-outline'"
            type="button"
            @click="setMcpField(name, 'enabled', !mcpField(name, 'enabled'))"
          >
            {{ mcpField(name, 'enabled') ? 'On' : 'Off' }}
          </button>
          <button class="btn-icon h-6! w-6!" type="button" :aria-label="`Remove ${name}`" @click="removeMcp(name)">
            <Trash2 class="h-3.5 w-3.5" />
          </button>
        </div>
        <div class="flex items-center gap-2 px-4 py-2.5">
          <button class="btn btn-primary" type="button" @click="openMcpModal()">
            <Plus class="h-4 w-4" /> Add server
          </button>
        </div>
        <p v-if="!mcpKeys().length" class="px-4 py-3 text-center text-[12px] text-muted-foreground">
          No MCP servers configured.
        </p>
      </div>
    </section>

    <!-- LSP -->
    <section v-else-if="settings.active === 'lsp'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.lsp"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <Toggle
            :model-value="!!liveValue(def)"
            @update:model-value="setValue(def, $event)"
          />
        </SettingRow>

        <div v-for="(_, idx) in lspList()" :key="idx" class="flex items-center gap-2 px-4 py-2">
          <input
            class="input h-7! w-24! text-[11.5px]!"
            :value="lspField(idx, 'id')"
            placeholder="lang id"
            @change="setLspField(idx, 'id', ($event.target as HTMLInputElement).value)"
          />
          <input
            class="input h-7! w-40! text-[11.5px]!"
            :value="lspField(idx, 'extensions') as unknown as string"
            placeholder="ext, comma separated"
            @change="setLspExts(idx, ($event.target as HTMLInputElement).value)"
          />
          <input
            class="input h-7! flex-1! text-[11.5px]!"
            :value="lspCommandText(idx)"
            placeholder="server command args…"
            @change="setLspCommand(idx, ($event.target as HTMLInputElement).value)"
          />
          <button class="btn-icon h-6! w-6!" type="button" :aria-label="`Remove ${lspField(idx, 'id')}`" @click="removeLsp(idx)">
            <Trash2 class="h-3.5 w-3.5" />
          </button>
        </div>
        <div class="flex items-center gap-2 px-4 py-2.5">
          <button class="btn btn-primary" type="button" @click="openLspModal()">
            <Plus class="h-4 w-4" /> Add language server
          </button>
        </div>
        <p v-if="!lspList().length" class="px-4 py-3 text-center text-[12px] text-muted-foreground">
          No language servers configured.
        </p>
      </div>
    </section>

    <!-- Addons (live daemon data) -->
    <section v-else-if="settings.active === 'addons'" class="max-w-2xl space-y-4 p-5">
      <button class="btn btn-outline" type="button" @click="addons.refresh()">Refresh addon status</button>
      <div class="panel divide-y divide-divider">
        <div v-for="a in addons.addons" :key="`${a.scopeRoot ?? a.scope}:${a.id}`" class="flex items-center gap-3 px-4 py-3">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span class="text-[13px] font-medium">{{ a.name }}</span>
              <span class="chip">v{{ a.version }}</span>
              <span class="chip">{{ a.scope }}</span>
              <span class="chip">{{ a.toolCount }} tool{{ a.toolCount === 1 ? '' : 's' }}</span>
            </div>
            <p class="mt-0.5 text-[12px] text-muted-foreground">{{ a.description || 'No description' }}</p>
            <p class="break-all text-[12px] text-muted-foreground">{{ a.scopeRoot || 'Global' }} · {{ a.status || 'Status unavailable' }}</p>
            <p v-if="a.error" class="break-words text-[12px] text-destructive">{{ a.error }}</p>
            <p class="text-[12px] text-muted-foreground">Granted: {{ a.grantedPermissions?.join(', ') || 'None' }}</p>
            <div v-if="a.hooks?.length" class="mt-2 space-y-2 border-t border-divider pt-2" data-testid="addon-hook-status">
              <p class="text-[12px] font-medium">Lifecycle observers</p>
              <div v-for="hook in a.hooks" :key="`${hook.scopeRoot}:${hook.name}`" class="break-words text-[12px]">
                <p>{{ hook.name }} · {{ hook.event }} · {{ hook.status }}</p>
                <p class="text-muted-foreground">{{ hook.completed }} completed · {{ hook.failed }} failed · {{ hook.scopeRoot }}</p>
                <p v-if="hook.eventId" class="break-all text-muted-foreground">Event {{ hook.eventId }}</p>
                <p v-if="hook.error" class="text-destructive">{{ hook.error }}</p>
              </div>
              <p class="text-[11px] text-muted-foreground">Read-only notifications. Delivery is bounded and is not replayed after restart.</p>
            </div>
          </div>
          <button
            class="btn"
            :class="a.enabled ? 'btn-primary' : 'btn-outline'"
            type="button"
            @click="addons.setEnabled(a, !a.enabled)"
          >
            <Check v-if="a.enabled" class="h-4 w-4" />
            {{ a.enabled ? 'Enabled' : 'Disabled' }}
          </button>
        </div>
        <p v-if="addons.addons.length === 0" class="px-4 py-6 text-center text-[12.5px] text-muted-foreground">
          No addons installed.
        </p>
      </div>
      <p v-if="addons.error" role="alert" class="text-[12px] text-destructive">{{ addons.error }}</p>
      <AddonFunctions :workspace-path="workspace.active?.path ?? ''" />
    </section>

    <!-- Versioning -->
    <section v-else-if="settings.active === 'versioning'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.versioning"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <Toggle
            :model-value="!!liveValue(def)"
            @update:model-value="setValue(def, $event)"
          />
        </SettingRow>
      </div>
    </section>

    <!-- Billing (cost display only) -->
    <section v-else-if="settings.active === 'billing'" class="max-w-2xl space-y-4 p-5">
      <div>
        <h1 class="text-[15px] font-semibold">Billing</h1>
        <p class="mt-0.5 text-[12px] text-muted-foreground">
          Cost display configuration. Per-model pricing and peak/off-peak rules
          are configured per model in <span class="font-medium">LLM &amp; Models</span>.
        </p>
      </div>
      <div class="panel divide-y divide-divider">
        <SettingRow label="Currency" description="Currency code used for cost reporting." source="Default" :resettable="false">
          <Combobox :model-value="billingValue('currency')" :options="CURRENCIES" placeholder="USD" @update:model-value="setBilling('currency', $event)" />
        </SettingRow>
        <SettingRow label="Timezone" description="IANA zone used for the reported timestamps." source="Default" :resettable="false">
          <Combobox :model-value="billingValue('timezone')" :options="TIMEZONES" placeholder="UTC" @update:model-value="setBilling('timezone', $event)" />
        </SettingRow>
      </div>
    </section>

    <!-- Daemon / execution -->
    <section v-else-if="settings.active === 'daemon'" class="max-w-2xl space-y-4 p-5">
      <div class="panel divide-y divide-divider">
        <SettingRow
          v-for="def in GROUPS.daemon"
          :key="def.key"
          :label="def.label"
          :description="def.desc"
          :source="sourceOf(def)"
          :resettable="resettable(def)"
          @reset="resetValue(def)"
        >
          <select
            v-if="def.type === 'select'"
            class="input h-7! w-28! px-1.5! text-[12px]!"
            :value="(liveValue(def) as string | undefined) ?? ''"
            @change="setValue(def, ($event.target as HTMLSelectElement).value)"
          >
            <option v-for="o in def.options" :key="o" :value="o">{{ o }}</option>
          </select>
          <Toggle
            v-else-if="def.type === 'switch'"
            :model-value="!!liveValue(def)"
            @update:model-value="setValue(def, $event)"
          />
          <input
            v-else-if="def.type === 'number'"
            class="input h-7! w-24! text-[12px]!"
            type="number"
            :value="liveValue(def) as number | undefined ?? ''"
            @change="setValue(def, Number(($event.target as HTMLInputElement).value))"
          />
          <input
            v-else
            class="input h-7! w-44! text-[12px]!"
            :placeholder="def.placeholder"
            :value="liveValue(def) as string | undefined ?? ''"
            @change="setValue(def, ($event.target as HTMLInputElement).value)"
          />
        </SettingRow>
      </div>
    </section>

    <!-- Settings TOML: open the config file in the editor -->
    <section v-else-if="settings.active === 'toml'" class="max-w-2xl space-y-4 p-5">
      <div>
        <h1 class="text-[15px] font-semibold">Settings TOML</h1>
        <p class="mt-0.5 text-[12px] text-muted-foreground">
          The layered configuration is persisted as <code class="chip">.metteur/config.toml</code>
          (user: <code class="chip">config.toml</code> in the daemon home, workspace:
          <code class="chip">&lt;workspace&gt;/.metteur/config.toml</code>). Open the desired file
          in the editor to edit it with full syntax support; saving the tab persists the layer
          through the daemon.
        </p>
      </div>
      <div class="panel divide-y divide-divider">
        <button
          class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-accent"
          type="button"
          @click="openConfigDoc('user')"
        >
          <UserCog class="h-4 w-4 text-primary" />
          <div class="min-w-0 flex-1">
            <p class="text-[13px] font-medium">User config.toml</p>
            <p class="text-[12px] text-muted-foreground">Global settings, applied to every workspace.</p>
          </div>
          <span class="btn btn-outline h-7! px-2.5! text-[12px]! pointer-events-none">Open in editor</span>
        </button>
        <button
          class="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-accent"
          type="button"
          :disabled="!config.hasWorkspace"
          :title="config.hasWorkspace ? undefined : 'Open a workspace first'"
          @click="openConfigDoc('workspace')"
        >
          <FileCog class="h-4 w-4 text-primary" />
          <div class="min-w-0 flex-1">
            <p class="text-[13px] font-medium">Workspace config.toml</p>
            <p class="text-[12px] text-muted-foreground">Overrides the user layer per key for this workspace.</p>
          </div>
          <span class="btn btn-outline h-7! px-2.5! text-[12px]! pointer-events-none">Open in editor</span>
        </button>
      </div>
    </section>
  </div>

  <!-- Add / Edit configuration modals -->
  <ConfigModal
    v-if="modelModal"
    :title="modelEditName ? 'Edit model' : 'Add model'"
    :fields="MODEL_FIELDS"
    :initial="modelModalInitial()"
    :confirm-label="modelEditName ? 'Save' : 'Add'"
    @confirm="submitModel"
    @cancel="modelModal = false"
  />
  <ConfigModal
    v-if="mcpModal"
    title="Add MCP server"
    :fields="MCP_FIELDS"
    confirm-label="Add"
    @confirm="submitMcp"
    @cancel="mcpModal = false"
  />
  <ConfigModal
    v-if="lspModal"
    title="Add language server"
    :fields="LSP_FIELDS"
    confirm-label="Add"
    @confirm="submitLsp"
    @cancel="lspModal = false"
  />
</template>
