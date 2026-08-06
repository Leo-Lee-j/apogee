<script setup lang="ts">
import TabBar from '@/components/layout/TabBar.vue'
</script>

<template>
  <main class="workspace-area">
    <div class="workspace-head a-block">
      <TabBar />
      <slot name="toolbar" />
    </div>
    <slot />
  </main>
</template>

<style scoped>
.workspace-area {
  flex: 1 1 auto;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  gap: var(--a-gutter);
}
.workspace-head {
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  overflow: visible;
}
.workspace-head > :first-child {
  border-top-left-radius: inherit;
  border-top-right-radius: inherit;
}
.workspace-head > :last-child {
  border-bottom-left-radius: inherit;
  border-bottom-right-radius: inherit;
}
</style>
