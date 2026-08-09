<script setup lang="ts">
const props = withDefaults(defineProps<{ collapsed?: boolean; panelPadding?: string | number }>(), { collapsed: false, panelPadding: 0 })
</script>

<template>
  <section class="workspace-panel a-block" :class="{ collapsed }" :style="{ padding: typeof props.panelPadding === 'number' ? `${props.panelPadding}px` : props.panelPadding }"><slot /></section>
</template>

<style scoped>
.workspace-panel { min-width: 0; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
.workspace-panel.collapsed { display: none; }
</style>
