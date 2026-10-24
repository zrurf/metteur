<script setup lang="ts">
import { nextTick } from 'vue'
const props = defineProps<{
  modelValue: string
  label: string
  items: { value: string; label: string }[]
}>()
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
async function keydown(event: KeyboardEvent, index: number) {
  let target: number
  if (event.key === 'ArrowRight') target = (index + 1) % props.items.length
  else if (event.key === 'ArrowLeft') target = (index + props.items.length - 1) % props.items.length
  else if (event.key === 'Home') target = 0
  else if (event.key === 'End') target = props.items.length - 1
  else return
  event.preventDefault()
  const nav = (event.currentTarget as HTMLElement).parentElement
  emit('update:modelValue', props.items[target]!.value)
  await nextTick()
  nav?.querySelectorAll<HTMLButtonElement>('[role=tab]')[target]?.focus()
}
</script>

<template>
  <nav class="surface-tabs" role="tablist" :aria-label="label">
    <button
      v-for="(item, index) in items"
      :key="item.value"
      type="button"
      role="tab"
      :aria-selected="modelValue === item.value"
      :tabindex="modelValue === item.value ? 0 : -1"
      @click="emit('update:modelValue', item.value)"
      @keydown="keydown($event, index)"
    >
      {{ item.label }}
    </button>
  </nav>
</template>
