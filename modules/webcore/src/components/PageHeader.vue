<script setup lang="ts">
defineProps<{ title: string; description?: string; toolbar?: boolean }>()
</script>

<template>
  <header class="surface-header" :class="{ 'surface-header-toolbar': toolbar }">
    <div class="surface-heading-group">
      <div class="surface-heading">
        <h1 class="surface-title">
          <slot name="title">{{ title }}</slot>
        </h1>
        <slot name="help" />
      </div>
      <p v-if="description" class="surface-description">{{ description }}</p>
    </div>
    <div v-if="$slots.default" class="surface-actions"><slot /></div>
  </header>
</template>
