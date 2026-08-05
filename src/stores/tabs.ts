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
import { inTauri, ipc } from '@/ipc'
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

export interface Tab {
  id: string
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
  const pattern = /:([A-Za-z_][A-Za-z0-9_-]*)|(?<!\{)\{([A-Za-z_][A-Za-z0-9_-]*)\}(?!\})/g
  for (const match of path.matchAll(pattern)) {
    const name = match[1] ?? match[2]
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

  async function persist(tab: Tab): Promise<boolean> {
    if (!inTauri || !tab.nodeId) return true
    try {
      await ipc.workspaceSaveRequest(tab.nodeId, tab.title, requestSpec(tab))
      return true
    } catch (error) {
      console.error('[workspace] 保存请求失败', error)
      return false
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
    // 单击预览：已有临时标签时复用它，避免浏览集合时开出 30 个标签（02 文档 §4.5）
    if (init?.preview) {
      const existing = tabs.value.find((t) => t.preview)
      if (existing) {
        // `nodeId` 先清掉再合并：`init` 没带节点 id 时不能留着上一次预览的那个，
        // 否则侧边栏会继续高亮一个与当前内容无关的行。
        Object.assign(existing, { nodeId: undefined }, init, { id: existing.id })
        tabs.value = [...tabs.value]
        activate(existing.id)
        return existing
      }
    }
    const tab: Tab = markRaw({
      id: nextId(),
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
      ...init,
    })
    tabs.value = [...tabs.value, tab]
    activate(tab.id)
    return tab
  }

  /**
   * 新建一个「快捷请求」：不属于任何集合的临时请求，发完即走。
   *
   * `Ctrl+T` 和新建面板里的第一张卡片都走这里，所以「什么算快捷请求」
   * （标题、不落盘、开完就聚焦 URL 栏）只有一处定义。两边各写一遍的话，
   * 迟早有一边忘了聚焦。
   *
   * 这里用 useUiStore 是单向依赖（ui 不认识 tabs），不构成环。
   */
  function openQuick(): Tab {
    const tab = open({ title: '快捷请求' })
    useUiStore().focusUrl()
    return tab
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

  /** 立即保存当前标签，并清除未保存标记。 */
  async function save(id: string): Promise<boolean> {
    const tab = tabs.value.find((item) => item.id === id)
    if (!tab) return false
    if (tab.nodeId) {
      const timer = persistTimers.get(tab.nodeId)
      if (timer !== undefined) {
        clearTimeout(timer)
        persistTimers.delete(tab.nodeId)
      }
      if (!(await persist(tab))) return false
    }
    markClean(id)
    return true
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
    activate,
    close,
    patch,
    save,
    syncPathParams,
  }
})
