<script setup lang="ts">
import { defineAsyncComponent } from 'vue'
import LauncherShell from './LauncherShell.vue'
import AppBody from './AppBody.vue'
import { useAppStore } from '@/stores/app'

const WorkspaceDialog = defineAsyncComponent(() => import('@/views/WorkspaceCreateDialog.vue'))
const app = useAppStore()
</script>

<template>
  <LauncherShell v-if="app.shell === 'launcher' || app.shell === 'opening'" />
  <AppBody v-else />
  <Transition name="workspace-dialog" appear>
    <WorkspaceDialog v-if="app.createDialogOpen" />
  </Transition>
</template>

<style>
.workspace-dialog-enter-active,
.workspace-dialog-leave-active {
  transition: opacity 160ms var(--a-ease);
}
.workspace-dialog-enter-active .dialog,
.workspace-dialog-leave-active .dialog {
  transition:
    opacity 160ms var(--a-ease),
    transform 160ms var(--a-ease);
}
.workspace-dialog-enter-from,
.workspace-dialog-leave-to {
  opacity: 0;
}
.workspace-dialog-enter-from .dialog {
  opacity: 0;
  transform: translateY(-6px) scale(0.98);
}
.workspace-dialog-leave-to .dialog {
  opacity: 0;
  transform: translateY(-3px) scale(0.99);
}
@media (prefers-reduced-motion: reduce) {
  .workspace-dialog-enter-active,
  .workspace-dialog-leave-active,
  .workspace-dialog-enter-active .dialog,
  .workspace-dialog-leave-active .dialog {
    transition-duration: 1ms;
  }
}
</style>
