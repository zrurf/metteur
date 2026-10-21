<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { Ellipsis } from '@lucide/vue'
const root=ref<HTMLDetailsElement|null>(null)
function close(){if(root.value)root.value.open=false}
function outside(event:PointerEvent){if(!root.value?.contains(event.target as Node))close()}
function escape(){close();root.value?.querySelector('summary')?.focus()}
onMounted(()=>document.addEventListener('pointerdown',outside))
onBeforeUnmount(()=>document.removeEventListener('pointerdown',outside))
</script>
<template><details ref="root" class="more-actions" @keydown.esc.prevent="escape"><summary class="btn" aria-label="More actions" data-tip="none"><Ellipsis :size="17" /></summary><div class="more-panel" @click="close"><slot /></div></details></template>
<style scoped>
.more-actions{position:relative}.more-actions summary{list-style:none;cursor:pointer;display:flex;align-items:center;justify-content:center}.more-actions summary::-webkit-details-marker{display:none}.more-actions summary:focus-visible{outline:2px solid var(--primary);outline-offset:2px}.more-panel{position:absolute;z-index:20;right:0;bottom:calc(100% + 8px);min-width:180px;padding:6px;background:var(--popover);border:1px solid var(--border);border-radius:10px;box-shadow:var(--shadow-popover);backdrop-filter:blur(16px)}.more-panel :deep(button){display:block;text-align:left;width:100%;border-radius:6px;padding:10px 12px;font-size:13px}.more-panel :deep(button:hover),.more-panel :deep(button:focus-visible){background:var(--hover)}.more-panel :deep(button:disabled){opacity:.45}
</style>
