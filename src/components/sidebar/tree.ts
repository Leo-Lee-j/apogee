/**
 * 集合树的数据形状与虚拟滚动参数。
 *
 * 单独成模块而不是塞在 `Sidebar.vue` 里，原因只有一个：**Spike S3 必须量测产品
 * 真正在用的那套参数和那份数据结构**。如果量测台自己抄一份 `ROW_H` / `FlatNode`
 * 出来，测的就是副本，产品改了参数而量测台没改也不会有人发现。
 *
 * 03 文档 §4.4 对这里的约束：
 *   - Rust 侧返回**扁平数组**，前端不做递归构造，所以没有 `children` 字段
 *   - 只渲染可视区约 30 行 + 上下缓冲 10 行
 *   - 展开/折叠只改 `visible` 计算，不重建数组
 */

/** 行高（px）。虚拟滚动靠定高换取 O(1) 的索引换算，不做动态测高。 */
export const ROW_H = 22

/** 可视区上下各多渲染的行数。缓冲越大越不容易露白，代价是每帧多算几行。 */
export const OVERSCAN = 10

/**
 * 扁平树节点。**没有 `children` 字段** —— 层级只由 `depth` 表达。
 *
 * 正式实现里这个数组由 Rust 侧 `list_tree` 返回（`Vec<FlatNode>`，
 * 含 `id / parent_id / depth / name / method / expanded`）。
 */
export interface FlatNode {
  id: string
  parentId?: string | null
  depth: number
  name: string
  method?: string
  isFolder: boolean
  expanded: boolean
  kind?: 'collection' | 'folder' | 'request'
}

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'] as const

export interface SynthOptions {
  /** 文件夹数量 */
  folders: number
  /** 每个文件夹下的请求数 */
  perFolder: number
  /**
   * 初始展开的文件夹数量。`'all'` 表示全部展开。
   *
   * 这个参数对 S3 很关键：折叠的文件夹下的子节点根本不进 `visible`，
   * 所以「合成了 5000 个节点」和「虚拟滚动要处理 5000 行」是两件事。
   * 要压满虚拟滚动就得全部展开。
   */
  expanded: number | 'all'
}

/**
 * 合成集合树。骨架阶段用它撑出规模，Spike S3 用它按档位造压力。
 *
 * 生成的结构与真实集合一致：文件夹（depth 0）+ 其下的请求（depth 1）。
 * 名称与方法都是确定性的，不用随机数 —— 量测要可复现。
 */
export function synthTree(options: SynthOptions): FlatNode[] {
  const { folders, perFolder, expanded } = options
  const expandCount = expanded === 'all' ? folders : expanded
  const out: FlatNode[] = []
  for (let f = 0; f < folders; f++) {
    out.push({
      id: `f${f}`,
      depth: 0,
      name: `模块-${f}`,
      isFolder: true,
      expanded: f < expandCount,
    })
    for (let r = 0; r < perFolder; r++) {
      out.push({
        id: `f${f}-r${r}`,
        depth: 1,
        name: `接口 ${f}-${r}`,
        method: METHODS[(f + r) % METHODS.length],
        isFolder: false,
        expanded: false,
      })
    }
  }
  return out
}

/**
 * 把扁平数组按展开状态过滤成「当前可见的行」。
 *
 * 这是 `Sidebar.vue` 里 `visible` 计算的实现体，抽出来供 S3 单独计时：
 * 展开/折叠的代价全在这一趟 O(N) 扫描里，而 DOM 只更新可视区那几十行。
 * 两者必须分开量，否则看不出「5000 节点」的成本到底落在哪。
 *
 * @param filter 非空时进入搜索模式：只保留匹配的请求，文件夹不参与
 */
export function visibleNodes(all: FlatNode[], filter = ''): FlatNode[] {
  const q = filter.trim().toLowerCase()
  const out: FlatNode[] = []
  let skipDepth = -1
  for (const n of all) {
    if (skipDepth >= 0 && n.depth > skipDepth) continue
    skipDepth = -1
    if (q) {
      if (n.isFolder) continue
      if (!n.name.toLowerCase().includes(q) && !(n.method ?? '').toLowerCase().includes(q)) continue
      out.push(n)
      continue
    }
    out.push(n)
    if (n.isFolder && !n.expanded) skipDepth = n.depth
  }
  return out
}

