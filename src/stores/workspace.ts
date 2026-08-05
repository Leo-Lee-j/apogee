import { computed, ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import {
  inTauri,
  ipc,
  normalizeError,
  type AppError,
  type WorkspaceNode,
  type WorkspaceRequest,
  type WorkspaceSnapshot,
} from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'

/**
 * 工作区状态只保存 Rust 返回的扁平节点，不在 WebView 中重建递归树。
 * 请求正文仍按需通过 workspaceReadRequest 读取，避免启动时把所有请求内容搬进 JS。
 */
export const useWorkspaceStore = defineStore('workspace', () => {
  const nodes = shallowRef<WorkspaceNode[] | undefined>(undefined)
  const workspaceId = ref<string | null>(null)
  const workspaceName = ref('')
  const loading = ref(false)
  const ready = ref(false)
  const lastError = ref<AppError | null>(null)
  let loadingPromise: Promise<void> | null = null

  const available = computed(() => inTauri)
  const collections = computed(() => (nodes.value ?? []).filter((node) => node.kind === 'collection'))

  function accept(snapshot: WorkspaceSnapshot) {
    workspaceId.value = snapshot.id
    workspaceName.value = snapshot.name
    nodes.value = snapshot.nodes
    ready.value = true
    lastError.value = null
  }

  function report(error: unknown): AppError {
    const appError = normalizeError(error)
    lastError.value = appError
    console.error('[workspace]', appError)
    return appError
  }

  async function load() {
    if (!inTauri) return
    if (loadingPromise) return loadingPromise

    loading.value = true
    loadingPromise = ipc
      .workspaceLoad()
      .then(accept)
      .catch((error) => {
        report(error)
      })
      .finally(() => {
        loading.value = false
        loadingPromise = null
      })
    return loadingPromise
  }

  async function applySnapshot(task: Promise<WorkspaceSnapshot>): Promise<boolean> {
    try {
      accept(await task)
      return true
    } catch (error) {
      report(error)
      return false
    }
  }

  async function parentForNewNode(parentId?: string): Promise<string | undefined> {
    if (parentId) return parentId
    if (!ready.value) await load()
    return collections.value[0]?.id
  }

  async function openRequest(node: WorkspaceNode, preview = true): Promise<WorkspaceRequest | null> {
    if (!inTauri || node.kind !== 'request') return null
    try {
      const request = await ipc.workspaceReadRequest(node.id)
      const spec = request.spec
      useTabsStore().open({
        title: request.name,
        nodeId: request.id,
        preview,
        method: spec.method,
        url: spec.url,
        params: spec.params ?? [],
        pathParams: spec.pathParams ?? [],
        headers: spec.headers ?? [],
        body: spec.body ?? { type: 'none' },
        auth: spec.auth ?? { type: 'inherit' },
        settings: spec.settings ?? {},
      })
      useUiStore().focusUrl()
      return request
    } catch (error) {
      report(error)
      return null
    }
  }

  async function createCollection(name = '新建集合'): Promise<boolean> {
    if (!inTauri) return false
    return applySnapshot(ipc.workspaceCreateCollection(name))
  }

  async function createFolder(parentId?: string, name = '新建文件夹'): Promise<boolean> {
    if (!inTauri) return false
    const parent = await parentForNewNode(parentId)
    if (!parent) {
      report(new Error('请先创建一个集合'))
      return false
    }
    return applySnapshot(ipc.workspaceCreateFolder(parent, name))
  }

  async function createRequest(parentId?: string, name = '新建请求'): Promise<WorkspaceRequest | null> {
    if (!inTauri) return null
    const parent = await parentForNewNode(parentId)
    if (!parent) {
      report(new Error('请先创建一个集合'))
      return null
    }

    try {
      const request = await ipc.workspaceCreateRequest(parent, name)
      await load()
      useTabsStore().open({
        title: request.name,
        nodeId: request.id,
        preview: false,
        method: request.spec.method,
        url: request.spec.url,
        params: request.spec.params ?? [],
        pathParams: request.spec.pathParams ?? [],
        headers: request.spec.headers ?? [],
        body: request.spec.body ?? { type: 'none' },
        auth: request.spec.auth ?? { type: 'inherit' },
        settings: request.spec.settings ?? {},
      })
      useUiStore().focusUrl()
      return request
    } catch (error) {
      report(error)
      return null
    }
  }

  async function renameNode(nodeId: string, name: string): Promise<boolean> {
    if (!inTauri) return false
    return applySnapshot(ipc.workspaceRenameNode(nodeId, name))
  }

  async function deleteNode(nodeId: string): Promise<boolean> {
    if (!inTauri) return false
    return applySnapshot(ipc.workspaceDeleteNode(nodeId))
  }

  return {
    nodes,
    workspaceId,
    workspaceName,
    loading,
    ready,
    lastError,
    available,
    collections,
    load,
    openRequest,
    createCollection,
    createFolder,
    createRequest,
    renameNode,
    deleteNode,
  }
})
