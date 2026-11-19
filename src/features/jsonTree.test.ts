import { beforeEach, expect, it, vi } from 'vitest'
import type { TreeNode, TreePage } from '@/ipc'
const getPage = vi.hoisted(() => vi.fn())
vi.mock('@/ipc', () => ({ ipc: { getResponseTreePage: getPage }, normalizeError: (error: Error) => ({ message: error.message }) }))
import { createJsonTree, TREE_CACHE_PAGES } from './jsonTree'
const node = (path: string, children = 0): TreeNode => ({ path, key: path.split('/').at(-1)!, kind: children ? 'array' : 'number', childCount: children, depth: path.split('/').length - 1, preview: '1' })
beforeEach(() => { getPage.mockReset() })
it('pages beyond 200 and independently pages nested children, retaining only one page per path', async () => {
  getPage.mockImplementation(async (_id, path, offset) => ({ total: path ? 401 : 201, nodes: Array.from({ length: Math.min(200, (path ? 401 : 201) - offset) }, (_, i) => node(`${path}/${offset + i}`, path ? 0 : 401)) }))
  const tree = createJsonTree(); await tree.reset('response')
  expect(tree.pages.value['']?.nodes).toHaveLength(200)
  await tree.load('', 200); expect(tree.pages.value['']?.nodes[0]?.key).toBe('200')
  await tree.toggle(tree.pages.value['']!.nodes[0]!)
  await tree.load('/200', 400)
  expect(tree.pages.value['/200']?.nodes[0]?.path).toBe('/200/400')
  expect(tree.rows.value.some(row => row.type === 'node' && row.node.path === '/200/400')).toBe(true)
  await tree.load('', 0); expect(Object.keys(tree.pages.value)).toEqual([''])
})
it('keeps the old page on failure and permits retry', async () => {
  getPage.mockResolvedValueOnce({ total: 201, nodes: [node('/0')] }).mockRejectedValueOnce(new Error('read failed')).mockResolvedValueOnce({ total: 201, nodes: [node('/200')] })
  const tree = createJsonTree(); await tree.reset('r'); await tree.load('', 200)
  expect(tree.pages.value['']).toMatchObject({ offset: 0, error: 'read failed' })
  await tree.load('', 200); expect(tree.pages.value['']).toMatchObject({ offset: 200, error: null })
})
it('ignores a late response after switching responses', async () => {
  let finish!: (page: TreePage) => void
  getPage.mockImplementationOnce(() => new Promise<TreePage>(resolve => { finish = resolve })).mockResolvedValueOnce({ total: 1, nodes: [node('/new')] })
  const tree = createJsonTree(), old = tree.reset('old')
  await tree.reset('new'); finish({ total: 1, nodes: [node('/old')] }); await old
  expect(tree.pages.value['']?.nodes[0]?.path).toBe('/new')
})
it('bounds cached pages and releases a collapsed branch', async () => {
  getPage.mockResolvedValue({ total: 1, nodes: [node('/x')] })
  const tree = createJsonTree(); await tree.reset('r')
  for (let i = 0; i < TREE_CACHE_PAGES - 1; i++) await tree.toggle(node(`/branch${i}`, 1))
  expect(await tree.toggle(node('/overflow', 1))).toContain('折叠')
  expect(Object.keys(tree.pages.value)).toHaveLength(TREE_CACHE_PAGES)
  await tree.toggle(node('/branch0', 1)); await tree.toggle(node('/overflow', 1))
  expect(tree.pages.value['/overflow']).toBeDefined()
  await tree.focus('/overflow/deeper/leaf')
  expect(Object.keys(tree.pages.value)).toEqual(['/overflow/deeper/leaf'])
  expect(tree.rootPath.value).toBe('/overflow/deeper/leaf')
})
