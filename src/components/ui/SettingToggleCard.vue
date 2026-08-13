<script setup lang="ts">
import AppSwitch from './AppSwitch.vue'

defineProps<{
  title: string
  description: string
  value: boolean
}>()

const emit = defineEmits<{ 'update:value': [value: boolean] }>()
</script>

<template>
  <div class="setting-toggle-card">
    <div class="setting-toggle-main">
      <span class="setting-toggle-title">{{ title }}</span>
      <AppSwitch :value="value" @update:value="(next: boolean) => emit('update:value', next)" />
    </div>
    <p class="setting-toggle-description">{{ description }}</p>
    <div v-if="$slots.default" class="setting-toggle-actions"><slot /></div>
  </div>
</template>

<style scoped>
.setting-toggle-card { margin: 0 0 8px; padding: 8px 10px; border: 1px solid var(--a-border-strong); border-radius: 6px; background: var(--a-bg-2); }
.setting-toggle-card:last-child { margin-bottom: 0; }
.setting-toggle-main { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: var(--a-h-control-xs); }
.setting-toggle-title { color: var(--a-text); font-size: var(--a-fs-base); font-weight: 500; line-height: var(--a-lh); }
.setting-toggle-description { margin: 2px 0 0; color: var(--a-text-dim); font-size: var(--a-fs-sm); font-weight: 400; line-height: var(--a-lh-sm); }
.setting-toggle-actions { margin-top: 7px; }
</style>
