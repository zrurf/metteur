<script setup lang="ts">
import SchedulePanel from '@/components/continuations/SchedulePanel.vue'
import { useTabsStore } from '@/stores/tabs'
useTabsStore().openSurface('schedules')
</script>
<template><SchedulePanel /></template>
