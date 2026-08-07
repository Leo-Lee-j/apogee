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
  <WorkspaceDialog v-if="app.createDialogOpen" />
</template>
