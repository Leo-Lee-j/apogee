import { ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import { ipc, normalizeError, type AppError, type AppSettings, type WorkspaceDescriptor, type WorkspaceOpenResult } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import { useUiStore } from '@/stores/ui'

export type AppShellKind = 'launcher' | 'opening' | 'workspace' | 'closing'

export const useAppStore = defineStore('app', () => {
  const shell = ref<AppShellKind>('launcher')
  const workspaces = shallowRef<WorkspaceDescriptor[]>([])
  const busyWorkspaceId = ref<string | null>(null)
  const lastError = ref<AppError | null>(null)
  const createDialogOpen = ref(false)
  const settings = ref<AppSettings>({ settingsVersion: 1, defaultWorkspaceRoot: '', openLastWorkspaceOnStart: false, showMemoryMonitor: true, lowMemoryMode: false, proxyMode: 'system', proxyUrl: '', timeoutMs: 30000, verifyTls: true, themeMode: 'light', language: 'system', fontFamily: 'system', uiFontSize: 13, resultFontSize: 13 })
  let initialized = false

  async function refresh() {
    try {
      workspaces.value = await ipc.workspaceList()
      lastError.value = null
    } catch (error) {
      lastError.value = normalizeError(error)
    }
  }

  async function initialize() {
    if (initialized) return
    initialized = true
    try {
      settings.value = await ipc.appSettingsLoad()
      const ui = useUiStore()
      ui.themeMode = settings.value.themeMode
      ui.applyTheme()
      ui.applyFont(settings.value.fontFamily)
      ui.applyFontSizes(settings.value.uiFontSize, settings.value.resultFontSize)
      if (typeof document !== 'undefined') {
        const systemLanguage = typeof navigator !== 'undefined' && navigator.language.toLowerCase().startsWith('en') ? 'en-US' : 'zh-CN'
        document.documentElement.lang = settings.value.language === 'system' ? systemLanguage : settings.value.language
      }
    } catch (error) {
      lastError.value = normalizeError(error)
    }
    await refresh()
    if (settings.value.openLastWorkspaceOnStart) {
      const last = workspaces.value.find((item) => item.status === 'available')
      if (last) await openWorkspace({ workspaceId: last.id }, last.id)
    }
  }

  async function acceptOpen(result: WorkspaceOpenResult) {
    const tabs = useTabsStore()
    tabs.resetWorkspace()
    await useWorkspaceStore().enter(result)
    if (tabs.tabs.length === 0) tabs.open({ title: '新建请求' })
    shell.value = 'workspace'
    busyWorkspaceId.value = null
    await refresh()
  }

  /**
   * Rust 会话已建立但前端落盘/进入失败，回到启动页前必须回滚会话。
   * 否则下次打开同一工作区会撞上 WORKSPACE_ALREADY_OPEN（Rust 侧 open_workspace
   * 也会自愈替换孤儿会话，这里是第一道防线）。
   */
  async function rollbackSession() {
    try {
      await ipc.workspaceClose()
    } catch {
      /* 尽力回滚；失败时 Rust 侧会在下一次 open 时替换孤儿会话 */
    }
  }

  async function openWorkspace(input: { workspaceId?: string; path?: string }, busyId = input.workspaceId ?? 'path') {
    shell.value = 'opening'
    busyWorkspaceId.value = busyId
    lastError.value = null
    try {
      const result = await ipc.workspaceOpen(input)
      try {
        await acceptOpen(result)
      } catch (error) {
        await rollbackSession()
        throw error
      }
      return true
    } catch (error) {
      const appError = normalizeError(error)
      console.error('[openWorkspace]', appError)
      shell.value = 'launcher'
      busyWorkspaceId.value = null
      await refresh()
      // refresh() 成功时会把 lastError 清空，必须在它之后设置，
      // 否则打开失败的错误会被吞掉，启动页上只看到一闪而过的转圈。
      lastError.value = appError
      return false
    }
  }

  async function createWorkspace(path: string | undefined, name: string) {
    shell.value = 'opening'
    busyWorkspaceId.value = 'create'
    lastError.value = null
    try {
      const result = await ipc.workspaceCreate(path, name)
      try {
        await acceptOpen(result)
      } catch (error) {
        await rollbackSession()
        throw error
      }
      return true
    } catch (error) {
      const appError = normalizeError(error)
      console.error('[createWorkspace]', appError)
      shell.value = 'launcher'
      busyWorkspaceId.value = null
      lastError.value = appError
      return false
    }
  }

  async function saveSettings(next: AppSettings) {
    try {
      settings.value = await ipc.appSettingsSave(next)
      useUiStore().applyFontSizes(settings.value.uiFontSize, settings.value.resultFontSize)
      lastError.value = null
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      return false
    }
  }

  async function closeWorkspace() {
    if (shell.value !== 'workspace') return false
    shell.value = 'closing'
    const tabs = useTabsStore()
    const workspace = useWorkspaceStore()
    const stateSaved = await workspace.flushWorkspaceState()
    if (!stateSaved || !(await workspace.saveDirtyEnvironments()) || !(await tabs.flushAll())) {
      shell.value = 'workspace'
      lastError.value = workspace.lastError ?? normalizeError(new Error('保存工作区失败，已取消返回'))
      return false
    }
    try {
      await ipc.workspaceClose()
      tabs.resetWorkspace()
      workspace.reset()
      shell.value = 'launcher'
      await refresh()
      return true
    } catch (error) {
      shell.value = 'workspace'
      lastError.value = normalizeError(error)
      return false
    }
  }

  async function switchWorkspace(workspaceId: string) {
    const workspace = useWorkspaceStore()
    if (shell.value !== 'workspace' || workspace.workspaceId === workspaceId) return false
    const tabs = useTabsStore()
    busyWorkspaceId.value = workspaceId
    lastError.value = null
    shell.value = 'closing'
    const stateSaved = await workspace.flushWorkspaceState()
    if (!stateSaved || !(await workspace.saveDirtyEnvironments()) || !(await tabs.flushAll())) {
      shell.value = 'workspace'
      lastError.value = workspace.lastError ?? normalizeError(new Error('保存当前工作区失败，已取消切换'))
      busyWorkspaceId.value = null
      return false
    }
    try {
      await ipc.workspaceClose()
    } catch (error) {
      shell.value = 'workspace'
      lastError.value = normalizeError(error)
      busyWorkspaceId.value = null
      return false
    }

    // The old session is invalid before any state belonging to the next
    // workspace can be installed in the frontend.
    tabs.resetWorkspace()
    workspace.reset()
    shell.value = 'opening'
    try {
      const result = await ipc.workspaceOpen({ workspaceId })
      try {
        await acceptOpen(result)
      } catch (error) {
        await rollbackSession()
        throw error
      }
      return true
    } catch (error) {
      const appError = normalizeError(error)
      console.error('[switchWorkspace]', appError)
      tabs.resetWorkspace()
      workspace.reset()
      shell.value = 'launcher'
      busyWorkspaceId.value = null
      await refresh()
      // 与 openWorkspace 相同：错误要在 refresh() 之后再设置，避免被清空。
      lastError.value = appError
      return false
    }
  }

  async function forgetWorkspace(id: string) {
    try {
      await ipc.workspaceForget(id)
      await refresh()
    } catch (error) {
      lastError.value = normalizeError(error)
    }
  }

  async function relocateWorkspace(id: string, path: string) {
    try {
      await ipc.workspaceRelocate(id, path)
      await refresh()
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      return false
    }
  }

  async function revealWorkspace(id: string) {
    try {
      await ipc.workspaceReveal(id)
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      return false
    }
  }

  return {
    shell, workspaces, busyWorkspaceId, lastError, createDialogOpen, settings,
    initialize, refresh, openWorkspace, createWorkspace, closeWorkspace, switchWorkspace, forgetWorkspace, relocateWorkspace, revealWorkspace, saveSettings,
  }
})
