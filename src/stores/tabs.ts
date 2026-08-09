import { defineStore } from 'pinia'
import { ref, shallowRef, computed, markRaw } from 'vue'
import type {
  AppError,
  AuthConfig,
  Body,
  KeyValue,
  RequestSettings,
  RequestSpec,
  ResponseSummary,
} from '@/ipc'
import { inTauri, ipc, normalizeError } from '@/ipc'
import { useUiStore } from '@/stores/ui'

/**
 * 标签状态。
 *
 * # 标签虚拟化（03 文档 §4.2）
 *
 * 「50 标签内存 < 400MB」靠三级状态实现：
 *   Active（1 个）    完整挂载：编辑器实例 + 响应视图
 *   Warm（最近 3 个）  保留 DOM，编辑器降级为只读快照
 *   Cold（其余）      只保留这里的轻量状态对象（目标 < 2KB/标签）
 *
 * 所以 Tab 对象里**不允许**放响应体、编辑器实例、DOM 引用。
 * 响应体在磁盘 spool，编辑器在 Active 时才创建。
 */

export type TabState = 'active' | 'warm' | 'cold'
export type TabKind = 'request' | 'settings' | 'workspace-stats' | 'container-config'

export interface Tab {
  id: string
  /** 决定标签下方挂载哪一种工作区内容；后续可继续扩展 history 等通用页面。 */
  kind: TabKind
  title: string
  method: string
  url: string
  /** 执行请求所需的可编辑字段。数组字段通过 store 整体替换，保持 shallowRef 约束。 */
  params: KeyValue[]
  pathParams: KeyValue[]
  headers: KeyValue[]
  body: Body
  auth: AuthConfig
  settings: RequestSettings
  /** 最近一次请求错误；成功发送后清除。 */
  error: AppError | null
  /**
   * 这个标签对应的集合树节点 id。从侧边栏打开时带上，草稿标签（新建请求）没有。
   *
   * 存 id 而不是从 `url` 反推：URL 是用户可编辑的，改一个字符就找不回节点了。
   * 侧边栏的「定位当前接口」和「高亮当前打开的行」都只认这个字段。
   */
  nodeId?: string
  /** 集合或文件夹配置页对应的容器节点。 */
  containerNodeId?: string
  /**
   * 新建请求标签处于选项面板状态时为 true。状态归属标签，而不是全局 UI，
   * 这样切换回其他接口时不会把它们的内容替换掉。
   */
  newPanel?: boolean
  /** 从侧栏发起新建请求时继承的集合/文件夹目标。 */
  newRequestParentId?: string
  /** 有未保存改动，标签上显示 `•` */
  dirty: boolean
  /** 临时标签（单击预览）：标题斜体，被下一次单击复用（02 文档 §4.5） */
  preview: boolean
  /** 只放摘要，不放 body */
  response: ResponseSummary | null
}

const WARM_LIMIT = 3

let seq = 0
function nextId() {
  return `tab-${++seq}`
}

