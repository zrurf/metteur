<script setup lang="ts">
import TaskPanel from '@/components/continuations/TaskPanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('tasks')
</script>
<template><TaskPanel /></template>
