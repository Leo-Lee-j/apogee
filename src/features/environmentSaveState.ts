import { computed, ref } from 'vue'
import type { useWorkspaceStore } from '@/stores/workspace'

type Workspace = Pick<ReturnType<typeof useWorkspaceStore>, 'workspaceId' | 'sessionId' |
  'activeEnvironmentDirty' | 'globalVariablesDirty' | 'saveEnvironment' | 'saveGlobalVariables'>

export function useEnvironmentSaveState(workspace: Workspace, valid: () => boolean) {
  const saving = ref(false)
  const failed = ref(false)
  const dirty = computed(() => workspace.activeEnvironmentDirty || workspace.globalVariablesDirty)
  const status = computed(() => saving.value ? '保存中…' : failed.value && dirty.value ? '保存失败，有未保存修改' : dirty.value ? '有未保存修改' : '已保存')
  let pending: Promise<boolean> | undefined

  function save() {
    if (pending) return pending
    if (!valid()) return Promise.resolve(false)
    const workspaceId = workspace.workspaceId
    const sessionId = workspace.sessionId
    const current = () => workspace.workspaceId === workspaceId && workspace.sessionId === sessionId
    saving.value = true
    failed.value = false
    pending = (async () => {
      try {
        do {
          if (!current() || !valid()) return false
          if (workspace.activeEnvironmentDirty && !(await workspace.saveEnvironment())) return false
          if (!current()) return false
          if (workspace.globalVariablesDirty && !(await workspace.saveGlobalVariables())) return false
          if (!current()) return false
        } while (dirty.value)
        return true
      } catch {
        return false
      }
    })().then((ok) => { failed.value = !ok && current(); return ok })
      .finally(() => { saving.value = false; pending = undefined })
    return pending
  }
  return { saving, dirty, status, save }
}
