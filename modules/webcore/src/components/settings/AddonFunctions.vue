<script setup lang="ts">
import { ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { gateway, type FunctionItem } from '@/core'
import { fileRoute } from '@/lib/file-token'
import ConfigModal from '@/components/settings/ConfigModal.vue'

const props = defineProps<{ workspacePath: string }>()
const router = useRouter()
const functions = ref<FunctionItem[]>([])
const error = ref('')
const selected = ref<FunctionItem | null>(null)
const busy = ref(false)
const imported = ref<{ name: string; filePath: string } | null>(null)
let revision = 0
async function refresh() {
  const version = ++revision, path = props.workspacePath
  const result = await gateway.listFunctions(path)
  if (version !== revision || path !== props.workspacePath) return
  functions.value = result.ok ? result.data.filter((f) => f.source === 'addon') : []
  error.value = result.ok ? '' : result.error
}
watch(() => props.workspacePath, () => { selected.value = null; imported.value = null; void refresh() }, { immediate: true })
async function importCopy(values: Record<string, unknown>) {
  const source = selected.value, path = props.workspacePath
  if (!source || !path || busy.value) return
  busy.value = true
  const result = await gateway.importFunction(path, source, String(values.name ?? ''), String(values.file ?? ''))
  busy.value = false
  if (path !== props.workspacePath) return
  if (result.ok) { imported.value = result.data; selected.value = null; error.value = '' }
  else error.value = result.error
}
</script>

<template>
  <section class="panel space-y-3 p-4" data-testid="addon-function-library">
    <div class="flex items-center justify-between gap-3">
      <h2 class="text-[13px] font-medium">Addon function library</h2>
      <button class="btn btn-outline" type="button" @click="refresh">Refresh functions</button>
    </div>
    <p class="text-[12px] text-muted-foreground">Package functions are read-only. Import a separately named file to edit a copy with version history.</p>
    <div v-for="fn in functions" :key="fn.id" class="flex items-center gap-3 border-t border-divider pt-3">
      <div class="min-w-0 flex-1">
        <p class="break-all text-[13px] font-medium">{{ fn.name }}</p>
        <p class="text-[12px] text-muted-foreground">{{ fn.description }}</p>
        <p class="text-[12px] text-muted-foreground">{{ fn.inputs.map((p) => `${p.name}: ${p.type}`).join(', ') || 'No inputs' }} → {{ fn.outputs.map((p) => `${p.name}: ${p.type}`).join(', ') || 'No outputs' }}</p>
      </div>
      <button class="btn btn-outline" type="button" :disabled="!workspacePath || !fn.addonBinding || busy" @click="selected = fn">Import copy</button>
    </div>
    <p v-if="functions.length === 0" class="text-[12px] text-muted-foreground">No package functions available in this scope.</p>
    <p v-if="error" role="alert" class="text-[12px] text-destructive">{{ error }}</p>
    <button v-if="imported" class="btn btn-outline" type="button" @click="router.push(fileRoute(imported.filePath))">Open {{ imported.name }}</button>
  </section>
  <ConfigModal v-if="selected" title="Import editable function copy" confirm-label="Import copy"
    :fields="[{ key: 'name', label: 'New function name', type: 'text' }, { key: 'file', label: 'New blueprint file path', type: 'text' }]"
    :initial="{ name: `${selected.name}Copy`, file: `functions/${selected.name}Copy.blueprint` }"
    @confirm="importCopy" @cancel="selected = null" />
</template>
