<script setup lang="ts">
import { h, ref, computed, shallowRef, watch, onMounted, onBeforeUnmount, nextTick } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import {
  AppAlert,
  AppButton,
  AppDropdown,
  AppIcon,
  AppInput,
  AppModal,
  AppPopconfirm,
  AppSpace,
} from '@/components/ui'
import { IconAdd, IconApi, IconCopy, IconDelete, IconDownload, IconEdit, IconLocate, IconMore, IconSort, IconFolder, IconFolderOpen, IconStats } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { toast } from '@/features/feedback'
import { ipc, type KeyValue, type RequestSpec, type WorkspaceNode } from '@/ipc'
import {
  ROW_H,
  OVERSCAN,
  synthTree,
  visibleNodes,
  sortVisible,
  indexOfNode,
  ancestorIndexes,
  type FlatNode,
  type SortMode,
} from './tree'

/**
 * 侧边栏。动态 import 挂载，不进首屏 chunk（03 文档 §4.1）。
 *
 * 集合树用自研虚拟滚动，不用 `n-tree`：这是三个热路径组件之一
 * （02 文档 §7.5 第 4 条）。1000 个请求首次展开要 < 100ms（PRD §5.1），
 * 通用组件库的树满足不了这个要求。
 *
 * 数据形状、虚拟滚动参数与 `visible` 过滤都在 `./tree.ts`，因为 **Spike S3 要量测
 * 的就是这个组件本身**（`src/spike/S3Panel.vue`），不能让量测台抄一份参数出来测副本。
 */

const props = defineProps<{
  /**
   * 外部注入的节点数组。不传则用内置合成数据。
   *
   * 正式实现里这里接 Rust 侧 `list_tree` 的返回；Spike S3 用它按 1000 / 5000 / 20000
   * 三档灌入压力数据。**不是**为了测试而留的后门 —— 数据源本来就该由外部提供。
   */
  nodes?: FlatNode[]
}>()

/**
 * 用 `shallowRef` 而不是 `ref`：这个数组在 S3 的 20000 节点档位下会产生
 * 20000 个 Proxy（03 文档 §4.2 明确禁止对大数组做深度响应式）。
 * 代价是改动节点内字段后必须手动换引用触发更新，见 `toggle`。
 */
const all = shallowRef<FlatNode[]>(props.nodes ?? synthTree({ folders: 40, perFolder: 25, expanded: 2 }))
watch(
  () => props.nodes,
  (next) => {
    if (next) all.value = next
  },
)

const filter = ref('')
const scrollTop = ref(0)
const viewportH = ref(400)
const scroller = ref<HTMLElement | null>(null)
const tabs = useTabsStore()
const workspace = useWorkspaceStore()
const selectedId = ref<string | null>(null)
type ActionKind = 'collection' | 'folder' | 'rename'
const actionKind = ref<ActionKind | null>(null)
const actionName = ref('')
/** 「新建文件夹」的保存位置（集合/文件夹 id），在弹窗里手动选择。 */
const actionParentId = ref<string | null>(null)
const actionError = ref('')
const deleteError = ref('')
const deleteTarget = ref<FlatNode | null>(null)
const actionSaving = ref(false)
const actionOpen = computed({
  get: () => actionKind.value !== null,
  set: (show: boolean) => {
    if (!show) closeAction()
  },
})

const selectedNode = computed(() => all.value.find((node) => node.id === selectedId.value) ?? null)
const actionModalTitle = computed(() => {
  if (actionKind.value === 'rename') return '重命名'
  if (actionKind.value === 'collection') return '新建集合'
  if (actionKind.value === 'folder') return '新建文件夹'
  return '新建'
})
const actionModalHint = computed(() => {
  if (actionKind.value === 'rename') return ''
  if (actionKind.value === 'collection') return '集合会保存到本地工作区，并立即出现在侧边栏。'
  if (actionKind.value === 'folder') return '文件夹用于组织请求，可在下方选择任意集合或文件夹作为保存位置。'
  return ''
})
const actionNamePlaceholder = computed(() => {
  if (actionKind.value === 'collection') return '例如：用户服务'
  if (actionKind.value === 'folder') return '例如：认证'
  if (actionKind.value === 'rename' && selectedNode.value) return selectedNode.value.name
  return '例如：获取用户详情'
})

/**
 * 当前打开的那个接口对应的节点 id（02 文档 §4.5 里标着「← 当前打开（高亮）」）。
 *
 * 认 `tab.nodeId` 而不是拿 URL 反推：URL 用户随时能改，改完就对不上了。
 * 草稿标签没有 nodeId，此时「定位」按钮置灰 —— 它确实没有可定位的位置。
 */
const activeNodeId = computed(() => tabs.active?.nodeId ?? null)

