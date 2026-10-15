<script setup lang="ts">
import PromptPanel from '@/components/resources/PromptPanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('prompts')
</script>
<template><main class="pv-main"><PromptPanel /></main></template>
