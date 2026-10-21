<script setup lang="ts">
import MemoryPanel from '@/components/resources/MemoryPanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('memory')
</script>
<template><div class="pv-main"><MemoryPanel /></div></template>