const CREATE_OPTIONS = [
  { label: '新建请求', key: 'request' },
  { label: '新建文件夹', key: 'folder' },
  { label: '新建集合', key: 'collection' },
]

function onCreateSelect(key: string) {
  if (key === 'collection') openAction('collection')
  else if (key === 'folder') openAction('folder')
  else if (key === 'request') tabs.openNewPanel(selectedRequestParentId())
}

// ── 排序 ──────────────────────────────────────────────────────────────

const SORT_OPTIONS: { mode: SortMode; label: string; hint: string }[] = [
  { mode: null, label: '默认顺序', hint: '集合里的原始顺序' },
  { mode: 'name-asc', label: '名称正序', hint: 'A → Z' },
  { mode: 'name-desc', label: '名称倒序', hint: 'Z → A' },
]

const sortMode = ref<SortMode>(null)
const sortLabel = computed(
  () => SORT_OPTIONS.find((o) => o.mode === sortMode.value)?.label ?? '默认顺序',
)
const sortDropdownOptions = computed(() => [
  {
    type: 'render' as const,
    key: 'sort-description',
    render: () =>
      h(
        'div',
        { class: 'sort-description', role: 'presentation' },
        '选择请求列表的排序方式',
      ),
  },
  { type: 'divider' as const, key: 'sort-divider' },
  ...SORT_OPTIONS.map((option) => ({
    key: option.mode ?? 'default',
    label: sortMode.value === option.mode ? `✓ ${option.label}` : option.label,
    extra: option.hint,
  })),
])

function onSortSelect(key: string | number) {
  const option = SORT_OPTIONS.find((item) => (item.mode ?? 'default') === key)
  if (option) sortMode.value = option.mode
}

/** 可见节点：折叠的文件夹下的子节点不参与渲染。只改这个计算，不重建原数组。 */
const visible = computed(() => sortVisible(visibleNodes(all.value, filter.value), sortMode.value))

/** 新建文件夹可选的目标：集合 + 文件夹（扁平树，按 depth 缩进渲染）。 */
const folderTargets = computed(() => all.value.filter((node) => node.isFolder))
const firstCollectionId = computed(() => all.value.find((node) => node.kind === 'collection')?.id ?? null)

const startIdx = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_H) - OVERSCAN))
const endIdx = computed(() =>
  Math.min(visible.value.length, Math.ceil((scrollTop.value + viewportH.value) / ROW_H) + OVERSCAN),
)
const slice = computed(() => visible.value.slice(startIdx.value, endIdx.value))
const totalH = computed(() => visible.value.length * ROW_H)

function onScroll(e: Event) {
  const el = e.target as HTMLElement
  scrollTop.value = el.scrollTop
  viewportH.value = el.clientHeight
}

/**
 * 视口高度必须在挂载时就从真实元素读出来，并跟着尺寸变化更新。
 *
 * **Spike S3 抓到的 bug**：原来 `viewportH` 初值写死 400，只在 `scroll` 事件里更新。
 * 于是首次渲染是按「400px 高的视口」算行数的 —— 侧边栏实际有 700px 时，
 * 底部会空出约 3 行，直到用户滚动一下才补上。窗口拉高、布局切换同样如此。
 * 虚拟滚动的可视区高度是输入参数，不是常量，不能猜。
 */
let resizeObserver: ResizeObserver | null = null
onMounted(() => {
  const el = scroller.value
  if (!el) return
  viewportH.value = el.clientHeight
  if (typeof ResizeObserver === 'undefined') return
  resizeObserver = new ResizeObserver(() => {
    viewportH.value = el.clientHeight
  })
  resizeObserver.observe(el)
})
onBeforeUnmount(() => {
  resizeObserver?.disconnect()
  resizeObserver = null
})

function toggle(n: FlatNode) {
  selectedId.value = n.id
  deleteError.value = ''
  if (!n.isFolder) {
    if (n.kind === 'request') {
      void workspace.openRequest(n)
    } else {
      // Spike / 浏览器回退数据仍可打开一个合成草稿。
      tabs.open({
        title: n.name,
        method: n.method ?? 'GET',
        url: `{{baseUrl}}/${n.id}`,
        nodeId: n.id,
        preview: true,
      })
    }
    return
  }
  n.expanded = !n.expanded
  // shallowRef 不追踪节点内部字段，换引用才能触发 visible 重算。
  // 数组本身不重建（slice 是浅拷贝），只是换个引用 —— 03 文档 §4.4 的
  // 「展开/折叠只改 visible 计算，不重建数组」指的正是这个。
  all.value = [...all.value]
}

/** 双击固定：以正式标签（非预览）打开请求（02 文档 §4.5 单击预览/双击固定）。 */
function pinOpen(n: FlatNode) {
  if (n.isFolder || n.kind !== 'request') return
  void workspace.openRequest(n, false)
}

