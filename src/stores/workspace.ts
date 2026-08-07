import { computed, ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import {
  inTauri,
  ipc,
  normalizeError,
  type AppError,
  type Environment,
  type EnvironmentVariable,
  type WorkspaceNode,
  type WorkspaceDescriptor,
  type WorkspaceOpenResult,
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
  const descriptor = shallowRef<WorkspaceDescriptor | null>(null)
  const sessionId = ref<string | null>(null)
  const environments = shallowRef<Environment[]>([])
  const activeEnvironmentId = ref<string | null>(null)
  const activeEnvironment = computed(
    () => environments.value.find((environment) => environment.id === activeEnvironmentId.value) ?? null,
  )
  /** 尚未落盘的环境 id；发送前统一 flush，避免 Rust 从磁盘读到旧值。 */
  const dirtyEnvironmentIds = new Set<string>()
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
    const incomingEnvironments = snapshot.environments ?? []
    const localEnvironments = new Map(environments.value.map((environment) => [environment.id, environment]))
    environments.value = incomingEnvironments.map((environment) =>
      dirtyEnvironmentIds.has(environment.id)
        ? localEnvironments.get(environment.id) ?? environment
        : environment,
    )
    for (const id of dirtyEnvironmentIds) {
      if (!environments.value.some((environment) => environment.id === id)) dirtyEnvironmentIds.delete(id)
    }
    if (!environments.value.some((environment) => environment.id === activeEnvironmentId.value)) {
      activeEnvironmentId.value = environments.value[0]?.id ?? null
    }
    ready.value = true
    lastError.value = null
  }

  function acceptOpen(result: WorkspaceOpenResult) {
    descriptor.value = result.descriptor
    sessionId.value = result.sessionId
    accept(result.snapshot)
  }

  function report(error: unknown): AppError {
    const appError = normalizeError(error)
    lastError.value = appError
    console.error('[workspace]', appError)
    return appError
  }

  async function load() {
    if (!inTauri || !sessionId.value) return
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

  async function enter(result: WorkspaceOpenResult) {
    loading.value = true
    try {
      acceptOpen(result)
    } finally {
      loading.value = false
    }
  }

  function reset() {
    nodes.value = undefined
    workspaceId.value = null
    workspaceName.value = ''
    descriptor.value = null
    sessionId.value = null
    environments.value = []
    activeEnvironmentId.value = null
    dirtyEnvironmentIds.clear()
    ready.value = false
    loading.value = false
    lastError.value = null
    loadingPromise = null
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

  function selectEnvironment(id: string | null) {
    activeEnvironmentId.value = id && environments.value.some((environment) => environment.id === id)
      ? id
      : null
  }

  function updateEnvironment(id: string, delta: Partial<Environment>) {
    const current = environments.value.find((environment) => environment.id === id)
    if (!current) return
    environments.value = environments.value.map((environment) =>
      environment.id === id ? { ...environment, ...delta } : environment,
    )
    dirtyEnvironmentIds.add(id)
  }

  function updateEnvironmentVariable(
    environmentId: string,
    index: number,
    delta: Partial<EnvironmentVariable>,
  ) {
    const environment = environments.value.find((item) => item.id === environmentId)
    if (!environment) return
    updateEnvironment(environmentId, {
      variables: environment.variables.map((variable, variableIndex) =>
        variableIndex === index ? { ...variable, ...delta } : variable,
      ),
    })
  }

  function addEnvironmentVariable(environmentId: string) {
    const environment = environments.value.find((item) => item.id === environmentId)
    if (!environment) return
    updateEnvironment(environmentId, {
      variables: [
        ...environment.variables,
        { key: '', value: '', enabled: true },
      ],
    })
  }

  function removeEnvironmentVariable(environmentId: string, index: number) {
    const environment = environments.value.find((item) => item.id === environmentId)
    if (!environment) return
    updateEnvironment(environmentId, {
      variables: environment.variables.filter((_, variableIndex) => variableIndex !== index),
    })
  }

  async function saveEnvironment(environmentId = activeEnvironmentId.value): Promise<boolean> {
    const environment = environments.value.find((item) => item.id === environmentId)
    if (!environment || !inTauri || !dirtyEnvironmentIds.has(environment.id)) return true
    try {
      const saved = await ipc.workspaceSaveEnvironment(environment)
      environments.value = environments.value.map((item) =>
        item.id === saved.id ? saved : item,
      )
      dirtyEnvironmentIds.delete(saved.id)
      return true
    } catch (error) {
      report(error)
      return false
    }
  }

  async function saveDirtyEnvironments(): Promise<boolean> {
    for (const id of [...dirtyEnvironmentIds]) {
      if (!(await saveEnvironment(id))) return false
    }
    return true
  }

  async function createEnvironment(name = '新环境'): Promise<Environment | null> {
    if (!inTauri) return null
    try {
      const environment = await ipc.workspaceCreateEnvironment(name)
      environments.value = [...environments.value, environment]
      activeEnvironmentId.value = environment.id
      return environment
    } catch (error) {
      report(error)
      return null
    }
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
    descriptor,
    sessionId,
    environments,
    activeEnvironmentId,
    activeEnvironment,
    loading,
    ready,
    lastError,
    available,
    collections,
    load,
    enter,
    reset,
    selectEnvironment,
    updateEnvironment,
    updateEnvironmentVariable,
    addEnvironmentVariable,
    removeEnvironmentVariable,
    saveEnvironment,
    saveDirtyEnvironments,
    createEnvironment,
    openRequest,
    createCollection,
    createFolder,
    createRequest,
    renameNode,
    deleteNode,
  }
})
