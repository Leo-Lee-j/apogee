#!/usr/bin/env node
import { round2 } from './e2e-round2.mjs'
import { round3 } from './e2e-round3.mjs'
import { round4 } from './e2e-round4.mjs'
/**
 * Desktop smoke test for the workspace lifecycle.
 *
 * The app is launched with an isolated data root and a test-only WebView2 CDP
 * port; playwright-core connects to the real Tauri WebView (no browser mock).
 */
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { spawn, execFileSync } from 'node:child_process'
import { createServer } from 'node:http'
import { chromium } from 'playwright-core'
import assert from 'node:assert/strict'

const external = process.env.APOGEE_E2E_EXTERNAL === '1'
const root = external ? resolve(process.env.APOGEE_DATA_DIR ?? '.apogee-e2e') : mkdtempSync(join(tmpdir(), 'apogee-e2e-'))
const exe = resolve('target/debug/apogee.exe')
const port = 9225
const externalAppPid = Number.parseInt(process.env.APOGEE_E2E_APP_PID ?? '', 10)
const child = external ? null : spawn(exe, [], {
  cwd: resolve('.'),
  env: {
    ...process.env,
    APOGEE_DATA_DIR: root,
    APOGEE_E2E_CDP_PORT: String(port),
    WEBVIEW2_USER_DATA_FOLDER: join(root, 'webview2'),
  },
  stdio: ['ignore', 'pipe', 'pipe'],
})
let childStderr = ''
child?.stderr?.on('data', (chunk) => { childStderr += String(chunk) })

function externalAppStatus() {
  if (!external || !Number.isInteger(externalAppPid) || externalAppPid <= 0) return ''
  try {
    process.kill(externalAppPid, 0)
    return `; Apogee PID ${externalAppPid} is still running`
  } catch {
    return `; Apogee PID ${externalAppPid} exited before CDP became ready`
  }
}

let browser
let workspacePage
let lastCdpError = ''
let receivedRequests = 0
let closedResponses = 0
const echoedRequests = []
// Deliberately leave responses open so cancellation exercises the real Rust HTTP task.
const slowServer = createServer((request, response) => {
  if (!request.url?.startsWith('/slow')) {
    const chunks = []
    request.on('data', (chunk) => chunks.push(chunk))
    request.on('end', () => {
      echoedRequests.push({ url: request.url, headers: request.headers, body: Buffer.concat(chunks).toString() })
      if (request.url === '/round2/set-cookie') response.setHeader('Set-Cookie', 'round2_session=fixture; Path=/; HttpOnly')
      if (request.url === '/round2/redirect') { response.statusCode = 302; response.setHeader('Set-Cookie', 'redirected=fixture; Path=/'); response.setHeader('Location', '/round2/needs-cookie') }
      if (request.url === '/round4/token') { response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ access_token: 'round4-synthetic-access', refresh_token: 'round4-synthetic-refresh', token_type: 'Bearer', expires_in: 3600 })); return }
      response.setHeader('Content-Type', 'application/json')
      response.end(request.url?.startsWith('/tree')
        ? JSON.stringify([...Array.from({ length: 200 }, (_, i) => i), { nested: Array.from({ length: 401 }, (_, i) => i) }])
        : '{"ok":true}')
    })
    return
  }
  receivedRequests += 1
  response.on('close', () => { closedResponses += 1 })
})

async function waitForCount(read, expected) {
  const deadline = Date.now() + 10_000
  while (read() < expected) {
    if (Date.now() >= deadline) throw new Error(`Local HTTP count did not reach ${expected}; got ${read()}`)
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 25))
  }
}