function selectedContainerId(): string | undefined {
  const selected = all.value.find((node) => node.id === selectedId.value)
  if (selected?.isFolder) return selected.id
  return selected?.parentId ?? all.value.find((node) => node.kind === 'collection')?.id
}

/** 新建请求继承当前选中的集合/文件夹；未选中时由工作区选择默认集合。 */
function selectedRequestParentId(): string | undefined {
  const selected = all.value.find((node) => node.id === selectedId.value)
  if (selected?.isFolder) return selected.id
  return selected?.kind === 'request' ? selected.parentId ?? undefined : undefined
}

function openAction(kind: ActionKind) {
  actionKind.value = kind
  actionError.value = ''
  deleteError.value = ''
  if (kind === 'rename') {
    if (!selectedNode.value?.kind) {
      actionKind.value = null
      return
    }
    actionName.value = selectedNode.value.name
    return
  }

  actionName.value = ''
  if (kind === 'folder') {
    // 默认继承当前选中的容器；没有选中时退回第一个集合。选择器里可以再改。
    actionParentId.value = selectedContainerId() ?? firstCollectionId.value
    if (!actionParentId.value) actionError.value = '请先创建一个集合。'
    return
  }
}

function closeAction() {
  if (actionSaving.value) return
  actionKind.value = null
  actionName.value = ''
  actionParentId.value = null
  actionError.value = ''
}

async function submitAction() {
  const kind = actionKind.value
  if (!kind) return
  const name = actionName.value.trim()
  if (!name) {
    actionError.value = '名称不能为空。'
    return
  }

  const parentId = kind === 'folder' ? (actionParentId.value ?? undefined) : undefined
  if (kind === 'folder' && !parentId) {
    actionError.value = '请选择保存位置。'
    return
  }
  if (kind === 'rename' && !selectedNode.value?.kind) {
    actionError.value = '请选择要重命名的集合、文件夹或请求。'
    return
  }

  actionSaving.value = true
  actionError.value = ''
  try {
    let success = false
    if (kind === 'collection') {
      success = await workspace.createCollection(name)
    } else if (kind === 'folder') {
      success = await workspace.createFolder(parentId, name)
    } else if (selectedNode.value) {
      success = await workspace.renameNode(selectedNode.value.id, name)
    }

    if (success) {
      actionKind.value = null
      actionName.value = ''
      actionError.value = ''
    } else {
      actionError.value = workspace.lastError?.message ?? '操作失败，请稍后重试。'
    }
  } finally {
    actionSaving.value = false
  }
}

async function deleteSelected() {
  const selected = selectedNode.value
  if (!selected?.kind) return
  deleteError.value = ''
  const success = await workspace.deleteNode(selected.id)
  if (success) {
    if (selectedId.value === selected.id) selectedId.value = null
  } else {
    deleteError.value = workspace.lastError?.message ?? '删除失败，请稍后重试。'
  }
}

const deleteOpen = computed({
  get: () => deleteTarget.value !== null,
  set: (show: boolean) => {
    if (!show) deleteTarget.value = null
  },
})

function itemMenuOptions(node: FlatNode) {
  const label = node.kind === 'collection' ? '集合操作' : node.kind === 'folder' ? '文件夹操作' : '接口操作'
  return [
    { type: 'group' as const, key: `${node.id}-group`, label, children: [
      ...(node.isFolder ? [{ label: '配置', key: 'config', icon: () => h(AppIcon, { component: IconEdit, size: 15 }) }] : []),
      { label: '重命名', key: 'rename', icon: () => h(AppIcon, { component: IconEdit, size: 15 }) },
      { label: '导出', key: 'export', icon: () => h(AppIcon, { component: IconDownload, size: 15 }) },
      { label: '复制', key: 'copy', icon: () => h(AppIcon, { component: IconCopy, size: 15 }) },
    ] },
    { type: 'divider' as const, key: `${node.id}-divider` },
    { label: '删除', key: 'delete', icon: () => h(AppIcon, { component: IconDelete, size: 15 }), props: { class: 'node-menu-delete' } },
  ]
}

function selectNode(node: FlatNode) {
  selectedId.value = node.id
  deleteError.value = ''
}

