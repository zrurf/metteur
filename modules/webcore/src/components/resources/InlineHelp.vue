<script setup lang="ts">
import { Info } from '@lucide/vue'
defineProps<{ text: string }>()
</script>
<template><button type="button" class="inline-help" :title="text" :aria-label="text"><Info :size="15" /></button></template>
<style scoped>
.inline-help { display:inline-flex; vertical-align:middle; align-items:center; justify-content:center; width:28px; height:28px; flex-shrink:0; border-radius:7px; color:var(--muted-foreground); cursor:help; }
.inline-help:hover,.inline-help:focus-visible { background:var(--hover); color:var(--foreground); }
.inline-help:focus-visible { outline:2px solid var(--primary); outline-offset:2px; }
</style>
