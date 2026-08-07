<script setup lang="ts">
import { defineAsyncComponent } from 'vue'
import LauncherShell from './LauncherShell.vue'
import AppBody from './AppBody.vue'
import { useAppStore } from '@/stores/app'

const WorkspaceDialog = defineAsyncComponent(() => import('@/views/WorkspaceCreateDialog.vue'))
const app = useAppStore()
</script>

<template>
  <LauncherShell v-if="app.shell === 'launcher'" />
  <!--
    打开 / 切换工作区时整屏转圈，加载完成后主界面淡入。
    用自研 CSS spinner 而不是 NSpin：RootShell 在两个 S5 变体里都是首屏组件，
    变体 B 要求首屏零 naive-ui（见 vite.config.ts 的 S5 注释）。
  -->
  <Transition
    v-else-if="app.shell === 'workspace' || app.shell === 'closing'"
    name="app-fade"
    appear
  >
    <AppBody />
  </Transition>
  <div v-else class="opening-screen" role="status" aria-live="polite">
    <span class="opening-spinner" aria-hidden="true" />
    <span class="opening-text">正在打开工作区…</span>
  </div>
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
  .app-fade-enter-active {
    transition-duration: 1ms;
  }
}

/* ── 打开 / 切换工作区 ──
   加载动画用整屏 spinner；加载完成后 AppBody 淡入。背景用 --a-bg-1 与主界面一致，
   spinner 消失与界面淡入在同一帧叠加，不会露出底色断层。 */
.app-fade-enter-active {
  transition: opacity 200ms var(--a-ease);
}
.app-fade-enter-from {
  opacity: 0;
}
.opening-screen {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 14px;
  background: var(--a-bg-1);
}
.opening-spinner {
  width: 28px;
  height: 28px;
  border: 3px solid var(--a-border-strong);
  border-top-color: var(--a-primary);
  border-radius: 50%;
  animation: opening-spin 0.7s linear infinite;
}
.opening-text {
  color: var(--a-text-dim);
  font: 13px var(--a-font-ui);
}
@keyframes opening-spin {
  to { transform: rotate(360deg); }
}
</style>
