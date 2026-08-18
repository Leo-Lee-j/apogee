#!/usr/bin/env node
/**
 * Desktop smoke test for the workspace lifecycle.
 *
 * The app is launched with an isolated data root and WebView2 remote debugging;
 * playwright-core connects to the real Tauri WebView (no browser mock).
 */
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { spawn, execFileSync } from 'node:child_process'
import { chromium } from 'playwright-core'

const external = process.env.APOGEE_E2E_EXTERNAL === '1'
const root = external ? resolve(process.env.APOGEE_DATA_DIR ?? '.apogee-e2e') : mkdtempSync(join(tmpdir(), 'apogee-e2e-'))
const exe = resolve('target/debug/apogee.exe')
const port = 9225
const child = external ? null : spawn(exe, [], {
  cwd: resolve('.'),
  env: {
    ...process.env,
    APOGEE_DATA_DIR: root,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}`,
  },
  stdio: ['ignore', 'pipe', 'pipe'],
})
let childStderr = ''
child?.stderr?.on('data', (chunk) => { childStderr += String(chunk) })

let browser
try {
  const endpoint = `http://127.0.0.1:${port}`
  const deadline = Date.now() + 30_000
  while (Date.now() < deadline) {
    try {
      browser = await chromium.connectOverCDP(endpoint)
      break
    } catch {
      if (child && child.exitCode !== null) throw new Error(`Apogee exited before CDP became ready (code ${child.exitCode})`)
      await new Promise((resolvePromise) => setTimeout(resolvePromise, 250))
    }
  }
  if (!browser) throw new Error(`WebView2 remote debugging endpoint did not become ready${childStderr ? `; stderr: ${childStderr}` : ''}`)

  const context = browser.contexts()[0]
  const page = context.pages()[0]
  if (!page) throw new Error('Tauri main WebView page not found')

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

  console.log('E2E PASS: launcher → workspace A → create B → top-bar switch back to A')
} catch (error) {
  console.error(`E2E FAIL: ${error instanceof Error ? error.stack ?? error.message : String(error)}`)
  process.exitCode = 1
} finally {
  await browser?.close().catch(() => {})
  if (child && process.platform === 'win32') {
    try { execFileSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' }) } catch { /* already exited */ }
  } else if (child) {
    child.kill('SIGTERM')
  }
  if (!external) rmSync(root, { recursive: true, force: true })
}
