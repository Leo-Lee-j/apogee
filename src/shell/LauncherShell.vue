<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useUiStore } from '@/stores/ui'
import { AppIcon } from '@/components/ui'
import { IconAdd, IconFolderOpen, IconMoon } from '@/components/ui/icons'
import { windowControls, type Unlisten } from '@/ipc'
import logoUrl from '../../src-tauri/icons/128x128.png'

const app = useAppStore()
const ui = useUiStore()
const focused = ref(0)
const maximized = ref(false)
let stopWatching: Unlisten | undefined

const rows = computed(() => [
  ...app.workspaces.map((workspace) => ({ kind: 'workspace' as const, workspace })),
  { kind: 'create' as const },
  { kind: 'open' as const },
])

onMounted(async () => {
  if (!windowControls.available) return
  maximized.value = await windowControls.isMaximized()
  stopWatching = await windowControls.onMaximizeChange((value) => { maximized.value = value })
})
onBeforeUnmount(() => stopWatching?.())

async function pickDirectory() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  return await open({ directory: true, multiple: false })
}

async function activate(index: number) {
  const row = rows.value[index]
  if (!row) return
  if (row.kind === 'workspace') {
    if (row.workspace.status === 'available') await app.openWorkspace({ workspaceId: row.workspace.id })
    return
  }
  if (row.kind === 'open') {
    const path = await pickDirectory()
    if (typeof path === 'string') await app.openWorkspace({ path })
    return
  }
  app.createDialogOpen = true
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'ArrowDown') focused.value = Math.min(rows.value.length - 1, focused.value + 1)
  else if (event.key === 'ArrowUp') focused.value = Math.max(0, focused.value - 1)
  else if (event.key === 'Enter') void activate(focused.value)
  else return
  event.preventDefault()
}

function statusText(status: string) {
  return status === 'missing' ? '目录不可用' : status === 'invalid' ? '工作区无效' : ''
}
</script>

<template>
  <div class="launcher" tabindex="0" @keydown="onKeydown">
    <header class="launcher-titlebar" data-tauri-drag-region>
      <div class="launcher-spacer" />
      <button class="title-icon" type="button" title="切换主题" @click="ui.themeMode = ui.isDark ? 'light' : 'dark'; ui.applyTheme()">
        <AppIcon :component="IconMoon" :size="15" />
      </button>
      <div v-if="windowControls.available" class="window-controls">
        <button type="button" title="最小化" @click="windowControls.minimize()"><span class="minimize" /></button>
        <button type="button" :title="maximized ? '向下还原' : '最大化'" @click="windowControls.toggleMaximize()"><span :class="maximized ? 'restore' : 'maximize'" /></button>
        <button class="close" type="button" title="关闭" @click="windowControls.close()"><span class="close-mark" /></button>
      </div>
    </header>

    <main class="launcher-content">
      <div class="brand" aria-label="Apogee">
        <img :src="logoUrl" alt="" width="42" height="42" />
        <span>Apogee</span>
      </div>

      <section class="workspace-list" aria-label="工作区">
        <button
          v-for="(row, index) in rows"
          :key="row.kind === 'workspace' ? row.workspace.id : row.kind"
          class="workspace-row"
          :class="{ focused: focused === index, command: row.kind !== 'workspace', unavailable: row.kind === 'workspace' && row.workspace.status !== 'available' }"
          type="button"
          :disabled="app.shell === 'opening'"
          @focus="focused = index"
          @click="activate(index)"
        >
          <template v-if="row.kind === 'workspace'">
            <span class="workspace-copy">
              <strong>{{ row.workspace.name }}</strong>
              <span class="path" :title="row.workspace.path">{{ row.workspace.path }}</span>
            </span>
            <span v-if="app.busyWorkspaceId === row.workspace.id" class="spinner" aria-label="正在打开" />
            <span v-else-if="statusText(row.workspace.status)" class="status">{{ statusText(row.workspace.status) }}</span>
          </template>
          <template v-else>
            <AppIcon :component="row.kind === 'create' ? IconAdd : IconFolderOpen" :size="17" />
            <span>{{ row.kind === 'create' ? '新建工作区' : '打开本地工作区' }}</span>
            <span v-if="app.busyWorkspaceId === row.kind" class="spinner" />
          </template>
        </button>
      </section>
      <p v-if="app.lastError" class="launcher-error">{{ app.lastError.message }}</p>
    </main>
  </div>
</template>

<style scoped>
.launcher { height: 100%; background: var(--a-bg-1); outline: none; }
.launcher-titlebar { height: var(--a-h-titlebar); display: flex; align-items: stretch; }
.launcher-spacer { flex: 1; }
.title-icon, .window-controls button { border: 0; background: transparent; color: var(--a-text-dim); cursor: pointer; }
.title-icon { width: 36px; }
.title-icon:hover, .window-controls button:hover { background: var(--a-hover); color: var(--a-text); }
.window-controls { display: flex; }
.window-controls button { position: relative; width: 46px; }
.window-controls .close:hover { background: var(--a-danger-bg); color: var(--a-danger); }
.minimize, .maximize, .restore, .close-mark { position: absolute; left: 18px; top: 17px; width: 10px; height: 1px; background: currentColor; }
.maximize { top: 13px; height: 9px; border: 1px solid currentColor; background: transparent; }
.restore { top: 14px; height: 8px; border: 1px solid currentColor; background: transparent; box-shadow: 2px -2px 0 -1px var(--a-bg-1), 2px -2px 0 0 currentColor; }
.close-mark { top: 17px; transform: rotate(45deg); }
.close-mark::after { content: ''; position: absolute; inset: 0; transform: rotate(90deg); background: currentColor; }
.launcher-content { width: min(680px, calc(100% - 48px)); margin: 0 auto; padding-top: max(76px, 10vh); }
.brand { display: flex; align-items: center; justify-content: center; gap: 12px; margin-bottom: 32px; color: var(--a-text); font-size: 24px; font-weight: 600; }
.brand img { display: block; border-radius: 9px; }
.workspace-list { overflow: hidden; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-2); box-shadow: var(--a-shadow-sm); }
.workspace-row { width: 100%; min-height: 58px; display: flex; align-items: center; gap: 12px; padding: 8px 16px; border: 0; border-bottom: 1px solid var(--a-border); background: transparent; color: var(--a-text); font: inherit; text-align: left; cursor: pointer; }
.workspace-row:last-child { border-bottom: 0; }
.workspace-row:hover, .workspace-row.focused { background: var(--a-hover); }
.workspace-row:focus-visible { outline: 1px solid var(--a-focus); outline-offset: -1px; }
.workspace-row.command { min-height: 48px; color: var(--a-text-soft); }
.workspace-row.command :deep(.n-icon) { color: var(--a-primary); }
.workspace-row.unavailable { color: var(--a-text-dim); }
.workspace-copy { min-width: 0; display: flex; flex: 1; flex-direction: column; gap: 2px; }
.workspace-copy strong { overflow: hidden; font-size: 14px; font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.path { overflow: hidden; color: var(--a-text-dim); font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
.status { color: var(--a-error); font-size: 12px; }
.spinner { width: 14px; height: 14px; border: 2px solid var(--a-border-strong); border-top-color: var(--a-primary); border-radius: 50%; animation: spin .7s linear infinite; }
.launcher-error { margin: 12px 0 0; color: var(--a-error); font-size: 12px; text-align: center; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (max-height: 620px) { .launcher-content { padding-top: 34px; } }
</style>
