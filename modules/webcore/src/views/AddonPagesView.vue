<script setup lang="ts">
import AddonPagesPanel from '@/components/addons/AddonPagesPanel.vue'
import {useTabsStore} from '@/stores/tabs'
useTabsStore().openSurface('addon-pages')
</script>
<template><AddonPagesPanel/></template>
