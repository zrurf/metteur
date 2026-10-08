import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import type { AddonInfo, McpServerInfo, UsageSummary } from '@/core'
import { useWorkspaceStore } from './workspace'

/**
 * Addon registry.
 *
 * Lists installed addons, toggles each one's enabled state and shows the usage
 * summary of the workspace's most recent run on refresh.
 */
export const useAddonStore = defineStore('addon', () => {
  const workspace = useWorkspaceStore()
  const addons = ref<AddonInfo[]>([])
  const usage = ref<UsageSummary | null>(null)
  const error = ref('')
  const mcpServers = ref<McpServerInfo[]>([])
  const mcpError = ref('')
  let generation = 0

  async function refresh() {
    const request = ++generation
    const scope = workspace.active?.path
    const [a, runs, mcp] = await Promise.all([gateway.listAddons(), loadRuns(), gateway.listMcpServers(scope)])
    if (request !== generation) return
    if (a.ok) addons.value = a.data
    mcpServers.value = mcp.ok ? mcp.data : []
    mcpError.value = mcp.ok ? '' : mcp.error
    usage.value = runs
  }

  async function loadRuns(): Promise<UsageSummary | null> {
    const ws = workspace.active
    if (!ws) return null
    const runs = await gateway.listExecutions(ws.path)
    const runId = runs.ok ? runs.data.find((r) => r.status === 'Running')?.runId ?? runs.data[0]?.runId : undefined
    if (!runId) return null
    const u = await gateway.getExecutionUsage(ws.path, runId)
    return u.ok ? u.data : null
  }

  /** Toggle an addon in the scope that owns it (workspace addons need the
   *  workspace path, or the daemon resolves them against the global dir). */
  async function setEnabled(target: AddonInfo, enabled: boolean): Promise<string> {
    error.value = ''
    const scope = target.scope === 'workspace' ? target.scopeRoot : ''
    if (scope === undefined) return (error.value = 'Addon workspace identity is unavailable. Refresh before changing it.')
    const r = await gateway.setAddonEnabled(target.id, enabled, scope)
    if (!r.ok) return (error.value = r.error)
    await refresh()
    return ''
  }

  watch(() => workspace.active?.path, () => {
    mcpServers.value = []
    mcpError.value = ''
    void refresh()
  })

  return { addons, usage, error, mcpServers, mcpError, refresh, setEnabled }
})
