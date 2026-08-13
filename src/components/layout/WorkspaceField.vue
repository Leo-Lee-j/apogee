<script setup lang="ts">
defineProps<{ label: string; hint?: string }>()
</script>

<template>
  <label class="workspace-field"><span class="workspace-field-label">{{ label }}</span><slot /><small v-if="hint" class="workspace-field-hint">{{ hint }}</small></label>
</template>

<style scoped>
.workspace-field { display: grid; gap: var(--workspace-field-gap, 5px); min-width: 0; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.workspace-field-label { line-height: var(--a-lh-sm); }
.workspace-field-hint { color: var(--a-text-faint); font-size: var(--a-fs-xs); line-height: var(--a-lh-xs); }
.workspace-field :deep(.n-input), .workspace-field :deep(.n-input-number), .workspace-field :deep(.n-select) { width: 100%; }
.workspace-field :deep(.n-input__input-el), .workspace-field :deep(.n-input-number-input__input) { height: var(--a-h-input-md); min-height: var(--a-h-input-md); line-height: var(--a-lh); }
</style>
