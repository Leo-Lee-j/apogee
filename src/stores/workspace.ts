import { computed, reactive, ref, shallowRef } from 'vue'
import { defineStore } from 'pinia'
import {
  inTauri,
  ipc,
  normalizeError,
  type AppError,
  type DebugCase,
  type DebugCaseType,
  type Environment,
  type EnvironmentVariable,
  type RequestSpec,
  type WorkspaceNode,
  type WorkspaceDescriptor,
  type WorkspaceOpenResult,
  type WorkspaceRequest,
  type WorkspaceSnapshot,
} from '@/ipc'
import { useTabsStore, type Tab } from '@/stores/tabs'
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
  /** 尚未落盘的环境 id；发送前统一 flush，避免 Rust 从磁盘读到旧值。 */
  const dirtyEnvironmentIds = reactive(new Set<string>())
  /** 已经按请求读取过的完整用例定义；工作区快照只携带轻量计数。 */
  const debugCasesByRequestId = shallowRef<Record<string, DebugCase[]>>({})
  /** 已展开用例子项的请求 id。使用集合副本更新，维持 shallowRef 的可预期刷新。 */
  const expandedDebugCaseRequestIds = ref(new Set<string>())
  const debugCaseLoadPromises = new Map<string, Promise<DebugCase[] | null>>()
  const debugCaseLoadVersions = new Map<string, number>()
  let debugCaseCacheVersion = 0
  const activeEnvironment = computed(
    () => environments.value.find((environment) => environment.id === activeEnvironmentId.value) ?? null,
  )
  const activeEnvironmentDirty = computed(
    () => activeEnvironmentId.value !== null && dirtyEnvironmentIds.has(activeEnvironmentId.value),
  )
  const loading = ref(false)
  const ready = ref(false)
  const lastError = ref<AppError | null>(null)
  let loadingPromise: Promise<void> | null = null
  const pendingStateSaves = new Set<Promise<boolean>>()
  let stateSaveQueue = Promise.resolve(true)

  const available = computed(() => inTauri)
  const collections = computed(() => (nodes.value ?? []).filter((node) => node.kind === 'collection'))

  function accept(snapshot: WorkspaceSnapshot) {
    invalidateDebugCaseCache()
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
    nodes.value = undefined
    environments.value = []
    activeEnvironmentId.value = null
    dirtyEnvironmentIds.clear()
    ready.value = false
    lastError.value = null
    descriptor.value = result.descriptor
    sessionId.value = result.sessionId
    accept(result.snapshot)
    activeEnvironmentId.value = result.state.activeEnvironmentId
      && environments.value.some((environment) => environment.id === result.state.activeEnvironmentId)
      ? result.state.activeEnvironmentId
      : null
  }

  function report(error: unknown): AppError {
    const appError = normalizeError(error)
    lastError.value = appError
    console.error('[workspace]', appError)
    return appError
  }

  /** 工作区快照来自磁盘，因而必须废弃内存中的用例定义与展开状态。 */
  function invalidateDebugCaseCache() {
    debugCaseCacheVersion += 1
    debugCaseLoadPromises.clear()
    debugCaseLoadVersions.clear()
    debugCasesByRequestId.value = {}
    expandedDebugCaseRequestIds.value = new Set()
  }

  function debugCasesForRequest(requestId: string): DebugCase[] {
    return debugCasesByRequestId.value[requestId] ?? []
  }

  function isDebugCasesExpanded(requestId: string) {
    return expandedDebugCaseRequestIds.value.has(requestId)
  }

  function setDebugCasesExpanded(requestId: string, expanded: boolean) {
    const next = new Set(expandedDebugCaseRequestIds.value)
    if (expanded) next.add(requestId)
    else next.delete(requestId)
    expandedDebugCaseRequestIds.value = next
  }

  /** 同步快照中的轻量计数，避免创建/删除后必须重读整个工作区才能出现或收起箭头。 */
  function updateDebugCaseCount(requestId: string, debugCaseCount: number) {
    const list = nodes.value
    if (!list) return
    const index = list.findIndex((node) => node.id === requestId && node.kind === 'request')
    if (index < 0 || list[index]?.debugCaseCount === debugCaseCount) return
    nodes.value = list.map((node, nodeIndex) => (
      nodeIndex === index ? { ...node, debugCaseCount } : node
    ))
  }

  /** 按需读取一个请求的用例定义；同一请求的并发展开会复用同一 IPC 调用。 */
  async function loadDebugCases(requestId: string, force = false): Promise<DebugCase[] | null> {
    if (!inTauri || !sessionId.value) return null
    if (!force && Object.prototype.hasOwnProperty.call(debugCasesByRequestId.value, requestId)) {
      return debugCasesByRequestId.value[requestId] ?? []
    }
    const pending = debugCaseLoadPromises.get(requestId)
    if (pending && !force) return pending

    const cacheVersion = debugCaseCacheVersion
    const loadVersion = (debugCaseLoadVersions.get(requestId) ?? 0) + 1
    debugCaseLoadVersions.set(requestId, loadVersion)
    const task = ipc.workspaceListDebugCases(requestId)
      .then((debugCases) => {
        // 快照重载或同请求的更晚读取完成后，旧请求不能把过期 YAML 再写回缓存。
        if (
          cacheVersion !== debugCaseCacheVersion
          || debugCaseLoadVersions.get(requestId) !== loadVersion
        ) return null
        debugCasesByRequestId.value = { ...debugCasesByRequestId.value, [requestId]: debugCases }
        updateDebugCaseCount(requestId, debugCases.length)
        return debugCases
      })
      .catch((error) => {
        report(error)
        return null
      })
    debugCaseLoadPromises.set(requestId, task)
    void task.finally(() => {
      if (debugCaseLoadPromises.get(requestId) === task) debugCaseLoadPromises.delete(requestId)
    })
    return task
  }

  /** 展开请求的用例子项时才拉取完整快照；折叠仅改本地可见行。 */
  async function toggleDebugCases(requestId: string): Promise<boolean> {
    if (isDebugCasesExpanded(requestId)) {
      setDebugCasesExpanded(requestId, false)
      return false
    }
    const debugCases = await loadDebugCases(requestId)
    if (!debugCases?.length) return false
    setDebugCasesExpanded(requestId, true)
    return true
  }

  async function createDebugCase(
    requestId: string,
    name: string,
    caseType: DebugCaseType,
    spec: RequestSpec,
  ): Promise<DebugCase | null> {
    if (!inTauri || !sessionId.value) return null
    try {
      const debugCase = await ipc.workspaceCreateDebugCase(requestId, name, caseType, spec)
      // 创建后强制刷新再展开：已有用例不会因只追加新返回项而从子树中消失。
      const debugCases = await loadDebugCases(requestId, true)
      if (debugCases) {
        setDebugCasesExpanded(requestId, true)
      } else {
        const count = nodes.value?.find((node) => node.id === requestId)?.debugCaseCount ?? 0
        updateDebugCaseCount(requestId, count + 1)
      }
      return debugCase
    } catch (error) {
      report(error)
      return null
    }
  }

  async function deleteDebugCase(requestId: string, caseId: string): Promise<boolean> {
    if (!inTauri || !sessionId.value) return false
    try {
      await ipc.workspaceDeleteDebugCase(requestId, caseId)
      const cached = debugCasesByRequestId.value[requestId]
      if (cached) {
        const next = cached.filter((debugCase) => debugCase.id !== caseId)
        debugCasesByRequestId.value = { ...debugCasesByRequestId.value, [requestId]: next }
        updateDebugCaseCount(requestId, next.length)
        if (next.length === 0) setDebugCasesExpanded(requestId, false)
      } else {
        const count = nodes.value?.find((node) => node.id === requestId)?.debugCaseCount ?? 0
        updateDebugCaseCount(requestId, Math.max(0, count - 1))
      }
      return true
    } catch (error) {
      report(error)
      return false
    }
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
    invalidateDebugCaseCache()
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
    void persistWorkspaceState()
  }

  function persistWorkspaceState(): Promise<boolean> {
    if (!inTauri || !sessionId.value) return Promise.resolve(true)
    const selectedId = activeEnvironmentId.value
    const task = stateSaveQueue.then(() =>
      ipc.workspaceSaveState(selectedId).then(
        () => true,
        (error) => {
          report(error)
          return false
        },
      ),
    )
    stateSaveQueue = task
    pendingStateSaves.add(task)
    void task.then(() => pendingStateSaves.delete(task))
    return task
  }

  async function flushWorkspaceState(): Promise<boolean> {
    let saved = true
    while (pendingStateSaves.size > 0) {
      const results = await Promise.all([...pendingStateSaves])
      saved = results.every(Boolean) && saved
    }
    return saved
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
      await persistWorkspaceState()
      return environment
    } catch (error) {
      report(error)
      return null
    }
  }

  async function duplicateEnvironment(environmentId = activeEnvironmentId.value): Promise<Environment | null> {
    const source = environments.value.find((environment) => environment.id === environmentId)
    if (!source) return null
    const created = await createEnvironment(`${source.name} 副本`)
    if (!created) return null
    updateEnvironment(created.id, {
      host: source.host,
      variables: source.variables.map((variable) => ({ ...variable })),
    })
    return (await saveEnvironment(created.id)) ? created : null
  }

  async function deleteEnvironment(environmentId = activeEnvironmentId.value): Promise<boolean> {
    if (!environmentId || !inTauri) return false
    try {
      const snapshot = await ipc.workspaceDeleteEnvironment(environmentId)
      dirtyEnvironmentIds.delete(environmentId)
      accept(snapshot)
      if (activeEnvironmentId.value === environmentId) {
        activeEnvironmentId.value = environments.value[0]?.id ?? null
      }
      await persistWorkspaceState()
      return true
    } catch (error) {
      report(error)
      return false
    }
  }

  async function openRequest(node: Pick<WorkspaceNode, 'id' | 'kind'>, preview = true): Promise<WorkspaceRequest | null> {
    if (!inTauri || node.kind !== 'request') return null
    try {
      const request = await ipc.workspaceReadRequest(node.id)
      const spec = request.spec
      const tabsStore = useTabsStore()
      // 已打开的接口重新激活时不抢焦点：每次都全选 URL 会让「切换/双击固定」很突兀。
      // 只有真正打开（之前没开）才聚焦 URL 栏（02 文档 §5.1）。
      const alreadyOpen = tabsStore.tabs.some((tab) => tab.nodeId === node.id)
      tabsStore.open({
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
      if (!alreadyOpen) useUiStore().focusUrl()
      return request
    } catch (error) {
      report(error)
      return null
    }
  }

  /**
   * 展开 nodeId 自身（若是文件夹/集合）及其祖先路径。
   *
   * 新建节点后调用，让新节点所在路径在侧边栏可见。扁平树的 `expanded` 在节点上，
   * 直接改对象再换数组引用就能触发侧边栏的 props watch 重算（03 文档 §4.2）。
   */
  function expandPath(nodeId: string) {
    const list = nodes.value
    if (!list) return
    const byId = new Map(list.map((node) => [node.id, node]))
    let changed = false
    let cursor = byId.get(nodeId)
    while (cursor) {
      if (cursor.isFolder && !cursor.expanded) {
        cursor.expanded = true
        changed = true
      }
      cursor = cursor.parentId ? byId.get(cursor.parentId) : undefined
    }
    if (changed) nodes.value = [...list]
  }

  /** 保存请求后同步侧边栏中的轻量节点字段，避免必须重新加载整个工作区。 */
  function updateRequestMethod(nodeId: string, method: string) {
    const list = nodes.value
    if (!list) return
    const index = list.findIndex((node) => node.id === nodeId && node.kind === 'request')
    if (index < 0 || list[index]?.method === method) return
    nodes.value = list.map((node, nodeIndex) => nodeIndex === index ? { ...node, method } : node)
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
    const ok = await applySnapshot(ipc.workspaceCreateFolder(parent, name))
    if (ok) expandPath(parent)
    return ok
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
      expandPath(parent)
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

  /**
   * 把草稿标签保存为集合内节点（「保存到」选择器流程）。
   *
   * 顺序必须是「先建节点 → 绑定 nodeId → 再落盘内容」：新建的节点 spec 是空的，
   * 绑定后再走 `tabs.save` 才会把当前编辑内容写进磁盘文件。失败统一走 report。
   */
  async function saveTabAs(tab: Tab, parentId: string, name: string): Promise<boolean> {
    if (!inTauri) return false
    try {
      const request = await ipc.workspaceCreateRequest(parentId, name)
      await load()
      expandPath(parentId)
      const tabs = useTabsStore()
      tabs.patch(tab.id, {
        nodeId: request.id,
        // 另存为后的标签已经是独立请求，不能继续把运行结果关联到原用例。
        sourceRequestId: undefined,
        debugCaseId: undefined,
        debugCaseEditor: undefined,
        title: request.name,
        newPanel: false,
        newRequestParentId: undefined,
      })
      const saved = await tabs.save(tab.id)
      if (saved.ok) updateRequestMethod(request.id, tab.method)
      if (!saved.ok && saved.error) report(saved.error)
      return saved.ok
    } catch (error) {
      report(error)
      return false
    }
  }

  async function renameNode(nodeId: string, name: string): Promise<boolean> {
    if (!inTauri) return false
    return applySnapshot(ipc.workspaceRenameNode(nodeId, name))
  }

  /** 复制集合、文件夹或接口；容器会连同其全部后代和调试用例一起复制。 */
  async function duplicateNode(node: WorkspaceNode): Promise<boolean> {
    if (!inTauri || !node.kind) return false
    try {
      const sourceNodes = nodes.value ?? []
      const byId = new Map(sourceNodes.map((item) => [item.id, item]))
      const isDescendant = (item: WorkspaceNode) => {
        let cursor = item
        while (cursor.parentId) {
          if (cursor.parentId === node.id) return true
          const parent = byId.get(cursor.parentId)
          if (!parent) return false
          cursor = parent
        }
        return false
      }
      const subtree = sourceNodes.filter((item) => item.id === node.id || isDescendant(item))
      const knownIds = new Set(sourceNodes.map((item) => item.id))
      const cloneParents = new Map<string, string>()
      const rootName = `${node.name} 副本`

      // 用例定义嵌入 Request YAML，不能只复制 RequestSpec。逐条创建会由后端生成
      // 新 case id 并绑定到新 request id，历史记录仍只属于原请求。
      const copyRequest = async (sourceId: string, parentId: string, name: string) => {
        const source = await ipc.workspaceReadRequest(sourceId)
        const copy = await ipc.workspaceCreateRequest(parentId, name)
        await ipc.workspaceSaveRequest(copy.id, name, source.spec)
        const cases = await ipc.workspaceListDebugCases(source.id)
        for (const debugCase of cases) {
          await ipc.workspaceCreateDebugCase(
            copy.id,
            debugCase.name,
            debugCase.caseType,
            debugCase.spec,
          )
        }
        return copy
      }

      if (node.kind === 'collection') {
        const snapshot = await ipc.workspaceCreateCollection(rootName)
        const root = snapshot.nodes.find((item) => !knownIds.has(item.id) && item.kind === 'collection')
        if (!root) throw new Error('无法识别复制后的集合')
        knownIds.add(root.id)
        cloneParents.set(node.id, root.id)
      } else if (node.kind === 'folder') {
        if (!node.parentId) throw new Error('文件夹缺少父级')
        const snapshot = await ipc.workspaceCreateFolder(node.parentId, rootName)
        const root = snapshot.nodes.find((item) => !knownIds.has(item.id) && item.parentId === node.parentId)
        if (!root) throw new Error('无法识别复制后的文件夹')
        knownIds.add(root.id)
        cloneParents.set(node.id, root.id)
      } else {
        if (!node.parentId) throw new Error('接口缺少父级')
        await copyRequest(node.id, node.parentId, rootName)
        await load()
        expandPath(node.parentId)
        return true
      }

      for (const item of subtree.slice(1)) {
        const parentId = item.parentId ? cloneParents.get(item.parentId) : undefined
        if (!parentId) throw new Error('无法确定复制目标的位置')
        if (item.kind === 'folder') {
          const snapshot = await ipc.workspaceCreateFolder(parentId, item.name)
          const copy = snapshot.nodes.find((candidate) => !knownIds.has(candidate.id) && candidate.parentId === parentId)
          if (!copy) throw new Error('无法识别复制后的文件夹')
          knownIds.add(copy.id)
          cloneParents.set(item.id, copy.id)
        } else if (item.kind === 'request') {
          await copyRequest(item.id, parentId, item.name)
        }
      }
      await load()
      const copiedRoot = cloneParents.get(node.id)
      if (copiedRoot) expandPath(copiedRoot)
      return true
    } catch (error) {
      report(error)
      return false
    }
  }

  /**
   * 将用例快照物化为独立调试标签。刻意不设置 nodeId：编辑此标签不能覆盖基础 Request。
   * sourceRequestId 只供发送时解析集合/文件夹继承，debugCaseId 用于关联运行结果。
   */
  function openDebugCase(debugCase: DebugCase): Tab {
    const spec = structuredClone(debugCase.spec)
    const tab = useTabsStore().open({
      title: `${debugCase.name} · 用例`,
      sourceRequestId: debugCase.requestId,
      debugCaseId: debugCase.id,
      debugCaseEditor: true,
      preview: false,
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
    return tab
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
    activeEnvironmentDirty,
    loading,
    ready,
    lastError,
    available,
    collections,
    debugCasesForRequest,
    isDebugCasesExpanded,
    loadDebugCases,
    toggleDebugCases,
    createDebugCase,
    deleteDebugCase,
    load,
    enter,
    reset,
    selectEnvironment,
    persistWorkspaceState,
    flushWorkspaceState,
    updateEnvironment,
    updateEnvironmentVariable,
    addEnvironmentVariable,
    removeEnvironmentVariable,
    saveEnvironment,
    saveDirtyEnvironments,
    createEnvironment,
    duplicateEnvironment,
    deleteEnvironment,
    openRequest,
    createCollection,
    createFolder,
    createRequest,
    saveTabAs,
    updateRequestMethod,
    renameNode,
    duplicateNode,
    openDebugCase,
    deleteNode,
  }
})
