#!/usr/bin/env node
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
      response.setHeader('Content-Type', 'application/json')
      response.end('{"ok":true}')
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

  await page.waitForSelector('.launcher', { timeout: 30_000 })
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
  assert.equal(await form.locator('.n-select').count(), 3, 'unsupported conversion must retain file rows and form-data type')

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

  console.log('E2E PASS: workspace lifecycle; cancellation; immediate Query/Header/form edits; blank rows; bulk metadata and duplicate/disabled params; form conversion and multipart upload; Path constraints; keyboard and drag ordering')
} catch (error) {
  console.error(`E2E FAIL: ${error instanceof Error ? error.stack ?? error.message : String(error)}`)
  process.exitCode = 1
} finally {
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
