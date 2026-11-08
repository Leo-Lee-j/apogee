<script setup lang="ts">
import { computed, defineAsyncComponent, onBeforeUnmount, onMounted, ref } from 'vue'
import type { DropdownOption } from 'naive-ui'
import { useAppStore } from '@/stores/app'
import { useUiStore } from '@/stores/ui'
import { useUpdateStore } from '@/stores/update'
import { AppDropdown, AppIcon } from '@/components/ui'
import { IconAdd, IconFolderOpen, IconMore, IconMoon, IconSun } from '@/components/ui/icons'
import { inTauri, windowControls, type Unlisten } from '@/ipc'
import logoUrl from '../../src-tauri/icons/128x128.png'

const app = useAppStore()
const ui = useUiStore()
const updates = useUpdateStore()
const updateOpen = ref(false)
const UpdatePanel = defineAsyncComponent(() => import('@/components/settings/UpdatePanel.vue'))
const focused = ref(0)
const maximized = ref(false)
let stopWatching: Unlisten | undefined

const rows = computed(() => [
  ...app.workspaces.map((workspace) => ({ kind: 'workspace' as const, workspace })),
  { kind: 'create' as const },
  { kind: 'open' as const },
])

onMounted(async () => {
  void updates.loadVersion()
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
  if (app.shell === 'opening') return
  const row = rows.value[index]
  if (!row) return
  if (row.kind === 'workspace') {
    if (row.workspace.status === 'available') await app.openWorkspace({ workspaceId: row.workspace.id })
    else await relocate(row.workspace.id)
    return
  }
  if (row.kind === 'open') {
    const path = await pickDirectory()
    if (typeof path === 'string') await app.openWorkspace({ path })
    return
  }
  app.createDialogOpen = true
}

async function relocate(workspaceId: string) {
  const selected = await pickDirectory()
  if (typeof selected === 'string') await app.relocateWorkspace(workspaceId, selected)
}

function workspaceMenu(workspace: { id: string; status: string }): DropdownOption[] {
  return [
    { key: 'reveal', label: '在文件管理器中显示', disabled: workspace.status !== 'available' },
    { key: 'relocate', label: '重新定位' },
    { key: 'remove', label: '从列表移除' },
  ]
}

async function selectWorkspaceAction(workspace: { id: string }, key: string | number) {
  if (key === 'reveal') await app.revealWorkspace(workspace.id)
  else if (key === 'relocate') await relocate(workspace.id)
  else if (key === 'remove') removeWorkspace(workspace.id)
}

async function removeWorkspace(workspaceId: string) {
  if (confirm('仅从工作区列表移除记录？磁盘目录不会被删除。')) await app.forgetWorkspace(workspaceId)
}

function onKeydown(event: KeyboardEvent) {
  if (app.shell === 'opening') return
  if (event.key === 'ArrowDown') focused.value = Math.min(rows.value.length - 1, focused.value + 1)
  else if (event.key === 'ArrowUp') focused.value = Math.max(0, focused.value - 1)
  else if (event.key === 'Enter') void activate(focused.value)
  else if (event.key === 'Delete') {
    const row = rows.value[focused.value]
    if (row?.kind === 'workspace') void removeWorkspace(row.workspace.id)
  }
  else return
  event.preventDefault()
}

function statusText(status: string) {
  return status === 'missing' ? '目录不可用' : status === 'invalid' ? '工作区无效' : ''
}
</script>

<template>
  <div class="launcher" :class="{ 'updates-expanded': updateOpen }" tabindex="0" @keydown="onKeydown">
    <!-- 启动页标题栏没有 gap/padding，spacer 占满整个 header，裸 drag-region
         只响应直接点击 header 本身，会完全拖不动；必须用 deep（见 TitleBar 注释）。 -->
    <header class="launcher-titlebar" data-tauri-drag-region="deep">
      <div class="launcher-spacer" />
      <button class="title-icon" type="button" :title="ui.isDark ? '切换到亮色主题' : '切换到暗色主题'" :aria-label="ui.isDark ? '切换到亮色主题' : '切换到暗色主题'" @click="ui.themeMode = ui.isDark ? 'light' : 'dark'; ui.applyTheme()">
        <AppIcon :component="ui.isDark ? IconSun : IconMoon" :size="15" />
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
        <div
          v-for="(row, index) in rows"
          :key="row.kind === 'workspace' ? row.workspace.id : row.kind"
          class="workspace-row"
          :class="{ focused: focused === index, command: row.kind !== 'workspace', unavailable: row.kind === 'workspace' && row.workspace.status !== 'available' }"
          :role="'button'"
          :tabindex="index === focused ? 0 : -1"
          :aria-disabled="app.shell === 'opening'"
          @focus="focused = index"
          @focusin="focused = index"
          @click="activate(index)"
        >
          <template v-if="row.kind === 'workspace'">
            <span class="workspace-copy">
              <strong>{{ row.workspace.name }}</strong>
              <span class="path" :title="row.workspace.path">{{ row.workspace.path }}</span>
            </span>
            <span v-if="app.busyWorkspaceId === row.workspace.id" class="spinner" aria-label="正在打开" />
            <span v-else-if="statusText(row.workspace.status)" class="status">{{ statusText(row.workspace.status) }}</span>
            <AppDropdown
              v-if="app.shell !== 'opening' && app.busyWorkspaceId !== row.workspace.id"
              trigger="click"
              placement="bottom-end"
              :options="workspaceMenu(row.workspace)"
              @select="selectWorkspaceAction(row.workspace, $event)"
            >
              <button class="row-action remove-action" type="button" aria-label="更多工作区操作" title="更多工作区操作" @click.stop>
                <AppIcon :component="IconMore" :size="16" />
              </button>
            </AppDropdown>
          </template>
          <template v-else>
            <AppIcon :component="row.kind === 'create' ? IconAdd : IconFolderOpen" :size="17" />
            <span>{{ row.kind === 'create' ? '新建工作区' : '打开本地工作区' }}</span>
            <span v-if="app.busyWorkspaceId === row.kind" class="spinner" />
          </template>
        </div>
      </section>
      <p v-if="app.lastError" class="launcher-error">{{ app.lastError.message }}</p>
      <div v-if="inTauri" class="launcher-updates" @keydown.stop>
        <button type="button" class="update-entry" :aria-expanded="updateOpen" @click="updateOpen = !updateOpen">
          v{{ updates.currentVersion }} · {{ updates.updateAvailable ? `发现 v${updates.latest?.version}` : '软件更新' }}
          <span>{{ updateOpen ? '收起' : '展开' }}</span>
        </button>
        <UpdatePanel v-if="updateOpen" />
      </div>
    </main>
  </div>
</template>

<style scoped>
.launcher { height: 100%; background: var(--a-bg-1); outline: none; }
.launcher.updates-expanded { overflow-y: auto; }
.launcher-updates { margin-top: 20px; padding-bottom: 24px; }
.update-entry { display: flex; align-items: center; justify-content: space-between; width: 100%; padding: 8px 0; border: 0; background: transparent; color: var(--a-text-dim); font: inherit; font-size: var(--a-fs-sm); cursor: pointer; }
.update-entry:hover { color: var(--a-text); }
.update-entry:focus-visible { outline: 1px solid var(--a-focus); outline-offset: 3px; }
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
.launcher-content { width: min(700px, calc(100% - 40px)); margin: 0 auto; padding-top: max(52px, 8vh); }
.brand { display: flex; align-items: center; justify-content: center; gap: 10px; margin-bottom: 32px; color: var(--a-text); font-size: var(--a-fs-3xl); font-weight: 600; }
.brand img { display: block; border-radius: var(--a-radius-md); box-shadow: var(--a-shadow-sm); }
.workspace-list { overflow: hidden; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-2); box-shadow: var(--a-shadow-sm); }
.workspace-row { width: 100%; min-height: max(58px, calc(var(--a-lh) + var(--a-lh-sm) + 18px)); display: flex; align-items: center; gap: 10px; padding: 9px 14px; border: 0; border-bottom: 1px solid var(--a-border); background: transparent; color: var(--a-text); font: inherit; text-align: left; cursor: pointer; transition: background 150ms ease, color 150ms ease; }
.workspace-row[aria-disabled='true'] { cursor: default; }
.workspace-row:last-child { border-bottom: 0; }
.workspace-row:hover, .workspace-row.focused { background: var(--a-hover); }
.workspace-row:focus-visible { outline: 1px solid var(--a-focus); outline-offset: -1px; }
.workspace-row.command { min-height: max(48px, calc(var(--a-lh) + 18px)); color: var(--a-text-soft); }
.workspace-row.command :deep(.n-icon) { color: var(--a-primary); }
.workspace-row.unavailable { color: var(--a-text-dim); }
.workspace-copy { min-width: 0; display: flex; flex: 1; flex-direction: column; gap: 2px; }
.workspace-copy strong { overflow: hidden; font-size: var(--a-fs-md); font-weight: 600; text-overflow: ellipsis; white-space: nowrap; }
.path { overflow: hidden; color: var(--a-text-dim); font-size: var(--a-fs-sm); text-overflow: ellipsis; white-space: nowrap; }
.status { color: var(--a-error); font-size: var(--a-fs-sm); }
.row-action { flex: 0 0 28px; width: 28px; height: 28px; display: grid; place-items: center; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); cursor: pointer; opacity: 0; }
.workspace-row:hover .row-action, .workspace-row:focus-within .row-action { opacity: 1; }
.row-action:hover { background: var(--a-hover-strong); color: var(--a-text); }
.spinner { width: 14px; height: 14px; border: 2px solid var(--a-border-strong); border-top-color: var(--a-primary); border-radius: 50%; animation: spin .7s linear infinite; }
.launcher-error { margin: 12px 0 0; color: var(--a-error); font-size: var(--a-fs-sm); text-align: center; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (max-height: 620px) { .launcher-content { padding-top: 34px; } }
</style>