/**
 * 排序方式。`null` 表示保持集合原顺序（用户自己排的顺序，不该被默默改掉）。
 */
export type SortMode = null | 'name-asc' | 'name-desc'

/**
 * 比较器只建一次。
 *
 * `localeCompare` 每次调用都可能重建一个内部 collator，20000 节点排一次要调
 * 约 N·logN 次 —— 这是 `Intl.Collator` 文档里点名的场景。
 *
 * `numeric: true` 不是可选项：接口名普遍带序号，不开的话「接口 2-10」会排在
 * 「接口 2-9」前面（按字符逐位比，'1' < '9'），用户会当成排序坏了。
 */
const nameCollator = new Intl.Collator('zh-Hans-CN', { numeric: true, sensitivity: 'base' })

/**
 * 把 `visibleNodes` 的结果按名称重排。**不改动传入数组**。
 *
 * 排序对每个文件夹是**局部**的：文件夹自身的位置不动，只重排「同一父级下连续的
 * 请求」。跨过文件夹边界混排会把 depth 序列打乱，而虚拟滚动的缩进和折叠语义
 * 全靠 depth 表达 —— 那等于把树排成一锅面条。
 *
 * 实现是「同深度的连续请求分桶 → 桶内排序 → 按原位置拼回」。过滤模式下
 * `visibleNodes` 只吐请求、不吐文件夹，于是整份结果就是一个桶，正好是想要的
 * 「搜索结果整体排序」。
 */
export function sortVisible(nodes: FlatNode[], mode: SortMode): FlatNode[] {
  if (!mode) return nodes
  const dir = mode === 'name-asc' ? 1 : -1
  const cmp = (a: FlatNode, b: FlatNode) => dir * nameCollator.compare(a.name, b.name)

  const out: FlatNode[] = []
  let bucket: FlatNode[] = []
  let bucketDepth = -1
  const flush = () => {
    if (bucket.length === 0) return
    bucket.sort(cmp)
    out.push(...bucket)
    bucket = []
    bucketDepth = -1
  }
  for (const n of nodes) {
    if (n.isFolder) {
      // 文件夹是桶的边界，位置保持不动
      flush()
      out.push(n)
      continue
    }
    if (bucket.length > 0 && n.depth !== bucketDepth) flush()
    bucketDepth = n.depth
    bucket.push(n)
  }
  flush()
  return out
}

/** 按 id 找节点下标。找不到返回 -1。 */
export function indexOfNode(all: FlatNode[], id: string): number {
  return all.findIndex((n) => n.id === id)
}

/**
 * 某个节点的祖先下标（由近及远）。
 *
 * 扁平数组里没有 `children`，也不依赖 `parent_id`：层级只由 `depth` 表达，
 * 而数组是前序的，所以从该节点往前扫，第一个 depth 更小的节点就是它的父级，
 * 继续往前找更小的即得整条链。
 *
 * 「定位当前接口」要用它把折叠着的祖先逐级展开 —— 目标行藏在折叠的文件夹里时，
 * 光滚动是滚不出来的，它根本不在 `visibleNodes` 的结果里。
 */
export function ancestorIndexes(all: FlatNode[], index: number): number[] {
  const start = all[index]
  if (!start) return []
  const out: number[] = []
  let need = start.depth - 1
  for (let i = index - 1; i >= 0 && need >= 0; i--) {
    const d = all[i]!.depth
    // 用 <= 而不是 ===：真实数据里 depth 有跳级的可能，用 === 会一路扫到头
    if (d <= need) {
      out.push(i)
      need = d - 1
    }
  }
  return out
}

/** 方法名 → 颜色 token。 */
export const methodColor: Record<string, string> = {
  GET: 'var(--a-get)',
  POST: 'var(--a-post)',
  PUT: 'var(--a-put)',
  PATCH: 'var(--a-patch)',
  DELETE: 'var(--a-delete)',
}
