<script setup lang="ts">
withDefaults(defineProps<{ columns?: number }>(), { columns: 1 })
</script>

<template><div class="workspace-form" :class="`columns-${columns}`"><slot /></div></template>

<style scoped>
.workspace-form { display: grid; gap: var(--workspace-form-gap, 12px); align-content: start; }
.workspace-form.columns-2 { grid-template-columns: repeat(2, minmax(0, 1fr)); }
@media (max-width: 680px) { .workspace-form.columns-2 { grid-template-columns: 1fr; } }
</style>
