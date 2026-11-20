import assert from 'node:assert/strict'
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

export async function round2(page, root, base, received) {
  const fixture = await page.evaluate(async base => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const snapshot = await ipc.workspaceCreateCollection('Round2 fixture')
    const collection = snapshot.nodes.find(node => node.name === 'Round2 fixture')
    const container = await ipc.workspaceReadContainer(collection.id)
    await ipc.workspaceSaveContainer({ ...container, auth: { type: 'bearer', token: 'round2-fixture-token' } }, container.auth)
    const request = await ipc.workspaceCreateRequest(collection.id, 'Round2 source')
    const spec = { ...request.spec, method: 'POST', url: base + '/round2/echo?from=raw&dup=1&dup=2',
      params: [{ key: 'extra', value: 'table', enabled: true }, { key: 'off', value: 'omit', enabled: false }],
      body: { type: 'json', content: '{"round2":"body-fixture"}' }, auth: { type: 'inherit' } }
    await ipc.workspaceSaveRequest(request.id, request.name, spec, request.spec.auth)
    const workspace = useWorkspaceStore(); await workspace.load(); await workspace.openRequest({ id: request.id, kind: 'request' }, false)
    return { collection, requestId: request.id }
  }, base)
  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Body', exact: true }).click()
  await page.locator('.body-types').getByText('none', { exact: true }).click()
  await page.locator('.body-types').getByText('JSON', { exact: true }).click()
  assert.equal(await page.evaluate(async () => (await import('/src/stores/tabs.ts')).useTabsStore().active.body.content), '{"round2":"body-fixture"}')
  await page.locator('.body-types').getByText('none', { exact: true }).click()
  await page.evaluate(async id => {
    const { useTabsStore } = await import('/src/stores/tabs.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const { useEditorRecoveryStore } = await import('/src/stores/editorRecovery.ts')
    const tabs = useTabsStore(); await tabs.flushAll(); await useEditorRecoveryStore().flush(); await tabs.close(tabs.active.id)
    await useWorkspaceStore().openRequest({ id, kind: 'request' }, true)
  }, fixture.requestId)
  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Body', exact: true }).click()
  await page.locator('.body-types').getByText('JSON', { exact: true }).click()
  assert.equal(await page.evaluate(async () => (await import('/src/stores/tabs.ts')).useTabsStore().active.body.content), '{"round2":"body-fixture"}')

  async function send() {
    const count = received.length
    await page.getByRole('button', { name: '发送请求', exact: true }).click()
    await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()
    const deadline = Date.now() + 10000
    while (received.length <= count) { if (Date.now() > deadline) throw new Error('Round2 request did not reach receiver'); await new Promise(resolve => setTimeout(resolve, 25)) }
    return received.at(-1)
  }
  const original = await send()
  assert.equal(original.headers.authorization, 'Bearer round2-fixture-token')
  assert.equal(original.url, '/round2/echo?from=raw&dup=1&dup=2&extra=table')
  await page.evaluate(async () => {
    await (await import('/src/stores/tabs.ts')).useTabsStore().flushAll()
    window.__round2CreateObjectURL = URL.createObjectURL
    URL.createObjectURL = function(blob) { window.__round2Blob = blob; return window.__round2CreateObjectURL.call(this, blob) }
  })
  await page.getByRole('button', { name: 'Round2 fixture 操作菜单', exact: true }).click()
  await page.getByText('导出', { exact: true }).click()
  await page.waitForFunction(() => Boolean(window.__round2Blob))
  const exported = await page.evaluate(async () => { const value = await window.__round2Blob.text(); URL.createObjectURL = window.__round2CreateObjectURL; delete window.__round2Blob; delete window.__round2CreateObjectURL; return JSON.parse(value) })
  assert.equal(exported.item[0].request.auth.type, 'bearer')
  assert.equal(exported.item[0].request.url.query.length, 5)
  exported.info.name = 'Round2 roundtrip'
  const exportPath = join(root, 'round2-export.json'); writeFileSync(exportPath, JSON.stringify(exported))
  await page.evaluate(async path => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const result = await ipc.postmanImportWorkspace(path, crypto.randomUUID())
    const workspace = useWorkspaceStore(); await workspace.load()
    await workspace.openRequest(workspace.nodes.find(node => node.parentId === result.collectionId && node.kind === 'request'), false)
  }, exportPath)
  const replay = await send()
  assert.equal(replay.headers.authorization, original.headers.authorization)
  assert.equal(replay.url, original.url); assert.equal(replay.body, original.body)

  const badPath = join(root, 'round2-invalid.json')
  writeFileSync(badPath, JSON.stringify({ info: { name: 'Round2 failed import' }, item: [
    { name: 'valid first', request: { url: base + '/round2/echo' } }, { name: 'x'.repeat(121), request: { url: base + '/round2/echo' } },
  ] }))
  const failure = await page.evaluate(async path => {
    const { ipc } = await import('/src/ipc/index.ts'); const before = await ipc.workspaceLoad(); let rejected = false
    try { await ipc.postmanImportWorkspace(path, crypto.randomUUID()) } catch { rejected = true }
    return { rejected, before, after: await ipc.workspaceLoad() }
  }, badPath)
  assert.equal(failure.rejected, true); assert.deepEqual(failure.after, failure.before)
  const foldersPath = join(root, 'round2-folders.json')
  const request = name => ({ name, request: { url: base + '/round2/echo' } })
  writeFileSync(foldersPath, JSON.stringify({ info: { name: 'Round2 folders' }, item: [
    { name: 'A/B', item: [request('slash')] }, { name: 'A', item: [{ name: 'B', item: [request('nested')] }] },
    { name: 'Same', item: [request('same-one')] }, { name: 'Same', item: [request('same-two')] },
  ] }))
  const parents = await page.evaluate(async path => {
    const { ipc } = await import('/src/ipc/index.ts'); try { await ipc.postmanImportWorkspace(path, crypto.randomUUID()) } catch (error) { throw new Error('Round2 folder import: ' + JSON.stringify(error)) }
    const snapshot = await ipc.workspaceLoad()
    return Object.fromEntries(snapshot.nodes.filter(node => ['slash', 'nested', 'same-one', 'same-two'].includes(node.name)).map(node => [node.name, node.parentId]))
  }, foldersPath)
  assert.notEqual(parents.slash, parents.nested); assert.notEqual(parents['same-one'], parents['same-two'])

  console.log('E2E Round2: Body recovery, auth/query roundtrip, import rollback and folder identities passed')
  async function openCookie(path, settings = { sendCookies: true }, headers = []) {
    await page.evaluate(async ({ url, settings, headers }) => (await import('/src/stores/tabs.ts')).useTabsStore().open({ title: 'Round2 cookies', url, settings, headers }), { url: base + path, settings, headers })
    return send()
  }
  await openCookie('/round2/set-cookie')
  assert.match((await openCookie('/round2/needs-cookie')).headers.cookie, /round2_session=fixture/)
  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: '管理 Cookie 会话', exact: true }).click()
  const manager = page.locator('.app-modal').filter({ hasText: '工作区 Cookie 会话' })
  await manager.getByRole('button', { name: '显示值', exact: true }).click()
  await manager.getByRole('row').filter({ hasText: 'round2_session' }).getByRole('button', { name: '编辑', exact: true }).click()
  await manager.getByRole('textbox', { name: 'Set-Cookie 内容', exact: true }).fill('round2_session=edited; Path=/; HttpOnly')
  await manager.getByRole('button', { name: '保存 Cookie', exact: true }).click()
  await manager.getByText('edited', { exact: true }).waitFor()
  await manager.getByRole('button', { name: '完成', exact: true }).click()
  assert.match((await openCookie('/round2/needs-cookie')).headers.cookie, /round2_session=edited/)
  assert.equal((await openCookie('/round2/needs-cookie', { sendCookies: false })).headers.cookie, undefined)
  assert.equal((await openCookie('/round2/needs-cookie', { sendCookies: true }, [{ key: 'Cookie', value: 'manual=winner', enabled: true }])).headers.cookie, 'manual=winner')
  await openCookie('/round2/redirect')
  assert.match(received.at(-1).headers.cookie, /redirected=fixture/)
  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Settings', exact: true }).click()
  await page.getByRole('button', { name: '管理 Cookie 会话', exact: true }).click()
  await manager.getByRole('row').filter({ hasText: 'round2_session' }).getByRole('button', { name: '删除', exact: true }).click()
  await manager.getByText('round2_session', { exact: true }).waitFor({ state: 'hidden' })
  await manager.getByRole('button', { name: '清空会话', exact: true }).click()
  await manager.getByText('当前会话没有 Cookie。', { exact: true }).waitFor()
  await manager.getByRole('button', { name: '完成', exact: true }).click()
  assert.equal((await openCookie('/round2/needs-cookie')).headers.cookie, undefined)
  console.log('E2E Round2 PASS: saved Body drafts; real sidebar auth + mixed-query export/reimport; import rollback and folder identities; Cookie receive/send, redirect, manual priority, disable and manager edit/delete/clear')
}
