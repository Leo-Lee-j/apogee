import { computed, shallowRef } from 'vue'
import { ipc, normalizeError, type TreeNode, type TreePage } from '@/ipc'

export const TREE_PAGE_SIZE = 200
export const TREE_CACHE_PAGES = 12
interface Page extends TreePage { offset: number; loading: boolean; error: string | null; retryOffset?: number }
export type TreeRow = { key: string; depth: number } & ({ type: 'node'; node: TreeNode } | { type: 'page'; path: string; page: Page })

export function createJsonTree() {
  const pages = shallowRef<Record<string, Page>>({})
  const expanded = shallowRef(new Set(['']))
  const rootPath = shallowRef('')
  let responseId = '', generation = 0
  const operations = new Map<string, symbol>()
  function descendants(path: string) {
    const next = { ...pages.value }, open = new Set(expanded.value)
    for (const key of Object.keys(next)) if (key !== path && (path === '' || key.startsWith(`${path}/`))) {
      delete next[key]; open.delete(key); operations.delete(key)
    }
    pages.value = next; expanded.value = open
  }
  async function load(path: string, offset = 0): Promise<void> {
    if (pages.value[path]?.loading || offset < 0) return
    const old = pages.value[path] ?? { nodes: [], total: 0, offset: 0, loading: false, error: null }
    if (!pages.value[path] && Object.keys(pages.value).length >= TREE_CACHE_PAGES) return
    const operation = Symbol(), version = generation
    operations.set(path, operation)
    pages.value = { ...pages.value, [path]: { ...old, loading: true, error: null } }
    try {
      const page = await ipc.getResponseTreePage(responseId, path, offset, TREE_PAGE_SIZE)
      if (version !== generation || operations.get(path) !== operation) return
      descendants(path)
      pages.value = { ...pages.value, [path]: { ...page, offset, loading: false, error: null } }
    } catch (error) {
      if (version === generation && operations.get(path) === operation)
        pages.value = { ...pages.value, [path]: { ...old, loading: false, error: normalizeError(error).message, retryOffset: offset } }
    }
  }
  async function toggle(node: TreeNode) {
    if (!node.childCount) return
    if (expanded.value.has(node.path)) {
      descendants(node.path)
      const next = { ...pages.value }; delete next[node.path]; pages.value = next
      const open = new Set(expanded.value); open.delete(node.path); expanded.value = open
      operations.delete(node.path)
    } else {
      if (Object.keys(pages.value).length >= TREE_CACHE_PAGES) return `最多同时展开 ${TREE_CACHE_PAGES} 层或分支，请折叠部分节点，或用“仅查看”继续浏览深层节点`
      expanded.value = new Set([...expanded.value, node.path])
      await load(node.path)
    }
  }
  function reset(id: string) {
    responseId = id
    return focus('')
  }
  function focus(path: string) {
    generation++; rootPath.value = path; operations.clear(); pages.value = {}; expanded.value = new Set([path])
    return load(path)
  }
  function dispose() { generation++; operations.clear() }
  const rows = computed<TreeRow[]>(() => {
    const result: TreeRow[] = []
    const baseDepth = rootPath.value.split('/').length - 1
    const stack: (TreeRow | { type: 'visit'; path: string; depth: number })[] = [{ type: 'visit', path: rootPath.value, depth: 0 }]
    while (stack.length) {
      const item = stack.pop()!
      if (item.type !== 'visit') {
        result.push(item)
        if (item.type === 'node' && expanded.value.has(item.node.path)) stack.push({ type: 'visit', path: item.node.path, depth: item.node.depth - baseDepth })
        continue
      }
      const page = pages.value[item.path]
      if (!page) continue
      result.push({ type: 'page', key: `page:${item.path}`, path: item.path, page, depth: item.depth })
      for (let i = page.nodes.length - 1; i >= 0; i--) {
        const node = page.nodes[i]!
        stack.push({ type: 'node', key: node.path, node, depth: node.depth - 1 - baseDepth })
      }
    }
    return result
  })
  return { pages, expanded, rootPath, rows, load, toggle, reset, focus, dispose }
}
