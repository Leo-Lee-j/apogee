<script setup lang="ts">
withDefaults(defineProps<{ dashed?: boolean }>(), { dashed: false })
</script>

<template>
  <header class="workspace-toolbar" :class="{ dashed }"><slot /></header>
</template>

<style scoped>
.workspace-toolbar { height: var(--a-h-toolbar); flex: 0 0 auto; display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 0 10px; background: var(--a-bg-2); }
.workspace-toolbar.dashed { border-bottom: 1px dashed var(--a-border); }
.workspace-toolbar:not(.dashed) { border-bottom: 1px solid var(--a-border); }
</style>
