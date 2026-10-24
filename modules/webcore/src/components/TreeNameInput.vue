<script setup lang="ts">
import { nextTick, onMounted, ref } from 'vue'
import { File, Folder } from '@lucide/vue'
const props = defineProps<{
  modelValue: string
  kind?: 'file' | 'folder'
  label: string
  error?: string
  busy?: boolean
}>()
const emit = defineEmits<{ 'update:modelValue': [value: string]; confirm: []; cancel: [] }>()
const input = ref<HTMLInputElement>()
onMounted(async () => {
  await nextTick()
  input.value?.focus()
  input.value?.select()
})
</script>
<template>
  <div class="tree-name-editor" @click.stop>
    <div class="tree-name-row">
      <component :is="kind === 'folder' ? Folder : File" :size="14" class="shrink-0 text-subtle" />
      <input
        ref="input"
        :value="modelValue"
        :aria-label="label"
        :aria-invalid="!!error"
        :disabled="busy"
        class="tree-name-input"
        autocomplete="off"
        spellcheck="false"
        @input="emit('update:modelValue', ($event.target as HTMLInputElement).value)"
        @keydown.enter.prevent.stop="!busy && emit('confirm')"
        @keydown.esc.prevent.stop="emit('cancel')"
        @blur="!busy && !error && emit('cancel')"
      />
    </div>
    <p v-if="error" class="tree-name-error" role="alert">{{ error }}</p>
  </div>
</template>
<style scoped>
.tree-name-row {
  display: flex;
  align-items: center;
  gap: 5px;
  min-height: 26px;
  padding-right: 4px;
}
.tree-name-input {
  min-width: 0;
  flex: 1;
  width: 100%;
  height: 23px;
  border: 1px solid var(--primary);
  border-radius: 2px;
  padding: 0 5px;
  background: var(--background);
  color: var(--foreground);
  font: inherit;
  outline: none;
  box-shadow: 0 0 0 1px color-mix(in srgb, var(--primary) 15%, transparent);
}
.tree-name-input[aria-invalid='true'] {
  border-color: var(--danger);
}
.tree-name-error {
  margin: 2px 4px 4px 19px;
  padding: 4px 6px;
  background: var(--danger-soft);
  border: 1px solid var(--danger);
  color: var(--danger);
  font-size: 11px;
  overflow-wrap: anywhere;
}
</style>
