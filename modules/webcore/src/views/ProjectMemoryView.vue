<script setup lang="ts">
import MemoryPanel from '@/components/resources/MemoryPanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('memory')
</script>
<template><main class="pv-main"><MemoryPanel /></main></template>
