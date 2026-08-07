import { ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import { ipc, normalizeError, type AppError, type AppSettings, type WorkspaceDescriptor, type WorkspaceOpenResult } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'

export type AppShellKind = 'launcher' | 'opening' | 'workspace' | 'closing'

export const useAppStore = defineStore('app', () => {
  const shell = ref<AppShellKind>('launcher')
  const workspaces = shallowRef<WorkspaceDescriptor[]>([])
  const busyWorkspaceId = ref<string | null>(null)
  const lastError = ref<AppError | null>(null)
  const createDialogOpen = ref(false)
  const settings = ref<AppSettings>({ defaultWorkspaceRoot: '', openLastWorkspaceOnStart: false })
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

  async function openWorkspace(input: { workspaceId?: string; path?: string }, busyId = input.workspaceId ?? 'path') {
    shell.value = 'opening'
    busyWorkspaceId.value = busyId
    lastError.value = null
    try {
      await acceptOpen(await ipc.workspaceOpen(input))
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      shell.value = 'launcher'
      busyWorkspaceId.value = null
      await refresh()
      return false
    }
  }

  async function createWorkspace(path: string | undefined, name: string) {
    shell.value = 'opening'
    busyWorkspaceId.value = 'create'
    lastError.value = null
    try {
      await acceptOpen(await ipc.workspaceCreate(path, name))
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      shell.value = 'launcher'
      busyWorkspaceId.value = null
      return false
    }
  }

  async function saveSettings(next: AppSettings) {
    try {
      settings.value = await ipc.appSettingsSave(next)
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
    await workspace.flushWorkspaceState()
    if (!(await workspace.saveDirtyEnvironments()) || !(await tabs.flushAll())) {
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
    await workspace.flushWorkspaceState()
    if (!(await workspace.saveDirtyEnvironments()) || !(await tabs.flushAll())) {
      lastError.value = workspace.lastError ?? normalizeError(new Error('保存当前工作区失败，已取消切换'))
      busyWorkspaceId.value = null
      return false
    }
    try {
      await acceptOpen(await ipc.workspaceOpen({ workspaceId }))
      return true
    } catch (error) {
      lastError.value = normalizeError(error)
      busyWorkspaceId.value = null
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

  return {
    shell, workspaces, busyWorkspaceId, lastError, createDialogOpen, settings,
    initialize, refresh, openWorkspace, createWorkspace, closeWorkspace, switchWorkspace, forgetWorkspace, saveSettings,
  }
})