/** 从 URL 路径中识别 :id / {id} 占位符，不把 {{variable}} 当成路径参数。 */
function pathParamNames(url: string): string[] {
  const path = url.split(/[?#]/, 1)[0] ?? ''
  const names: string[] = []
  const seen = new Set<string>()
  const pattern = /\{\{([A-Za-z_][A-Za-z0-9_-]*)\}\}|:([A-Za-z_][A-Za-z0-9_-]*)|(?<!\{)\{([A-Za-z_][A-Za-z0-9_-]*)\}(?!\})/g
  for (const match of path.matchAll(pattern)) {
    const name = match[1] ?? match[2] ?? match[3]
    if (name && !seen.has(name)) {
      seen.add(name)
      names.push(name)
    }
  }
  return names
}

export const useTabsStore = defineStore('tabs', () => {
  /**
   * 用 shallowRef 而不是 ref：标签数组可能到几十项，深度响应式会为每个嵌套字段
   * 建 Proxy。这里我们只需要「数组整体变了」的粒度，字段级更新走整体替换。
   * 03 文档 §4.2 里同样的理由适用于集合树（上万节点时差距是数量级的）。
   */
  const tabs = shallowRef<Tab[]>([])
  const activeId = ref<string | null>(null)
  /** 最近访问顺序，队首最新。用来判定 warm/cold。 */
  const mru = ref<string[]>([])
  const persistTimers = new Map<string, ReturnType<typeof setTimeout>>()
  const PERSISTED_FIELDS = new Set([
    'title', 'method', 'url', 'params', 'pathParams', 'headers', 'body', 'auth', 'settings',
  ])

  function requestSpec(tab: Tab): RequestSpec {
    return {
      method: tab.method,
      url: tab.url,
      params: tab.params,
      pathParams: tab.pathParams,
      headers: tab.headers,
      body: tab.body,
      auth: tab.auth,
      settings: tab.settings,
    }
  }

  /**
   * 落盘一次。成功返回 null；失败返回归一化后的 AppError，调用方负责把它
   * 展示给用户（不要只写控制台）。草稿（无 nodeId）或非 Tauri 短路为成功，
   * 因为没有可写盘的对象。
   */
  async function persist(tab: Tab): Promise<AppError | null> {
    if (!inTauri || !tab.nodeId) return null
    try {
      await ipc.workspaceSaveRequest(tab.nodeId, tab.title, requestSpec(tab))
      return null
    } catch (error) {
      console.error('[workspace] 保存请求失败', error)
      return normalizeError(error)
    }
  }

  function schedulePersist(tab: Tab) {
    if (!inTauri || !tab.nodeId) return
    const oldTimer = persistTimers.get(tab.nodeId)
    if (oldTimer !== undefined) clearTimeout(oldTimer)
    persistTimers.set(
      tab.nodeId,
      setTimeout(() => {
        persistTimers.delete(tab.nodeId!)
        void persist(tab)
      }, 300),
    )
  }

  function markClean(id: string) {
    const index = tabs.value.findIndex((item) => item.id === id)
    if (index < 0) return
    const current = tabs.value[index]
    if (!current || !current.dirty) return
    const next = markRaw({ ...current, dirty: false })
    tabs.value = tabs.value.map((item, itemIndex) => (itemIndex === index ? next : item))
  }

  const active = computed(() => tabs.value.find((t) => t.id === activeId.value) ?? null)

  function stateOf(id: string): TabState {
    if (id === activeId.value) return 'active'
    const idx = mru.value.indexOf(id)
    return idx >= 0 && idx < WARM_LIMIT ? 'warm' : 'cold'
  }

  /** 当前需要完整挂载的标签 id 集合。响应视图据此决定挂不挂载重组件。 */
  const mountedIds = computed(() => {
    const ids = new Set<string>()
    if (activeId.value) ids.add(activeId.value)
    for (const id of mru.value.slice(0, WARM_LIMIT)) ids.add(id)
    return ids
  })

  function touch(id: string) {
    const rest = mru.value.filter((x) => x !== id)
    mru.value = [id, ...rest]
  }

  function open(init?: Partial<Tab>): Tab {
    // 已打开的接口直接激活，不重复开标签：nodeId 是稳定身份（URL 可被用户改写）。
    // 没有这一条，单击一个已固定打开的接口时，预览复用会把另一个预览标签改写成它，
    // 同一个接口就出现两个标签（02 文档 §4.5 的预期是「已打开则激活」）。
    if (init?.nodeId) {
      const existing = tabs.value.find((tab) => tab.nodeId === init.nodeId)
      if (existing) {
        // 双击固定：预览标签被显式要求以正式标签打开（preview:false）时提升为正式。
        if (existing.preview && init.preview === false) {
          patch(existing.id, { preview: false })
        }
        activate(existing.id)
        return existing
      }
    }
    // 单击预览：已有临时标签时复用它，避免浏览集合时开出 30 个标签（02 文档 §4.5）
    if (init?.preview) {
      const existing = tabs.value.find((t) => t.preview)
      if (existing) {
        // 不能原地改 markRaw 对象：active 是 computed，返回同一个对象引用时 Vue 认为
        // 值没变，依赖它的组件不重渲染，内容区会停在旧请求上。建新对象替换。
        // 同时重置不应跨请求保留的脏/错误/响应状态（否则切到 B 还带着 A 的草稿标记）。
        const next: Tab = markRaw({
          ...existing,
          nodeId: undefined, // 先清旧预览的 nodeId，再让 init 决定，避免高亮错行
          ...init,
          id: existing.id,
          preview: true,
          dirty: false,
          error: null,
          response: null,
          newPanel: false,
          newRequestParentId: undefined,
        })
        tabs.value = tabs.value.map((tab) => (tab.id === existing.id ? next : tab))
        activate(existing.id)
        return next
      }
    }
    const tab: Tab = markRaw({
      id: nextId(),
      kind: 'request',
      title: '新建请求',
      method: 'GET',
      url: '',
      params: [],
      pathParams: [],
      headers: [],
      body: { type: 'none' },
      auth: { type: 'inherit' },
      settings: {},
      error: null,
      dirty: false,
      preview: false,
      response: null,
      newPanel: false,
      ...init,
    })
    tabs.value = [...tabs.value, tab]
    activate(tab.id)
    return tab
  }

  /**
   * 新建一个独立的空白请求标签：不属于任何集合，打开后立即聚焦 URL 栏。
   * Ctrl+T 走这里；标签栏“+”和侧边栏“新建请求”走 openNewPanel()，先显示选项面板。
   */
  function openQuick(): Tab {
    const tab = open({ title: '新建请求' })
    useUiStore().focusUrl()
    return tab
  }

  /** 创建一个独立的“新建请求”标签，内容区随后显示选项面板。 */
  function openNewPanel(parentId?: string): Tab {
    return open({
      title: '新建请求',
      newPanel: true,
      newRequestParentId: parentId,
    })
  }

  /** 设置是单实例工作区页；重复点击入口只激活已有标签。 */
  function openSettings(): Tab {
    useUiStore().spikeOpen = false
    const existing = tabs.value.find((tab) => tab.kind === 'settings')
    if (existing) {
      activate(existing.id)
      return existing
    }
    return open({ kind: 'settings', title: '设置' })
  }

  /** 工作区统计是单实例页面；重复打开时仅激活已有标签。 */
  function openWorkspaceStats(): Tab {
    useUiStore().spikeOpen = false
    const existing = tabs.value.find((tab) => tab.kind === 'workspace-stats')
    if (existing) {
      activate(existing.id)
      return existing
    }
    return open({ kind: 'workspace-stats', title: '工作区统计' })
  }

  function openContainerConfig(node: { id: string; name: string }): Tab {
    useUiStore().spikeOpen = false
    const existing = tabs.value.find((tab) => tab.kind === 'container-config' && tab.containerNodeId === node.id)
    if (existing) {
      activate(existing.id)
      return existing
    }
    return open({ kind: 'container-config', title: `配置 · ${node.name}`, containerNodeId: node.id, preview: false })
  }

  /** 将当前新建面板标签转换为空白请求编辑标签。 */
  function resolveNewPanel() {
    const tab = active.value
    if (!tab?.newPanel) return
    patch(tab.id, { newPanel: false, newRequestParentId: undefined })
    useUiStore().focusUrl()
  }

  function activate(id: string) {
    activeId.value = id
    touch(id)
  }

  function close(id: string) {
    const idx = tabs.value.findIndex((t) => t.id === id)
    const current = tabs.value[idx]
    if (current?.nodeId) {
      const timer = persistTimers.get(current.nodeId)
      if (timer !== undefined) clearTimeout(timer)
      persistTimers.delete(current.nodeId)
      void persist(current)
    }
    tabs.value = tabs.value.filter((t) => t.id !== id)
    mru.value = mru.value.filter((x) => x !== id)
    if (activeId.value === id) {
      const next = tabs.value[Math.min(idx, tabs.value.length - 1)]
      activeId.value = next?.id ?? null
      if (next) touch(next.id)
    }
  }

  function patch(id: string, delta: Partial<Tab>) {
    const index = tabs.value.findIndex((x) => x.id === id)
    if (index < 0) return

    // 不要原地修改 markRaw 的 Tab。active 是一个 computed，如果更新后仍返回
    // 同一个对象引用，Vue 可能认为值没有变化，依赖它的 editorSections 直到
    // 切换标签才会重新计算。创建新对象同时满足 shallowRef 的整体替换约束。
    const current = tabs.value[index]
    const next = markRaw({ ...current, ...delta })
    tabs.value = tabs.value.map((tab, tabIndex) => (tabIndex === index ? next : tab))

    if (Object.keys(delta).some((key) => PERSISTED_FIELDS.has(key))) schedulePersist(next)
  }

  /**
   * 立即保存当前标签并清除未保存标记。
   *
   * 草稿（无 nodeId）返回 `{ ok: false, error: null }` 且**不**清脏点——
   * 它没有可写盘的对象，保存要先走「保存到」选择器建节点（`workspace.saveTabAs`）。
   * 这里不再谎清脏标记，避免「脏点消失但内容没落盘」的假象。
   */
  async function save(id: string): Promise<{ ok: boolean; error: AppError | null }> {
    const tab = tabs.value.find((item) => item.id === id)
    if (!tab || !tab.nodeId) return { ok: false, error: null }
    const timer = persistTimers.get(tab.nodeId)
    if (timer !== undefined) {
      clearTimeout(timer)
      persistTimers.delete(tab.nodeId)
    }
    const error = await persist(tab)
    if (error) return { ok: false, error }
    markClean(id)
    return { ok: true, error: null }
  }

  async function flushAll(): Promise<boolean> {
    for (const timer of persistTimers.values()) clearTimeout(timer)
    persistTimers.clear()
    for (const tab of tabs.value) {
      if (tab.nodeId && (await persist(tab))) return false
    }
    return true
  }

  function resetWorkspace() {
    for (const timer of persistTimers.values()) clearTimeout(timer)
    persistTimers.clear()
    tabs.value = []
    activeId.value = null
    mru.value = []
  }

  /** URL 改变时同步路径参数行，保留用户已经填写的值。 */
  function syncPathParams(id: string, url: string) {
    const t = tabs.value.find((x) => x.id === id)
    if (!t) return

    const oldRows = t.pathParams ?? []
    const nextRows = pathParamNames(url).map((name) => {
      const existing = oldRows.find((row) => row.key === name)
      return existing ?? {
        enabled: true,
        key: name,
        value: '',
        description: '路径参数',
        kind: 'text' as const,
      }
    })
    const unchanged =
      nextRows.length === oldRows.length &&
      nextRows.every((row, index) => {
        const old = oldRows[index]
        return old?.key === row.key && old?.value === row.value && old?.enabled === row.enabled
      })
    if (!unchanged) patch(id, { pathParams: nextRows })
  }

  return {
    tabs,
    activeId,
    active,
    mru,
    mountedIds,
    stateOf,
    open,
    openQuick,
    openNewPanel,
    openSettings,
    openWorkspaceStats,
    openContainerConfig,
    resolveNewPanel,
    activate,
    close,
    patch,
    save,
    flushAll,
    resetWorkspace,
    syncPathParams,
  }
})
