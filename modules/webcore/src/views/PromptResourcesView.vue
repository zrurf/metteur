<script setup lang="ts">
import PromptPanel from '@/components/resources/PromptPanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('prompts')
</script>
<template><div class="pv-main"><PromptPanel /></div></template>
