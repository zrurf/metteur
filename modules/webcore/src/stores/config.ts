import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { gateway } from '@/core'
import type { DaemonConfig } from '@/core'
import { useWorkspaceStore } from './workspace'

/**
 * Layered daemon configuration (VSCode user/workspace model).
 *
 * The daemon persists two layers — the *user* layer (`~/.metteur/config.toml`)
 * and the *workspace* layer (`<ws>/.metteur/config.toml`). The effective value
 * is the workspace layer merged over the user layer per key, mirroring
 * `metteur_shared::config::Config::merge` (empty collections fall back to the
 * user layer). Edits land in exactly one layer; saving persists it through
 * `daemon.setConfig`, which also re-merges the affected workspace.
 */

/** Effective-config merge: workspace overrides user per (nested) key. */
export function mergeConfig(user: DaemonConfig, ws: DaemonConfig): DaemonConfig {
  const out: DaemonConfig = { ...user }
  for (const key of Object.keys(ws)) {
    const wv = ws[key]
    if (key === 'oversight') {
      out[key] = mergePresence(user[key], wv)
      continue
    }
    if (wv === undefined || wv === null) continue
    if (Array.isArray(wv)) {
      if (wv.length > 0) out[key] = wv
    } else if (typeof wv === 'object') {
      const uv = user[key]
      if (uv && typeof uv === 'object' && !Array.isArray(uv)) {
        out[key] = mergeSection(uv as Record<string, unknown>, wv as Record<string, unknown>)
      } else if (Object.keys(wv as Record<string, unknown>).length > 0) {
        out[key] = wv
      }
    } else {
      out[key] = wv
    }
  }
  // Trust policy is global-only; section collection rules remain unchanged.
  if (user.addon || ws.addon)
    out.addon = {
      ...out.addon,
      require_signature: user.addon?.require_signature,
      signing_keys: user.addon?.signing_keys,
    }
  if (ws.llm?.project_instruction_files?.join('\0') === 'METTEUR.md\0AGENTS.md') {
    out.llm = { ...out.llm, project_instruction_files: user.llm?.project_instruction_files }
  } else if (ws.llm?.project_instruction_files) {
    out.llm = { ...out.llm, project_instruction_files: ws.llm.project_instruction_files }
  }
  return out
}

function mergePresence(base: unknown, override: unknown): unknown {
  if (override && typeof override === 'object' && !Array.isArray(override)) {
    const out: Record<string, unknown> =
      base && typeof base === 'object' && !Array.isArray(base)
        ? { ...(base as Record<string, unknown>) }
        : {}
    for (const [key, value] of Object.entries(override)) out[key] = mergePresence(out[key], value)
    return out
  }
  return override === undefined ? base : override
}

/** Field-level merge inside one section; empty collections fall back. */
function mergeSection(
  user: Record<string, unknown>,
  ws: Record<string, unknown>,
): Record<string, unknown> {
  const out: Record<string, unknown> = { ...user }
  for (const key of Object.keys(ws)) {
    const wv = ws[key]
    if (wv === undefined || wv === null) continue
    if (Array.isArray(wv)) {
      if (wv.length > 0) out[key] = wv
    } else if (typeof wv === 'object') {
      if (Object.keys(wv as Record<string, unknown>).length > 0) out[key] = wv
    } else {
      out[key] = wv
    }
  }
  return out
}

export type ConfigLayer = 'user' | 'workspace'

export const useConfigStore = defineStore('config', () => {
  const workspace = useWorkspaceStore()
  /** User (global) layer, edited in place before `save('user')`. */
  const user = ref<DaemonConfig>({})
  /** Workspace layer, edited in place before `save('workspace')`. */
  const ws = ref<DaemonConfig>({})
  /** Whether both layers were fetched from the daemon. */
  const loaded = ref(false)
  const saving = ref(false)
  const lastError = ref('')
  const legacyUser = ref(false)
  const legacyWorkspace = ref(false)
  const defaults = ref<DaemonConfig>({})

  const hasWorkspace = computed(() => !!workspace.active)
  /** User + workspace merged per key (what actually applies). */
  const effective = computed(() => {
    const base: DaemonConfig = { ...defaults.value, ...user.value }
    for (const [section, value] of Object.entries(user.value)) {
      if (value && typeof value === 'object' && !Array.isArray(value)) {
        base[section] = { ...(defaults.value[section] as object), ...value }
      }
    }
    // Preserve the historical distinction between an absent LSP section and
    // a present section whose debounce field was omitted.
    if (user.value.lsp && user.value.lsp.debounce_ms === undefined)
      base.lsp = { ...base.lsp, debounce_ms: 300 }
    return mergeConfig(base, ws.value)
  })

  /** Fetch the daemon's compatibility projection; no file is rewritten. */
  let loadEpoch = 0
  async function load(): Promise<void> {
    const epoch = ++loadEpoch
    const activePath = workspace.active?.path
    loaded.value = false
    const [u, w] = await Promise.all([
      gateway.getConfigState(''),
      activePath ? gateway.getConfigState(activePath) : Promise.resolve(null),
    ])
    if (epoch !== loadEpoch || workspace.active?.path !== activePath) return
    if (!u.ok || (w && !w.ok)) {
      lastError.value = !u.ok ? u.error : w && !w.ok ? w.error : ''
      ws.value = {}
      legacyWorkspace.value = false
      loaded.value = false
      return
    }
    user.value = u.data.overrides
    defaults.value = u.data.defaults
    legacyUser.value = u.data.legacy
    ws.value = w?.ok ? w.data.overrides : {}
    legacyWorkspace.value = w?.ok ? w.data.legacy : false
    lastError.value = ''
    loaded.value = true
  }

  /** Persist one layer; re-fetches both layers afterwards. Returns an error
   *  message, or `''` on success. */
  async function save(layer: ConfigLayer): Promise<string> {
    saving.value = true
    lastError.value = ''
    try {
      if (!loaded.value) return (lastError.value = 'Load configuration successfully before saving')
      if (layer === 'workspace' && !workspace.active)
        return (lastError.value = 'Open a workspace before saving its configuration')
      const cfg = { ...(layer === 'user' ? user.value : ws.value), config_version: 2 }
      const path = layer === 'workspace' ? (workspace.active?.path ?? '') : ''
      const r = await gateway.setConfig(cfg, path)
      if (!r.ok) {
        lastError.value = r.error
        return r.error
      }
      await load()
      return lastError.value
    } finally {
      saving.value = false
    }
  }

  /** Remove a key (or the whole section) from one layer, in place. */
  function resetKey(layer: ConfigLayer, section: string, key?: string) {
    const cfg = layer === 'user' ? user.value : ws.value
    const sec = cfg[section]
    if (!sec || typeof sec !== 'object' || Array.isArray(sec)) return
    const rec = sec as Record<string, unknown>
    if (key === undefined) delete cfg[section]
    else delete rec[key]
  }

  return {
    user,
    ws,
    legacyUser,
    legacyWorkspace,
    effective,
    hasWorkspace,
    loaded,
    saving,
    lastError,
    load,
    save,
    resetKey,
  }
})