try {
  const endpoint = `http://127.0.0.1:${port}`
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      browser = await chromium.connectOverCDP(endpoint)
      break
    } catch (error) {
      lastCdpError = error instanceof Error ? error.message : String(error)
      if (child && child.exitCode !== null) throw new Error(`Apogee exited before CDP became ready (code ${child.exitCode})`)
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 250))
    }
  }
  if (!browser) {
    throw new Error(
      `WebView2 remote debugging endpoint did not become ready${lastCdpError ? `; last CDP error: ${lastCdpError}` : ''}${externalAppStatus()}${childStderr ? `; stderr: ${childStderr}` : ''}`,
    )
  }

  // CDP can accept connections before WebView2 creates its first page on a
  // cold runner. Wait for the page as well as the debugging endpoint.
  const pageDeadline = Date.now() + 30_000
  let page
  while (!page && Date.now() < pageDeadline) {
    page = browser.contexts().flatMap(context => context.pages())[0]
    if (!page) await new Promise(resolvePromise => setTimeout(resolvePromise, 100))
  }
  if (!page) throw new Error(`Tauri main WebView page did not become ready${externalAppStatus()}`)
  workspacePage = page
  page.on('pageerror', error => console.error(`WebView error: ${error.message}`))

  await page.waitForSelector('.launcher', { timeout: 30_000 })
  await page.getByRole('button', { name: '更多工作区操作', exact: true }).first().click()
  await page.getByText('在文件管理器中显示', { exact: true }).waitFor()
  await page.keyboard.press('Escape')
  await page.getByText('在文件管理器中显示', { exact: true }).waitFor({ state: 'hidden' })
  await page.getByText('示例工作区', { exact: true }).click()
  await page.waitForSelector('.titlebar', { timeout: 30_000 })

  // Create a second workspace through the real top-bar interaction.
  await page.getByRole('button', { name: '切换工作区' }).click()
  await page.getByText('新增工作区', { exact: true }).click()
  await page.getByPlaceholder('例如：用户服务').fill('E2E 第二工作区')
  await page.getByRole('button', { name: '创建', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('.titlebar')?.textContent?.includes('E2E 第二工作区'))

  // Switch back via the intentional top-bar switcher and assert the UI follows.
  await page.getByRole('button', { name: '切换工作区' }).click()
  await page.getByText('示例工作区', { exact: true }).click()
  await page.waitForFunction(() => document.querySelector('.titlebar')?.textContent?.includes('示例工作区'))

  await new Promise((resolvePromise, reject) => {
    slowServer.once('error', reject)
    slowServer.listen(0, '127.0.0.1', resolvePromise)
  })
  const address = slowServer.address()
  if (!address || typeof address === 'string') throw new Error('Local HTTP server did not bind')
  await page.getByRole('textbox', { name: '请求地址', exact: true }).fill(`http://127.0.0.1:${address.port}/slow`)

  await page.getByRole('button', { name: '发送请求', exact: true }).click()
  await waitForCount(() => receivedRequests, 1)
  await page.getByRole('button', { name: '取消请求', exact: true }).click()
  await waitForCount(() => closedResponses, 1)

  await page.getByRole('button', { name: '发送请求', exact: true }).click()
  await waitForCount(() => receivedRequests, 2)
  await page.getByRole('textbox', { name: '请求地址', exact: true }).focus()
  await page.keyboard.press('Escape')
  await waitForCount(() => closedResponses, 2)
  await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()

  await page.getByRole('textbox', { name: '请求地址', exact: true }).fill(`http://127.0.0.1:${address.port}/echo`)
  await page.getByRole('tab', { name: 'Params', exact: true }).click()
  await page.getByRole('button', { name: '批量编辑', exact: true }).click()
  await page.getByRole('textbox', { name: '批量编辑内容' }).fill('')
  await page.getByRole('button', { name: '完成', exact: true }).click()
  const query = page.locator('.parameter-group').filter({ hasText: 'Query 参数' })
  await query.getByRole('textbox', { name: '参数名', exact: true }).last().pressSequentially('q')
  await query.getByRole('textbox', { name: '参数值', exact: true }).first().fill('hello & world')
  await page.keyboard.press('Control+Enter')
  await waitForCount(() => echoedRequests.length, 1)
  assert.equal(echoedRequests[0].url, '/echo?q=hello%20%26%20world', 'new row must be sent without blur')
  await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()

  await query.getByRole('textbox', { name: '参数值', exact: true }).first().fill('')
  await query.getByRole('textbox', { name: '参数名', exact: true }).first().fill('')
  assert.equal(await query.locator('.parameter-row:not(.parameter-row--draft)').count(), 1, 'clearing cells must preserve the row')
  await query.getByRole('textbox', { name: '参数名', exact: true }).first().fill('again')
  await query.getByRole('textbox', { name: '参数说明', exact: true }).first().fill('保留说明')
  await page.getByRole('button', { name: '批量编辑', exact: true }).click()
  await page.getByRole('textbox', { name: '批量编辑内容' }).fill('again: updated\n// disabled: omit\nagain: second')
  await page.getByRole('button', { name: '完成', exact: true }).click()
  assert.equal(await query.getByRole('textbox', { name: '参数说明', exact: true }).first().inputValue(), '保留说明')

  await page.getByRole('tab', { name: 'Headers', exact: true }).click()
  const header = page.locator('.parameter-group').filter({ hasText: '请求 Headers' })
  await header.getByRole('textbox', { name: '参数名', exact: true }).last().pressSequentially('Accept')
  // Read the live input value; Vue need not mirror it into an HTML value attribute.
  const names = await header.getByRole('textbox', { name: '参数名', exact: true }).evaluateAll((inputs) => inputs.map((input) => input.value))
  const acceptIndex = names.indexOf('Accept')
  assert.ok(acceptIndex >= 0, 'an explicitly edited common header must stay visible')
  await header.getByRole('textbox', { name: '参数值', exact: true }).nth(acceptIndex).fill('application/x-e2e')
  await page.keyboard.press('Control+Enter')
  await waitForCount(() => echoedRequests.length, 2)
  assert.equal(echoedRequests[1].headers.accept, 'application/x-e2e')
  assert.equal(echoedRequests[1].url, '/echo?again=updated&again=second')
  await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()

  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Body', exact: true }).click()
  await page.locator('.body-types').getByText('x-www-form-urlencoded', { exact: true }).click()
  const form = page.locator('.body-parameter-table')
  await form.getByRole('textbox', { name: '参数名', exact: true }).last().pressSequentially('message')
  await form.getByRole('textbox', { name: '参数值', exact: true }).first().fill('中文 + &')
  await page.keyboard.press('Control+Enter')
  await waitForCount(() => echoedRequests.length, 3)
  assert.equal(echoedRequests[2].body, 'message=%E4%B8%AD%E6%96%87%20%2B%20%26')
  assert.equal(echoedRequests[2].headers['content-type'], 'application/x-www-form-urlencoded')
  await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()

  await page.locator('.body-types').getByText('form-data', { exact: true }).click()
  assert.equal(await form.getByRole('textbox', { name: '参数值', exact: true }).first().inputValue(), '中文 + &', 'form conversion must retain existing text')
  await form.locator('.parameter-row--draft .n-select').click()
  await page.locator('.n-base-select-option').getByText('文件', { exact: true }).click()
  assert.equal(await form.locator('.parameter-row:not(.parameter-row--draft)').count(), 2, 'selecting file kind must add exactly one row')
  const uploadPath = join(root, 'parameter-upload.txt')
  writeFileSync(uploadPath, 'e2e file content')
  await form.getByRole('textbox', { name: '参数名', exact: true }).nth(1).fill('upload')
  await form.getByRole('textbox', { name: '参数值', exact: true }).nth(1).fill(uploadPath)
  await page.keyboard.press('Control+Enter')
  await waitForCount(() => echoedRequests.length, 4)
  assert.match(echoedRequests[3].headers['content-type'], /^multipart\/form-data; boundary=/)
  assert.ok(echoedRequests[3].body.includes('e2e file content'), 'multipart must include the file content')
  assert.ok(echoedRequests[3].body.includes('中文 + &'), 'multipart must preserve the converted text field')
  await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()
  await page.locator('.body-types').getByText('x-www-form-urlencoded', { exact: true }).click()
  assert.equal(await form.locator('.n-select').count(), 0, 'switching back restores the independent urlencoded draft')
  assert.equal(await form.getByRole('textbox', { name: '参数值', exact: true }).first().inputValue(), '中文 + &')
  await page.locator('.body-types').getByText('form-data', { exact: true }).click()
  assert.equal(await form.locator('.n-select').count(), 3, 'switching back to multipart restores file rows')

  await page.getByRole('textbox', { name: '请求地址', exact: true }).fill(`http://127.0.0.1:${address.port}/:id/{name}?copy=:ignored`)
  await page.getByRole('tablist', { name: '请求编辑项' }).getByRole('tab', { name: 'Params', exact: true }).click()
  const path = page.locator('.parameter-group').filter({ hasText: 'Path 参数' })
  assert.equal(await path.locator('.parameter-row').count(), 2)
  assert.equal(await path.getByRole('textbox', { name: '参数名', exact: true }).first().getAttribute('readonly'), '')
  assert.equal(await path.getByRole('button', { name: '删除参数', exact: true }).count(), 0)
  await path.getByRole('textbox', { name: '参数值', exact: true }).nth(0).fill('42')
  await path.getByRole('textbox', { name: '参数值', exact: true }).nth(1).fill('hello world')

  const dragHandles = query.getByRole('button', { name: '拖动调整参数顺序', exact: true })
  await dragHandles.nth(2).focus()
  await page.keyboard.press('Alt+ArrowUp')
  assert.equal(await query.getByRole('textbox', { name: '参数值', exact: true }).nth(1).inputValue(), 'second')
  // Editing cells can scroll the table horizontally on a narrow WebView. Bring
  // the handle itself into view; a partially visible row does not reveal it.
  await dragHandles.first().scrollIntoViewIfNeeded()
  await dragHandles.first().hover()
  const handleBox = await dragHandles.first().boundingBox()
  const targetBox = await query.locator('.parameter-row').nth(1).boundingBox()
  assert.ok(handleBox && targetBox)
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2)
  await page.mouse.down()
  await page.mouse.move(handleBox.x + handleBox.width / 2, targetBox.y + targetBox.height - 2, { steps: 12 })
  await page.waitForTimeout(200)
  await page.mouse.up()
  await page.waitForFunction(() => {
    const group = [...document.querySelectorAll('.parameter-group')].find((element) => element.textContent.includes('Query 参数'))
    return group?.querySelector('input[aria-label="参数值"]')?.value === 'second'
  })
  await page.getByRole('button', { name: '发送请求', exact: true }).click()
  await waitForCount(() => echoedRequests.length, 5)
  assert.equal(echoedRequests[4].url, '/42/hello%20world?copy=:ignored&again=second&again=updated', 'drag order must match the transmitted data')

  // Native close must be intercepted before WebView destruction. Cancel retains the editor.
  await page.getByRole('button', { name: '关闭', exact: true }).click()
  await page.getByText('有未保存的编辑', { exact: true }).waitFor()
  await page.getByRole('button', { name: '取消', exact: true }).click()
  assert.equal(await page.getByRole('textbox', { name: '请求地址', exact: true }).inputValue(), `http://127.0.0.1:${address.port}/:id/{name}?copy=:ignored`)
  // Keep the draft when switching away; restore it from the workspace-local recovery file.
  await page.getByRole('button', { name: '切换工作区' }).click()
  await page.getByText('E2E 第二工作区', { exact: true }).click()
  await page.getByRole('button', { name: '保存配置与草稿', exact: true }).click()
  await page.waitForFunction(() => document.querySelector('.titlebar')?.textContent?.includes('E2E 第二工作区'))
  await page.getByRole('button', { name: '切换工作区' }).click()
  await page.getByText('示例工作区', { exact: true }).click()
  await page.waitForFunction(() => document.querySelector('input[aria-label="请求地址"]')?.value.includes('/:id/{name}'))
  assert.equal(await page.getByRole('textbox', { name: '请求地址', exact: true }).inputValue(), `http://127.0.0.1:${address.port}/:id/{name}?copy=:ignored`)

  await page.keyboard.press('Control+t')
  await page.getByRole('textbox', { name: '请求地址', exact: true }).fill(`http://127.0.0.1:${address.port}/tree`)
  await page.getByRole('button', { name: '发送请求', exact: true }).click()
  await page.getByRole('tab', { name: '树形', exact: true }).click()
  const tree = page.getByLabel('JSON 树', { exact: true })
  await tree.getByText('1–200 / 201 项', { exact: true }).waitFor()
  assert.ok(await tree.locator('.row').count() < 60, 'JSON tree must mount only viewport rows')
  await tree.getByRole('button', { name: '下一页', exact: true }).click()
  await tree.getByRole('button', { name: '展开 200', exact: true }).click()
  await tree.getByRole('button', { name: '展开 nested', exact: true }).click()
  await tree.getByText('1–200 / 401 项', { exact: true }).waitFor()
  await tree.getByLabel('跳转页码').last().fill('3')
  await tree.getByLabel('跳转页码').last().press('Enter')
  await tree.getByText('401–401 / 401 项', { exact: true }).waitFor({ timeout: 5000 }).catch(async error => {
    throw new Error(`${error.message}; tree: ${await tree.innerText()}`)
  })
  await tree.locator('.key').getByText('400', { exact: true }).waitFor()

  const postmanPath = join(root, 'body-fixtures.postman_collection.json')
  writeFileSync(postmanPath, JSON.stringify({ info: { name: 'Body fidelity' }, item: [
    { name: 'Multipart', request: { method: 'POST', url: `http://127.0.0.1:${address.port}/import-form`, body: { mode: 'formdata', formdata: [
      { key: 'text', value: '中文 + &' }, { key: 'upload', type: 'file', src: uploadPath }, { key: 'ignored', value: 'omit', disabled: true },
    ] } } },
    { name: 'Binary', request: { method: 'POST', url: `http://127.0.0.1:${address.port}/import-binary`, body: { mode: 'file', file: { src: uploadPath } } } },
    { name: 'GraphQL', request: { method: 'POST', url: `http://127.0.0.1:${address.port}/import-graphql`, body: { mode: 'graphql', graphql: { query: 'query Q { ok }', variables: '{"id":42}', operationName: 'Q' } } } },
  ] }))
  const importedEchoStart = echoedRequests.length
  const migration = await page.evaluate(async path => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const result = await ipc.postmanImportWorkspace(path, crypto.randomUUID())
    const workspace = useWorkspaceStore()
    await workspace.load()
    const ids = workspace.nodes.filter(node => node.parentId === result.collectionId && node.kind === 'request').map(node => node.id)
    const exported = JSON.parse((await ipc.workspaceExportPostman(result.collectionId)).json).item
    return { result, ids, exported }
  }, postmanPath)
  assert.equal(migration.result.importedRequests, 3)
  assert.ok(migration.result.warnings.some(warning => warning.includes('Multipart') && warning.includes('文件')))
  assert.ok(migration.result.warnings.some(warning => warning.includes('Binary') && warning.includes('文件')))
  const exportedPath = join(root, 'body-roundtrip.postman_collection.json')
  writeFileSync(exportedPath, JSON.stringify({ info: { name: 'Body roundtrip' }, item: migration.exported }))
  const roundtrip = await page.evaluate(async path => {
    const { ipc } = await import('/src/ipc/index.ts')
    const result = await ipc.postmanImportWorkspace(path, crypto.randomUUID())
    const snapshot = await ipc.workspaceLoad()
    return Promise.all(snapshot.nodes.filter(node => node.parentId === result.collectionId && node.kind === 'request').map(async node => {
      const request = await ipc.workspaceReadRequest(node.id)
      return { id: node.id, name: request.name, spec: request.spec }
    }))
  }, exportedPath)
  for (const id of [...migration.ids, ...roundtrip.map(request => request.id)]) {
    await page.evaluate(async requestId => {
      const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
      await useWorkspaceStore().openRequest({ id: requestId, kind: 'request' }, false)
    }, id)
    await page.getByRole('button', { name: '发送请求', exact: true }).click()
    await page.getByRole('button', { name: '发送请求', exact: true }).waitFor()
  }
  await waitForCount(() => echoedRequests.length, importedEchoStart + 6)
  const transmissions = echoedRequests.slice(importedEchoStart)
  for (const suffix of ['form', 'binary', 'graphql']) {
    const matches = transmissions.filter(request => request.url === `/import-${suffix}`)
    assert.equal(matches.length, 2, 'import and export/reimport must both reach the receiver')
    if (suffix === 'form') {
      for (const request of matches) { assert.ok(request.body.includes('e2e file content')); assert.ok(request.body.includes('中文 + &')); assert.ok(!request.body.includes('omit')) }
    } else if (suffix === 'binary') {
      for (const request of matches) assert.equal(request.body, 'e2e file content')
    } else {
      for (const request of matches) assert.deepEqual(JSON.parse(request.body), { query: 'query Q { ok }', variables: { id: 42 }, operationName: 'Q' })
    }
  }
  const duplication = await page.evaluate(async collectionId => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const workspace = useWorkspaceStore()
    const original = await ipc.workspaceReadContainer(collectionId)
    await ipc.workspaceSaveContainer({ ...original, auth: { type: 'bearer', token: 'copy-fixture' }, settings: { timeoutMs: 4321 } }, original.auth)
    const source = workspace.nodes.find(node => node.parentId === collectionId && node.kind === 'request')
    const request = await ipc.workspaceReadRequest(source.id)
    await ipc.workspaceSaveRequest(request.id, request.name, { ...request.spec, auth: { type: 'inherit' } }, request.spec.auth)
    const sourceCase = await ipc.workspaceCreateDebugCase(request.id, 'copy case', 'success', request.spec)
    await workspace.duplicateNode(workspace.nodes.find(node => node.id === collectionId))
    const copied = workspace.nodes.find(node => node.name === 'Body fidelity 副本')
    const copiedRequest = workspace.nodes.find(node => node.parentId === copied.id && node.name === request.name)
    const config = await ipc.workspaceReadContainer(copied.id)
    const cases = await ipc.workspaceListDebugCases(copiedRequest.id)
    return { config, copiedRequest, cases, sourceCase }
  }, migration.result.collectionId)
  assert.equal(duplication.config.auth.token, 'copy-fixture')
  assert.equal(duplication.config.settings.timeoutMs, 4321)
  assert.equal(duplication.cases.length, 1)
  assert.notEqual(duplication.cases[0].id, duplication.sourceCase.id)
  assert.equal(duplication.cases[0].requestId, duplication.copiedRequest.id)
  await page.evaluate(async container => {
    const { useTabsStore } = await import('/src/stores/tabs.ts')
    useTabsStore().openContainerConfig(container)
  }, duplication.config)
  const timeout = page.locator('.container-config .workspace-field').filter({ hasText: '超时（ms）' }).locator('input').first()
  await timeout.fill('9876')
  await timeout.press('Tab')
  await page.getByText('未保存修改', { exact: true }).waitFor()
  await page.keyboard.press('Control+t')
  await page.getByRole('tab', { name: `配置 · ${duplication.config.name}`, exact: false }).click()
  assert.equal(await timeout.inputValue(), '9876', 'container edits must survive page unmount and remount')
  await page.locator('.container-config').getByRole('button', { name: '保存', exact: true }).click()
  await page.getByText('未保存修改', { exact: true }).waitFor({ state: 'hidden' })

  await round2(page, root, 'http://127.0.0.1:' + address.port, echoedRequests)
  await round3(page, root, 'http://127.0.0.1:' + address.port, echoedRequests)
  await round4(page, root, 'http://127.0.0.1:' + address.port, echoedRequests)
  console.log('E2E PASS: workspace lifecycle and draft recovery; native close cancellation; container editor retention; atomic copy via IPC; virtual JSON tree and nested paging; Postman body transmission and export/reimport; cancellation; immediate Query/Header/form edits; blank rows; bulk metadata and duplicate/disabled params; form conversion and multipart upload; Path constraints; keyboard and drag ordering')
} catch (error) {
  console.error(`E2E FAIL: ${error instanceof Error ? error.stack ?? error.message : String(error)}`)
  process.exitCode = 1
} finally {
  // Remove vault references through the real repository before the test directory is removed.
  if (workspacePage) {
    try {
      await workspacePage.evaluate(async () => {
        const { ipc } = await import('/src/ipc/index.ts')
        const snapshot = await ipc.workspaceLoad()
        for (const node of snapshot.nodes.filter(n => n.kind === 'collection')) await ipc.workspaceDeleteNode(node.id)
        for (const environment of snapshot.environments) await ipc.workspaceDeleteEnvironment(environment.id)
        await ipc.workspaceSaveGlobalVariables([])
      })
    } catch (error) { console.error('E2E vault cleanup failed:', error); process.exitCode = 1 }
  }
  slowServer.closeAllConnections()
  slowServer.close()
  await browser?.close().catch(() => {})
  if (child && process.platform === 'win32') {
    try { execFileSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' }) } catch { /* already exited */ }
  } else if (child) {
    child.kill('SIGTERM')
  }
  if (!external) rmSync(root, { recursive: true, force: true })
}