function exportFilename(node: FlatNode) {
  const safeName = node.name.replace(/[\\/:*?"<>|]/g, '_').trim() || 'apogee-export'
  return `${safeName}.postman_collection.json`
}

function isDescendantOf(node: FlatNode, rootId: string, byId: Map<string, FlatNode>) {
  let cursor: FlatNode | undefined = node
  while (cursor?.parentId) {
    if (cursor.parentId === rootId) return true
    cursor = byId.get(cursor.parentId)
  }
  return false
}

type PostmanItem = {
  name: string
  item?: PostmanItem[]
  request?: Record<string, unknown>
}

function postmanRows(rows: KeyValue[] = []) {
  return rows.map((row) => ({
    key: row.key,
    value: row.value,
    disabled: !row.enabled,
    ...(row.description ? { description: row.description } : {}),
  }))
}

function postmanBody(body: RequestSpec['body']): Record<string, unknown> | undefined {
  if (!body || body.type === 'none') return undefined
  if (body.type === 'json') {
    return { mode: 'raw', raw: body.content, options: { raw: { language: 'json' } } }
  }
  if (body.type === 'text') {
    return { mode: 'raw', raw: body.content, options: { raw: { language: 'text' } } }
  }
  if (body.type === 'formData') return { mode: 'formdata', formdata: postmanRows(body.rows) }
  if (body.type === 'urlEncoded') return { mode: 'urlencoded', urlencoded: postmanRows(body.rows) }
  if (body.type === 'binary') return { mode: 'file', file: { src: body.path } }
  return {
    mode: 'graphql',
    graphql: {
      query: body.query,
      variables: body.variables,
      ...(body.operationName ? { operationName: body.operationName } : {}),
    },
  }
}

function postmanAuth(auth: RequestSpec['auth']): Record<string, unknown> | undefined {
  if (!auth || auth.type === 'inherit') return undefined
  if (auth.type === 'none') return { type: 'noauth' }
  if (auth.type === 'basic') {
    return { type: 'basic', basic: [{ key: 'username', value: auth.username, type: 'string' }, { key: 'password', value: auth.password, type: 'string' }] }
  }
  if (auth.type === 'bearer') return { type: 'bearer', bearer: [{ key: 'token', value: auth.token, type: 'string' }] }
  return {
    type: 'apikey',
    apikey: [
      { key: 'key', value: auth.key, type: 'string' },
      { key: 'value', value: auth.value, type: 'string' },
      { key: 'in', value: auth.location, type: 'string' },
    ],
  }
}

function postmanRequest(spec: RequestSpec): Record<string, unknown> {
  const body = postmanBody(spec.body)
  const auth = postmanAuth(spec.auth)
  return {
    method: spec.method,
    header: postmanRows(spec.headers),
    url: {
      raw: spec.url,
      ...(spec.params?.length ? { query: postmanRows(spec.params) } : {}),
    },
    ...(body ? { body } : {}),
    ...(auth ? { auth } : {}),
  }
}

async function exportNode(node: FlatNode) {
  if (!workspace.available || !node.kind) return
  try {
    const byId = new Map(all.value.map((item) => [item.id, item]))
    const scope = all.value.filter((item) => item.id === node.id || isDescendantOf(item, node.id, byId))
    const requests = await Promise.all(scope
      .filter((item) => item.kind === 'request')
      .map(async (item) => ({ node: item, request: await ipc.workspaceReadRequest(item.id) })))
    const requestsById = new Map(requests.map(({ node: item, request }) => [item.id, request]))
    const buildItems = (parentId: string): PostmanItem[] => {
      const items: PostmanItem[] = []
      for (const item of scope.filter((candidate) => candidate.parentId === parentId)) {
        if (item.kind === 'request') {
          const request = requestsById.get(item.id)
          if (request) items.push({ name: item.name, request: postmanRequest(request.spec) })
          continue
        }
        items.push({ name: item.name, item: buildItems(item.id) })
      }
      return items
    }
    const payload = {
      info: {
        name: node.name,
        schema: 'https://schema.getpostman.com/json/collection/v2.1.0/collection.json',
      },
      item: node.kind === 'request'
        ? [{ name: node.name, request: postmanRequest(requestsById.get(node.id)!.spec) }]
        : buildItems(node.id),
    }
    const blob = new Blob([JSON.stringify(payload, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = exportFilename(node)
    anchor.click()
    URL.revokeObjectURL(url)
    toast().success(`已导出 Postman Collection：“${node.name}”`)
  } catch (error) {
    deleteError.value = error instanceof Error ? error.message : '导出失败，请稍后重试。'
  }
}

async function onItemMenuSelect(key: string | number, node: FlatNode) {
  selectNode(node)
  if (key === 'rename') openAction('rename')
  else if (key === 'config' && node.isFolder) tabs.openContainerConfig(node)
  else if (key === 'export') await exportNode(node)
  else if (key === 'copy') {
    const success = await workspace.duplicateNode(node as WorkspaceNode)
    if (success) toast().success(`已复制“${node.name}”`)
    else deleteError.value = workspace.lastError?.message ?? '复制失败，请稍后重试。'
  } else if (key === 'delete') deleteTarget.value = node
}

async function deleteItem() {
  const target = deleteTarget.value
  if (!target?.kind) return
  deleteError.value = ''
  const success = await workspace.deleteNode(target.id)
  if (success && selectedId.value === target.id) selectedId.value = null
  if (!success) deleteError.value = workspace.lastError?.message ?? '删除失败，请稍后重试。'
  if (success) deleteTarget.value = null
}

// ── 定位当前接口 ──────────────────────────────────────────────────────

/**
 * 把第 `row` 行滚进视口。
 *
 * 已经完整在视口内就**不动** —— 定位的目的是「让我看到它」，行本来就在眼前时
 * 再跳一次只会让人丢失上下文。需要滚动时居中，这样目标行上下都有参照。
 *
 * 直接算 px 而不是用 `scrollIntoView`：行是绝对定位在一个撑高的 spacer 里的，
 * 目标行在滚动前可能还没有对应的 DOM 节点（虚拟滚动只渲染可视区那几十行）。
 */
function revealRow(row: number) {
  const el = scroller.value
  const vh = el?.clientHeight ?? viewportH.value
  const top = row * ROW_H
  if (top >= scrollTop.value && top + ROW_H <= scrollTop.value + vh) return
  const max = Math.max(0, totalH.value - vh)
  const next = Math.min(max, Math.max(0, top - Math.round((vh - ROW_H) / 2)))
  scrollTop.value = next
  if (el) el.scrollTop = next
}

/**
 * 定位到当前活跃接口：逐级展开祖先 → 必要时清掉过滤 → 滚到该行。
 *
 * 三步都不能省。目标行可能因为三种原因不在屏上：祖先文件夹折叠着（它压根不在
 * `visible` 里）、过滤条件把它筛掉了、或者只是滚出了视口。只做最后一步的
 * 「定位」在前两种情况下会安静地什么都不发生，那是最难排查的一类交互 bug。
 */
async function locateActive() {
  const id = activeNodeId.value
  if (!id) return

  const idx = indexOfNode(all.value, id)
  if (idx < 0) return

  let expandedAny = false
  for (const ai of ancestorIndexes(all.value, idx)) {
    const ancestor = all.value[ai]!
    if (!ancestor.expanded) {
      ancestor.expanded = true
      expandedAny = true
    }
  }
  if (expandedAny) all.value = [...all.value]

  // 过滤模式下目标行可能被筛掉。清掉过滤是有损操作（用户的输入没了），
  // 所以只在确实筛掉了的时候做。
  if (filter.value.trim() && !visible.value.some((n) => n.id === id)) filter.value = ''

  // 等 DOM 把 spacer 高度更新上去：新展开的行还没进 DOM 时，
  // 浏览器会把超出当前 scrollHeight 的 scrollTop 直接夹掉。
  await nextTick()

  const row = visible.value.findIndex((n) => n.id === id)
  if (row >= 0) revealRow(row)
}

/**
 * 供 Spike S3 驱动滚动与读取渲染状态。
 *
 * 只暴露量测需要的最小面：滚动容器（S3 要按帧改 `scrollTop`）以及
 * 三个规模数字（总数 / 可见 / 实渲染）。不暴露内部 ref，避免量测台反过来
 * 篡改组件状态。
 */
defineExpose({
  scroller,
  stats: () => ({
    total: all.value.length,
    visible: visible.value.length,
    rendered: slice.value.length,
    rowHeight: ROW_H,
    overscan: OVERSCAN,
    scrollHeight: totalH.value,
  }),
  /**
   * 程序化滚动。**同步**更新内部 `scrollTop`，不等 `scroll` 事件。
   *
   * 这不是给量测开的后门：「在树里定位到某个请求」「键盘上下键移动选中行」
   * 都需要这个入口（02 文档 §4.5 的单击预览与命令面板跳转都会用到）。
   * S3 顺便用它测「一次滚动步进的主线程成本」——走 `scroll` 事件是异步的，
   * 测不出单步开销，只能测到显示器节奏。
   */
  scrollTo: (px: number) => {
    scrollTop.value = px
    const el = scroller.value
    if (el) el.scrollTop = px
  },
  toggleFolderAt: (index: number) => {
    const node = all.value[index]
    if (node?.isFolder) toggle(node)
  },
  /**
   * 产品入口，不是量测用的：命令面板的「在集合中显示当前请求」和快捷键都会调它。
   * 放在这里而不是只挂在按钮上，是因为定位这件事有多个触发点。
   */
  locateActive,
})
</script>

<template>
  <aside class="sidebar">
    <NaiveThemeScope>
      <div class="head">
        <div class="head-tools">
        <AppButton
          class="head-button"
          quaternary
          circle
          size="small"
          aria-label="工作区统计"
          title="工作区统计"
          @click="tabs.openWorkspaceStats()"
        >
          <template #icon>
            <AppIcon :component="IconStats" :size="16" />
          </template>
        </AppButton>

        <AppDropdown :options="CREATE_OPTIONS" @select="onCreateSelect">
          <AppButton class="head-button" quaternary circle size="small" aria-label="新建" title="新建">
            <template #icon>
              <AppIcon :component="IconAdd" :size="16" />
            </template>
          </AppButton>
        </AppDropdown>

        <AppButton
          class="head-button"
          quaternary
          circle
          size="small"
          aria-label="重命名选中项"
          title="重命名选中项"
          :disabled="!selectedId"
          @click="openAction('rename')"
        >
          <template #icon>
            <AppIcon :component="IconEdit" :size="16" />
          </template>
        </AppButton>

        <AppPopconfirm
          v-if="workspace.available"
          :show-icon="false"
          positive-text="删除"
          negative-text="取消"
          @positive-click="deleteSelected"
        >
          <template #trigger>
            <AppButton
              class="head-button danger-button"
              quaternary
              circle
              size="small"
              aria-label="删除选中项"
              title="删除选中项"
              :disabled="!selectedId"
            >
              <template #icon>
                <AppIcon :component="IconDelete" :size="16" />
              </template>
            </AppButton>
          </template>
          确定删除“{{ selectedNode?.name ?? '当前选中项' }}”及其内容吗？
        </AppPopconfirm>

        <!-- 定位当前接口。置灰时也保留 title，说明为什么不能点。 -->
        <AppButton
          class="head-button"
          quaternary
          circle
          size="small"
          :disabled="!activeNodeId"
          :title="activeNodeId ? '定位当前接口' : '当前标签不是集合里的接口'"
          aria-label="定位当前接口"
          @click="locateActive"
        >
          <template #icon>
            <AppIcon :component="IconLocate" :size="16" />
          </template>
        </AppButton>

        <AppDropdown :options="sortDropdownOptions" @select="onSortSelect">
          <AppButton
            class="head-button"
            :class="{ on: sortMode !== null }"
            quaternary
            circle
            size="small"
            aria-haspopup="menu"
            :title="`排序：${sortLabel}`"
            aria-label="排序方式"
          >
            <template #icon>
              <AppIcon :component="IconSort" :size="16" />
            </template>
          </AppButton>
        </AppDropdown>
        </div>
        <AppInput
          v-model:value="filter"
          class="filter"
          size="small"
          placeholder="过滤请求…"
          aria-label="过滤请求"
          :input-props="{ spellcheck: false }"
        />
      </div>

      <AppAlert
        v-if="deleteError"
        class="delete-error"
        type="error"
        :show-icon="false"
        closable
        @close="deleteError = ''"
      >
        {{ deleteError }}
      </AppAlert>

      <AppModal
        v-model:show="actionOpen"
        preset="card"
        :title="actionModalTitle"
        :style="{ width: '420px' }"
        :mask-closable="!actionSaving"
        :closable="!actionSaving"
      >
        <div class="action-modal">
          <p class="action-hint">{{ actionModalHint }}</p>
          <AppAlert v-if="actionError" class="action-error" type="error" :show-icon="false">
            {{ actionError }}
          </AppAlert>

          <template v-if="actionKind === 'folder'">
            <span class="action-label">保存位置</span>
            <div class="action-target-list" role="listbox" aria-label="选择新建文件夹的位置">
              <button
                v-for="node in folderTargets"
                :key="node.id"
                type="button"
                class="action-target-row"
                :class="{ selected: node.id === actionParentId }"
                role="option"
                :aria-selected="node.id === actionParentId"
                :style="{ paddingLeft: `${8 + node.depth * 18}px` }"
                @click="actionParentId = node.id"
              >
                <span class="action-target-icon">
                  <AppIcon :component="node.kind === 'collection' ? IconFolder : IconFolderOpen" :size="15" />
                </span>
                <span class="action-target-name">{{ node.name }}</span>
              </button>
              <p v-if="!folderTargets.length" class="action-target-empty">还没有集合，请先创建集合。</p>
            </div>
          </template>

          <label class="action-field">
            <span v-if="actionKind !== 'rename'">名称</span>
            <AppInput
              v-model:value="actionName"
              autofocus
              clearable
              maxlength="120"
              show-count
              :placeholder="actionNamePlaceholder"
              :input-props="{ spellcheck: false }"
              @keyup.enter="submitAction"
            />
          </label>
        </div>
        <template #footer>
          <AppSpace justify="end">
            <AppButton :disabled="actionSaving" @click="closeAction">取消</AppButton>
            <AppButton type="primary" :loading="actionSaving" @click="submitAction">确认</AppButton>
          </AppSpace>
        </template>
      </AppModal>

      <AppModal
        v-model:show="deleteOpen"
        preset="card"
        title="删除"
        :style="{ width: '400px' }"
      >
        <p class="delete-confirmation">确定删除“{{ deleteTarget?.name }}”及其内容吗？此操作不可恢复。</p>
        <template #footer>
          <AppSpace justify="end">
            <AppButton @click="deleteTarget = null">取消</AppButton>
            <AppButton type="error" @click="deleteItem">删除</AppButton>
          </AppSpace>
        </template>
      </AppModal>
    </NaiveThemeScope>
    <div ref="scroller" class="scroller" @scroll.passive="onScroll">
      <div class="spacer" :style="{ height: `${totalH}px` }">
        <div
          v-for="(n, i) in slice"
          :key="n.id"
          class="row"
          :class="{ folder: n.isFolder, active: n.id === activeNodeId, selected: n.id === selectedId }"
          :aria-current="n.id === activeNodeId ? 'true' : undefined"
          :style="{
            transform: `translateY(${(startIdx + i) * ROW_H}px)`,
            paddingLeft: `${8 + n.depth * 18}px`,
          }"
          @click="toggle(n)"
          @dblclick="pinOpen(n)"
        >
          <span class="node-icon" :class="{ folder: n.isFolder }">
            <AppIcon :component="n.isFolder ? (n.expanded ? IconFolderOpen : IconFolder) : IconApi" :size="15" />
          </span>
          <span v-if="!n.isFolder" class="m method-badge" :class="`method-${String(n.method ?? 'GET').toLowerCase()}`">{{ n.method === 'DELETE' ? 'DEL' : n.method }}</span>
          <span class="name">{{ n.name }}</span>
          <AppDropdown :options="itemMenuOptions(n)" placement="bottom-end" @select="onItemMenuSelect($event, n)">
            <AppButton
              class="item-menu-button"
              quaternary
              circle
              size="tiny"
              :aria-label="`${n.name} 操作菜单`"
              title="更多操作"
              @click.stop
            >
              <template #icon><AppIcon :component="IconMore" :size="15" /></template>
            </AppButton>
          </AppDropdown>
        </div>
      </div>
    </div>
    <div class="foot">
      共 {{ all.length }} 项，可见 {{ visible.length }}，实渲染 {{ slice.length }}
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  width: var(--a-w-sidebar); flex: 0 0 auto;
  display: flex; flex-direction: column; min-height: 0;
  /* 没有 border-right —— 与主区之间靠 --a-gutter 的间隙分隔（02 文档 §3.2）。
     背景由 .a-block 提供，这里保留一份是为了组件单独使用时也有底色。 */
  background: var(--a-sidebar, var(--a-bg-2));
}
/* 功能按钮单独一行，过滤框独占下一行，避免侧边栏变窄时控件互相挤压。 */
.head {
  padding: 8px;
  flex: 0 0 auto;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 4px;
}
.head-tools { display: flex; align-items: center; gap: 3px; min-height: 30px; }
.head-button {
  flex: 0 0 auto;
  color: var(--a-text-dim);
}
.head-button:hover:not(:disabled) { color: var(--a-text); }
.head-button.danger-button { color: var(--a-text-faint); }
.head-button.danger-button:hover:not(:disabled) { color: var(--a-error); }
.filter {
  flex: 0 0 auto;
  width: 100%;
  min-width: 0;
  height: 36px;
  padding: 0;
  background: var(--a-bg-3);
  border-radius: var(--a-radius-md);
  color: var(--a-text);
  font: 12px/1 var(--a-font-ui);
}
.filter :deep(.n-input__input-el) {
  color: var(--a-text);
  font: 12px/1 var(--a-font-ui);
  padding: 0 6px;
}
.filter :deep(.n-input__border),
.filter :deep(.n-input__state-border) { border-radius: var(--a-radius-md); }
/* 不能写 outline: none —— 那会盖掉 tokens.css 里的全局 :focus-visible 焦点环。 */
.filter:focus-within :deep(.n-input__state-border) {
  border-color: var(--a-focus);
  box-shadow: 0 0 0 1px var(--a-focus) inset;
}

/* ── 头部工具按钮 ──
   功能按钮集中在同一行，24×24 命中区（图标只有 14px）。方形而不是圆形：
   它们和排序按钮同属工具栏，圆角对齐 --a-radius-sm 就够，胶囊只留给徽章。 */
.head-button.on { color: var(--a-primary); background: var(--a-selected); }
.ico {
  width: 14px; height: 14px;
  fill: none; stroke: currentcolor; stroke-width: 1.5;
  stroke-linecap: round; stroke-linejoin: round;
}
:global(.sort-description) {
  padding: 3px 8px 2px;
  color: var(--a-text-faint);
  font: var(--a-fs-xs) / 1.5 var(--a-font-ui);
  white-space: nowrap;
  pointer-events: none;
}

.delete-error { margin: 0 6px 4px; }
.delete-confirmation { margin: 0; color: var(--a-text-soft); font: var(--a-fs-sm) / 1.6 var(--a-font-ui); }
.action-modal { display: flex; flex-direction: column; gap: 10px; }
.action-hint { margin: 0; color: var(--a-text-dim); font-size: 12px; line-height: 1.5; }
.action-error { margin-bottom: 2px; }
.action-label {
  color: var(--a-text-dim);
  font: var(--a-fw-medium) var(--a-fs-sm) / 1 var(--a-font-ui);
}
.action-field {
  display: grid;
  gap: 7px;
  min-width: 0;
  color: var(--a-text-soft);
  font: var(--a-fs-sm) / 1 var(--a-font-ui);
}
.action-field :deep(.n-input) { min-height: 38px; }
.action-field :deep(.n-input-wrapper) { padding-inline: 12px; }
.action-target-list {
  max-height: 200px;
  overflow-y: auto;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
  padding: 4px;
}
.action-target-row {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  box-sizing: border-box;
  height: 28px;
  padding: 0 8px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text);
  font: var(--a-fs-sm) / 1 var(--a-font-ui);
  text-align: left;
  cursor: pointer;
}
.action-target-row:hover { background: var(--a-hover); }
.action-target-row.selected { background: var(--a-selected); }
.action-target-icon { flex: 0 0 auto; display: inline-flex; color: var(--a-primary); }
.action-target-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.action-target-empty { margin: 0; padding: 12px 8px; color: var(--a-text-faint); font-size: 12px; text-align: center; }

/* scrollbar-gutter: stable —— 树加载/滚动条出现时内容区不会因 12px 滚动条而左右抖动。 */
.scroller { flex: 1 1 auto; overflow-y: auto; overflow-x: hidden; min-height: 0; scrollbar-gutter: stable; }
.spacer { position: relative; }
.row {
  position: absolute; left: 0; right: 0; height: 25px;
  display: flex; align-items: center; gap: 5px;
  /* transform only positions the virtual row; the row owns vertical alignment. */
  font: 12px/normal var(--a-font-ui); color: var(--a-text-dim);
  white-space: nowrap; cursor: pointer;
}

.row:hover { background: var(--a-selected); color: var(--a-text); border-radius: 6px; box-shadow: inset 0 0 0 1px var(--a-primary); }
.row.selected:not(.active) { box-shadow: inset 2px 0 0 var(--a-border-strong); }
.row.folder { color: var(--a-text); font-weight: 600; }
/* 当前打开的接口与“新建请求”选项使用同一套轻量激活态。 */
.row.active { background: var(--a-selected); color: var(--a-text); border: 0; border-radius: 6px; box-shadow: inset 0 0 0 1px var(--a-primary); }
.row.active:hover { background: var(--a-selected); color: var(--a-text); }
.row.active .node-icon,
.row.active .node-icon.folder { color: var(--a-primary) !important; }
.node-icon {
  display: flex;
  align-items: center;
  justify-content: center;
  flex: 0 0 16px;
  height: 100%;
  color: var(--a-text-faint);
}
.node-icon.folder { color: var(--a-text-dim); }
.m.method-badge { width: 40px; min-width: 40px; flex: 0 0 40px; height: 18px; display: inline-flex; align-items: center; justify-content: center; padding: 0; border: 1px solid color-mix(in srgb, currentColor 22%, transparent); border-radius: 5px; font: 700 10px/18px var(--a-font-mono); letter-spacing: .02em; }
.method-get { color: var(--a-get); background: var(--a-get-bg); }
.method-post { color: var(--a-post); background: var(--a-post-bg); }
.method-put { color: var(--a-put); background: var(--a-put-bg); }
.method-patch { color: var(--a-patch); background: var(--a-patch-bg); }
.method-delete { color: var(--a-delete); background: var(--a-delete-bg); }
.m,
.name {
  display: block;
  align-self: center;
  height: auto;
  line-height: normal;
}
.name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.item-menu-button {
  flex: 0 0 24px;
  width: 24px;
  height: 24px;
  margin-left: auto;
  padding: 0;
  color: var(--a-text-faint);
  opacity: 0;
}
.row:hover .item-menu-button,
.row.selected .item-menu-button,
.item-menu-button:focus-visible { opacity: 1; }
.item-menu-button:hover { color: var(--a-text); background: var(--a-bg-3); }
:global(.node-menu-delete) { color: var(--a-error) !important; }
.m { font: 700 10px/normal var(--a-font-mono); width: 40px; }
.name {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}
.foot {
  flex: 0 0 auto; padding: 3px 6px; border-top: 1px solid var(--a-border);
  font: 10px/14px var(--a-font-mono); color: var(--a-text-faint);
}
</style>
