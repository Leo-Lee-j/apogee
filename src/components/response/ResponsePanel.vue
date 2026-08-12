<script setup lang="ts">
import { ref, computed, defineAsyncComponent, watch } from 'vue'
import { fetchResponseRange, responseUrl, type AppError } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import AppIcon from '@/components/ui/AppIcon.vue'
import AppTooltip from '@/components/ui/AppTooltip.vue'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { IconCode, IconCopy, IconList, IconPreview, IconTime } from '@/components/ui/icons'

/**
 * 响应区 —— 性能设计的核心战场（02 文档 §4.4）。
 *
 * 首屏只渲染状态行和 Tab 结构。真正的查看器（CodeMirror / 虚拟树）
 * **全部动态 import**，理由见 03 文档 §4.1：CodeMirror 不能进首屏 chunk。
 */

const CodeMirrorView = defineAsyncComponent(() => import('./CodeMirrorView.vue'))
const JsonTreeView = defineAsyncComponent(() => import('./JsonTreeView.vue'))

type Section = 'body' | 'headers' | 'cookies' | 'timing' | 'actual'
type BodyView = 'pretty' | 'raw' | 'tree' | 'preview'
type CodeTarget = 'windows' | 'powershell' | 'posix'
const section = ref<Section>('body')
const bodyView = ref<BodyView>('pretty')
const codeTarget = ref<CodeTarget>('windows')
const copyState = ref('')

const tabs = useTabsStore()
const resp = computed(() => tabs.active?.response ?? null)
const requestError = computed<AppError | null>(() => tabs.active?.error ?? null)
const headers = computed(() => resp.value?.headers ?? [])
const cookies = computed(() => headers.value
  .filter(([name]) => name.toLowerCase() === 'set-cookie')
  .map(([, value], index) => {
    const [pair = '', ...attributes] = value.split(';').map((part) => part.trim())
    const separator = pair.indexOf('=')
    return {
      id: index,
      name: separator >= 0 ? pair.slice(0, separator) : pair,
      value: separator >= 0 ? pair.slice(separator + 1) : '',
      attributes: attributes.join('; '),
    }
  }))
const isJson = computed(() => resp.value?.contentType?.toLowerCase().includes('json') ?? false)
const canPreview = computed(() => {
  const contentType = resp.value?.contentType?.toLowerCase() ?? ''
  return contentType.includes('html') || contentType.startsWith('image/') || contentType.includes('svg')
})
const previewSrc = computed(() => resp.value ? responseUrl(resp.value.id) : '')
const actualRequest = computed(() => resp.value?.actualRequest ?? null)

function decodeQueryPart(value: string) {
  try {
    return decodeURIComponent(value.replaceAll('+', ' '))
  } catch {
    return value
  }
}

const actualParams = computed(() => {
  const url = actualRequest.value?.url
  if (!url) return []
  const queryStart = url.indexOf('?')
  if (queryStart < 0) return []
  const hashStart = url.indexOf('#', queryStart)
  const query = url.slice(queryStart + 1, hashStart < 0 ? undefined : hashStart)
  return query.split('&').filter(Boolean).map((pair, index) => {
    const separator = pair.indexOf('=')
    const encodedName = separator < 0 ? pair : pair.slice(0, separator)
    const encodedValue = separator < 0 ? '' : pair.slice(separator + 1)
    return {
      id: index,
      name: decodeQueryPart(encodedName),
      value: decodeQueryPart(encodedValue),
      encoded: pair,
    }
  })
})

function quotePosix(value: string) {
  return `'${value.replaceAll("'", `'"'"'`)}'`
}

function quotePowerShell(value: string) {
  return `'${value.replaceAll("'", "''")}'`
}

function quoteCmd(value: string) {
  return `"${value.replaceAll('"', '\\"').replaceAll('%', '%%')}"`
}

const requestCode = computed(() => {
  const request = actualRequest.value
  if (!request) return ''
  const quote = codeTarget.value === 'posix' ? quotePosix : codeTarget.value === 'powershell' ? quotePowerShell : quoteCmd
  const continuation = codeTarget.value === 'posix' ? ' \\\n  ' : codeTarget.value === 'powershell' ? ' `\n  ' : ' ^\r\n  '
  const executable = codeTarget.value === 'windows' ? 'curl.exe' : 'curl'
  const parts = [executable, '--request', quote(request.method)]
  for (const [name, value] of request.headers) {
    parts.push('--header', quote(`${name}: ${value}`))
  }
  if (request.body != null) parts.push('--data-raw', quote(request.body))
  parts.push(quote(request.url))
  return parts.join(continuation)
})

const timingRows = computed(() => {
  if (!resp.value) return []
  const timing = resp.value.timing
  const detailed = timing.networkPhasesAvailable ?? false
  return [
    { label: '请求准备', value: timing.prepareMs ?? 0, note: '变量解析与请求组装' },
    { label: 'DNS 查询', value: detailed ? timing.dnsMs : null, note: detailed ? '' : '计入 TTFB' },
    { label: 'TCP 连接', value: detailed ? timing.tcpMs : null, note: detailed ? '' : '计入 TTFB / 可能复用' },
    { label: 'TLS 握手', value: detailed ? timing.tlsMs : null, note: detailed ? '' : '计入 TTFB / 可能复用' },
    { label: '发送请求', value: detailed ? timing.requestMs : null, note: detailed ? '' : '计入 TTFB' },
    { label: detailed ? '等待响应 (TTFB)' : '连接、发送及等待响应', value: timing.ttfbMs, note: detailed ? '' : 'TTFB' },
    { label: '下载响应', value: timing.downloadMs, note: '' },
  ]
})

watch(() => resp.value?.id, () => {
  section.value = 'body'
  bodyView.value = isJson.value && resp.value?.syntaxHighlight ? 'pretty' : 'raw'
  copyState.value = ''
}, { immediate: true })

/** 分级降级（02 文档 §4.4）。唯一的高亮阈值是 2MB。 */
const tierLabel: Record<string, string> = {
  full: '全功能',
  virtualized: '虚拟滚动',
  noHighlight: '已关闭高亮',
  summaryOnly: '仅摘要',
}

const statusClass = computed(() => {
  const s = resp.value?.status
  if (s == null) return resp.value?.timing ? 'ok' : ''
  if (s < 300) return 'ok'
  if (s < 400) return 'redirect'
  if (s < 500) return 'warn'
  return s < 600 ? 'error' : 'neterr'
})

function fmtBytes(n: number) {
  if (n < 1024) return `${n} B`
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1048576).toFixed(2)} MB`
}

function fmtMs(n: number) {
  return n < 1 ? `${n.toFixed(2)} ms` : `${n.toFixed(n < 10 ? 1 : 0)} ms`
}

async function copyResponse() {
  if (!resp.value) return
  try {
    let text = ''
    if (section.value === 'headers') {
      text = headers.value.map(([name, value]) => `${name}: ${value}`).join('\n')
    } else if (section.value === 'cookies') {
      text = cookies.value.map((cookie) => `${cookie.name}=${cookie.value}${cookie.attributes ? `; ${cookie.attributes}` : ''}`).join('\n')
    } else if (section.value === 'timing') {
      text = timingRows.value.map((row) => `${row.label}: ${row.value == null ? row.note : fmtMs(row.value)}`).join('\n')
    } else {
      if (resp.value.sizeBytes > 2 * 1024 * 1024) {
        copyState.value = '响应体超过 2 MB，请保存到文件'
        window.setTimeout(() => { copyState.value = '' }, 2200)
        return
      }
      text = resp.value.sizeBytes === 0
        ? ''
        : (await fetchResponseRange(resp.value.id, 0, resp.value.sizeBytes)).text
    }
    await navigator.clipboard.writeText(text)
    copyState.value = '已复制'
  } catch (error) {
    copyState.value = error instanceof Error ? error.message : '复制失败'
  }
  window.setTimeout(() => { copyState.value = '' }, 1800)
}

async function copyText(text: string, success = '已复制') {
  try {
    await navigator.clipboard.writeText(text)
    copyState.value = success
  } catch (error) {
    copyState.value = error instanceof Error ? error.message : '复制失败'
  }
  window.setTimeout(() => { copyState.value = '' }, 1800)
}
</script>

<template>
  <section class="resp">
    <NaiveThemeScope>
    <div class="statusline">
      <template v-if="requestError">
        <strong class="response-title">响应</strong>
        <span class="a-pill status neterr">请求失败</span>
        <span class="error-kind-inline">{{ requestError.kind }}</span>
      </template>
      <template v-else-if="resp">
        <nav class="sections" role="tablist" aria-label="响应信息">
          <button v-for="item in ([['body', '响应体', null], ['headers', 'Headers', headers.length], ['cookies', 'Cookies', cookies.length], ['timing', '耗时', null], ['actual', '实际请求', null]] as const)" :key="item[0]" type="button" role="tab" :aria-selected="section === item[0]" :class="{ on: section === item[0] }" @click="section = item[0]">
            {{ item[1] }}<span v-if="item[2] !== null" class="count-badge">{{ item[2] }}</span>
          </button>
        </nav>
        <span class="spacer" />
        <span :class="['a-pill', 'status', statusClass]">
          {{ resp.status != null ? `${resp.status} ${resp.statusText ?? ''}`.trim() : '状态不可用' }}
        </span>
        <span class="metric"><AppIcon :component="IconTime" :size="13" />{{ resp.timing.totalMs.toFixed(0) }} ms</span>
        <span class="metric">{{ fmtBytes(resp.sizeBytes) }}</span>
      </template>
      <span v-else class="hint">按 <kbd>Ctrl</kbd><kbd>Enter</kbd> 发送请求</span>
    </div>

    <div v-if="resp && !requestError" class="subbar">
      <div v-if="section === 'body'" class="body-views" role="tablist" aria-label="响应体视图">
        <button type="button" role="tab" :aria-selected="bodyView === 'pretty'" :disabled="!isJson || !resp.syntaxHighlight" :class="{ on: bodyView === 'pretty' }" @click="bodyView = 'pretty'"><AppIcon :component="IconCode" :size="14" />Body</button>
        <button type="button" role="tab" :aria-selected="bodyView === 'raw'" :class="{ on: bodyView === 'raw' }" @click="bodyView = 'raw'"><AppIcon :component="IconCode" :size="14" />原文</button>
        <button type="button" role="tab" :aria-selected="bodyView === 'tree'" :disabled="!isJson || !resp.indexed" :class="{ on: bodyView === 'tree' }" @click="bodyView = 'tree'"><AppIcon :component="IconList" :size="14" />树形</button>
        <button type="button" role="tab" :aria-selected="bodyView === 'preview'" :disabled="!canPreview" :class="{ on: bodyView === 'preview' }" @click="bodyView = 'preview'"><AppIcon :component="IconPreview" :size="14" />预览</button>
        <span class="format-label">{{ resp.contentType ?? '未知类型' }}<template v-if="resp.charset"> · {{ resp.charset }}</template></span>
      </div>
      <span v-else class="section-label">{{ { headers: '响应 Headers', cookies: '响应 Cookies', timing: '请求耗时明细', actual: '最终发送到网络的请求' }[section] }}</span>
      <span class="spacer" />
      <span v-if="copyState" class="copy-state" role="status">{{ copyState }}</span>
      <span v-if="section !== 'actual'" class="tier" :title="`渲染分级：${resp.tier}`">{{ tierLabel[resp.tier] }}{{ resp.syntaxHighlight ? '' : ' · 无高亮' }}</span>
      <AppTooltip v-if="section !== 'actual'">
        <template #trigger><button type="button" class="icon-button" aria-label="复制当前响应内容" @click="copyResponse"><AppIcon :component="IconCopy" :size="15" /></button></template>
        复制当前内容
      </AppTooltip>
    </div>

    <div class="pane">
      <div v-if="requestError" class="error-state" role="alert">
        <h3>请求未完成</h3>
        <p class="error-message">{{ requestError.message }}</p>
        <p class="error-kind">错误类型：{{ requestError.kind }}</p>
        <details v-if="requestError.detail" class="error-detail">
          <summary>技术详情</summary>
          <pre>{{ requestError.detail }}</pre>
        </details>
        <ul v-if="requestError.hints.length" class="hints">
          <li v-for="hint in requestError.hints" :key="`${hint.label}-${hint.action ?? ''}`">
            <span>{{ hint.label }}</span>
            <code v-if="hint.action">{{ hint.action }}</code>
          </li>
        </ul>
      </div>
      <template v-else-if="resp">
        <!-- >50MB：不渲染，只给摘要 + 出口（02 文档 §4.4 SummaryOnly 分级） -->
        <div v-if="section === 'body' && resp.tier === 'summaryOnly' && bodyView !== 'raw'" class="summary">
          <p class="big">{{ fmtBytes(resp.sizeBytes) }} 响应体</p>
          <p class="dim">超过 50 MB，不在界面内渲染。这是设计行为，不是失败。</p>
          <div class="actions">
            <button type="button" @click="bodyView = 'raw'">流式预览头部</button>
          </div>
        </div>
        <CodeMirrorView
          v-else-if="section === 'body' && (bodyView === 'pretty' || bodyView === 'raw')"
          :response-id="resp.id"
          :total-bytes="resp.sizeBytes"
          :highlight="bodyView === 'pretty' && resp.syntaxHighlight"
          :format-json="bodyView === 'pretty'"
        />
        <JsonTreeView v-else-if="section === 'body' && bodyView === 'tree'" :response-id="resp.id" />
        <iframe v-else-if="section === 'body' && bodyView === 'preview'" class="preview" :src="previewSrc" title="响应预览" sandbox="" />
        <div v-else-if="section === 'headers'" class="data-table" role="table" aria-label="响应 Headers">
          <div class="data-row data-head" role="row"><span>名称</span><span>值</span></div>
          <div v-for="([name, value], index) in headers" :key="`${name}-${index}`" class="data-row" role="row"><code>{{ name }}</code><span class="selectable">{{ value }}</span></div>
          <p v-if="!headers.length" class="empty-inline">没有响应 Header。</p>
        </div>
        <div v-else-if="section === 'cookies'" class="data-table cookie-table" role="table" aria-label="响应 Cookies">
          <div class="data-row data-head" role="row"><span>名称</span><span>值</span><span>属性</span></div>
          <div v-for="cookie in cookies" :key="cookie.id" class="data-row" role="row"><code>{{ cookie.name }}</code><span class="selectable">{{ cookie.value }}</span><span class="cookie-attrs">{{ cookie.attributes || '—' }}</span></div>
          <p v-if="!cookies.length" class="empty-inline">此响应没有设置 Cookie。</p>
        </div>
        <div v-else-if="section === 'timing'" class="timing-panel">
          <div v-for="row in timingRows" :key="row.label" class="timing-row" :class="{ unavailable: row.value == null }">
            <span>{{ row.label }}</span>
            <div class="timing-track"><span v-if="row.value != null" :style="{ width: `${Math.max(2, row.value / Math.max(resp.timing.totalMs, 1) * 100)}%` }" /></div>
            <code>{{ row.value == null ? row.note : fmtMs(row.value) }}</code>
          </div>
          <p class="timing-note">当前 HTTP 引擎只提供整体 TTFB；连接池复用时 DNS、TCP 和 TLS 阶段不会再次发生。</p>
          <div class="timing-total"><strong>总耗时</strong><strong>{{ fmtMs(resp.timing.totalMs) }}</strong></div>
        </div>
        <div v-else-if="actualRequest" class="actual-panel">
          <section class="actual-summary">
            <div class="actual-field">
              <span class="actual-label">请求类型</span>
              <span class="a-pill method-pill">{{ actualRequest.method }}</span>
            </div>
            <div class="actual-field actual-url-row">
              <span class="actual-label">请求 URL</span>
              <AppTooltip>
                <template #trigger>
                  <button type="button" class="copy-url a-selectable" @click="copyText(actualRequest.url, 'URL 已复制')">{{ actualRequest.url }}</button>
                </template>
                点击复制编码后的 URL
              </AppTooltip>
            </div>
          </section>

          <section class="actual-section">
            <header><strong>参数</strong><span>{{ actualParams.length }} 项</span></header>
            <div v-if="actualParams.length" class="actual-params">
              <div class="actual-param actual-param-head"><span>名称</span><span>值</span><span>实际编码</span></div>
              <div v-for="param in actualParams" :key="param.id" class="actual-param">
                <code>{{ param.name }}</code>
                <span class="a-selectable">{{ param.value }}</span>
                <code class="encoded-param a-selectable">{{ param.encoded }}</code>
              </div>
            </div>
            <p v-else class="actual-none">此请求没有 Query 参数。</p>
          </section>

          <section class="actual-section">
            <header><strong>Header</strong><span>{{ actualRequest.headers.length }} 项</span></header>
            <div class="actual-headers">
              <div class="actual-header actual-header-head"><span>名称</span><span>值</span></div>
              <div v-for="([name, value], index) in actualRequest.headers" :key="`${name}-${index}`" class="actual-header"><code>{{ name }}</code><span class="a-selectable">{{ value }}</span></div>
            </div>
          </section>

          <section class="actual-section code-section">
            <header class="code-head">
              <strong>请求代码</strong>
              <nav class="code-tabs" role="tablist" aria-label="cURL 平台">
                <button v-for="target in ([['windows', 'Windows CMD'], ['powershell', 'PowerShell'], ['posix', 'Linux / macOS']] as const)" :key="target[0]" type="button" role="tab" :aria-selected="codeTarget === target[0]" :class="{ on: codeTarget === target[0] }" @click="codeTarget = target[0]">{{ target[1] }}</button>
              </nav>
              <span class="spacer" />
              <AppTooltip>
                <template #trigger><button type="button" class="icon-button" aria-label="复制请求代码" @click="copyText(requestCode, '请求代码已复制')"><AppIcon :component="IconCopy" :size="15" /></button></template>
                复制当前 cURL
              </AppTooltip>
            </header>
            <pre class="request-code a-selectable"><code>{{ requestCode }}</code></pre>
          </section>
        </div>
        <div v-else class="empty actual-empty">此响应没有实际请求快照，请重新发送请求。</div>
      </template>
      <div v-else class="empty">
        <p>发出请求后，响应会显示在这里。</p>
        <p class="dim">大响应体会按体积自动降级渲染，不会卡住界面。</p>
      </div>
    </div>
    </NaiveThemeScope>
  </section>
</template>

<style scoped>
/* 没有 border-top —— 与请求区之间靠 --a-gutter 的间隙分隔（02 文档 §3.2） */
.resp { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
.statusline {
  height: 36px; flex: 0 0 auto;
  display: flex; align-items: center; gap: 8px; padding: 0 10px;
  background: var(--a-bg-2); border-bottom: 1px solid var(--a-border);
  font-size: 12px; color: var(--a-text-dim);
}
.response-title { color: var(--a-text); font-size: 13px; }
.error-kind-inline { color: var(--a-text-faint); font-family: var(--a-font-mono); }
.sections { display: flex; align-self: stretch; gap: 2px; }
.sections button {
  position: relative; display: inline-flex; align-items: center; gap: 5px;
  padding: 0 8px; border: 0; background: transparent; color: var(--a-text-dim);
  font: var(--a-fw-medium) 12px var(--a-font-ui); cursor: pointer;
}
.sections button::after { content: ''; position: absolute; height: 2px; left: 8px; right: 8px; bottom: 0; background: transparent; }
.sections button:hover { color: var(--a-text); }
.sections button.on { color: var(--a-text); }
.sections button.on::after { background: var(--a-primary); }
.count-badge { min-width: 16px; height: 16px; padding: 0 4px; border-radius: var(--a-radius-pill); background: var(--a-bg-3); color: var(--a-text-faint); font: 10px/16px var(--a-font-mono); }
/* 状态徽章。.a-pill 提供形态（见 tokens.css），这里只给语义色。
   网络错误刻意用实色填充而不是换色相：rose 和 red 在半透明 pill 里几乎分不开，
   靠份量差 + 图标区分更可靠（02 文档 §4.4）。 */
.status.ok { color: var(--a-ok); background: var(--a-ok-bg); }
.status.redirect { color: var(--a-redirect); background: var(--a-redirect-bg); }
.status.warn { color: var(--a-warn); background: var(--a-warn-bg); }
.status.error { color: var(--a-error); background: var(--a-error-bg); }
.status.neterr {
  color: var(--a-neterr);
  background: var(--a-neterr-bg);
  border-color: var(--a-neterr-bg);
}
/* 分隔符是纯装饰，不承载信息 */
.sep { color: var(--a-text-ghost); }
.tier { color: var(--a-text-faint); }
.hint kbd {
  font: var(--a-fs-xs) / 1 var(--a-font-mono); background: var(--a-bg-1);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); padding: 2px 4px; margin: 0 1px;
}
.spacer { flex: 1 1 auto; }
.metric { display: inline-flex; align-items: center; gap: 4px; color: var(--a-text-dim); font-family: var(--a-font-mono); white-space: nowrap; }
.subbar { height: 32px; flex: 0 0 auto; display: flex; align-items: center; gap: 8px; padding: 0 8px; border-bottom: 1px solid var(--a-border); background: var(--a-bg-2); font-size: 11px; color: var(--a-text-faint); }
.body-views { display: flex; align-items: center; gap: 2px; height: 100%; }
.body-views button { display: inline-flex; align-items: center; gap: 4px; height: 24px; padding: 0 7px; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); font: 11px var(--a-font-ui); cursor: pointer; }
.body-views button:hover:not(:disabled) { background: var(--a-hover); color: var(--a-text); }
.body-views button.on { background: var(--a-selected); color: var(--a-primary); }
.body-views button:disabled { opacity: .38; cursor: not-allowed; }
.format-label { margin-left: 6px; padding-left: 10px; border-left: 1px solid var(--a-border); font-family: var(--a-font-mono); }
.section-label { color: var(--a-text-dim); font-weight: var(--a-fw-medium); }
.copy-state { color: var(--a-primary); max-width: 230px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.icon-button { width: 24px; height: 24px; display: inline-flex; align-items: center; justify-content: center; padding: 0; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); cursor: pointer; }
.icon-button:hover { background: var(--a-hover); color: var(--a-text); }
.pane { flex: 1 1 auto; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
.error-state, .empty, .summary, .todo { padding: 16px; color: var(--a-text-dim); font-size: 12px; }
.empty { flex: 1; display: grid; place-content: center; justify-items: center; gap: 4px; text-align: center; background: color-mix(in srgb, var(--a-bg-1) 72%, transparent); }
.empty::before { content: '↗'; display: grid; place-items: center; width: 34px; height: 34px; margin-bottom: 8px; border: 1px solid var(--a-border); border-radius: 10px; color: var(--a-primary); font: 18px/1 var(--a-font-mono); background: var(--a-bg-2); }
.empty p { margin: 0; }
.error-state { overflow: auto; }
.error-state h3 { margin: 0 0 8px; color: var(--a-error); font-size: 14px; }
.error-message { margin: 0 0 6px; color: var(--a-text); font-size: 13px; }
.error-kind { margin: 0; color: var(--a-text-faint); font-family: var(--a-font-mono); }
.error-detail { margin-top: 12px; color: var(--a-text-dim); }
.error-detail summary { cursor: pointer; color: var(--a-text); }
.error-detail pre { margin: 6px 0 0; white-space: pre-wrap; font: 11px/1.5 var(--a-font-mono); }
.hints { margin: 14px 0 0; padding-left: 18px; color: var(--a-text-dim); }
.hints li { margin: 5px 0; }
.hints code { margin-left: 6px; color: var(--a-primary); font: 11px var(--a-font-mono); }
.dim { color: var(--a-text-faint); }
.big { font-size: 16px; color: var(--a-text); }
.actions { display: flex; gap: 8px; margin-top: 12px; }
.actions button {
  height: 26px; padding: 0 10px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text);
  font: inherit; cursor: pointer;
}
.actions button:hover { background: var(--a-hover); }
.preview { flex: 1 1 auto; width: 100%; min-height: 0; border: 0; background: #fff; }
.data-table { flex: 1 1 auto; min-height: 0; overflow: auto; font-size: var(--a-font-result); }
.data-row { min-height: 30px; display: grid; grid-template-columns: minmax(140px, 28%) minmax(260px, 1fr); border-bottom: 1px solid var(--a-border); }
.data-row > span, .data-row > code { display: flex; align-items: center; min-width: 0; padding: 5px 10px; border-right: 1px solid var(--a-border); overflow-wrap: anywhere; }
.data-row > :last-child { border-right: 0; }
.data-row code { color: var(--a-post); font: 11px/18px var(--a-font-mono); }
.data-head { position: sticky; top: 0; z-index: 1; min-height: 28px; background: var(--a-bg-3); color: var(--a-text-faint); font-weight: var(--a-fw-medium); }
.cookie-table .data-row { grid-template-columns: minmax(120px, 22%) minmax(160px, 34%) minmax(200px, 1fr); }
.cookie-attrs { color: var(--a-text-faint); font-family: var(--a-font-mono); font-size: 11px; }
.selectable { user-select: text; color: var(--a-text-soft); }
.empty-inline { margin: 0; padding: 18px 10px; color: var(--a-text-faint); }
.timing-panel { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 18px 20px; }
.timing-row { display: grid; grid-template-columns: 160px minmax(120px, 1fr) 142px; gap: 14px; align-items: center; min-height: 34px; color: var(--a-text-dim); font-size: var(--a-font-result); }
.timing-track { height: 6px; overflow: hidden; border-radius: 3px; background: var(--a-bg-3); }
.timing-track span { display: block; height: 100%; min-width: 2px; border-radius: 3px; background: var(--a-primary); }
.timing-row code { text-align: right; color: var(--a-text-soft); font: 11px var(--a-font-mono); }
.timing-row.unavailable .timing-track { height: 0; border-top: 1px dashed var(--a-border-strong); border-radius: 0; background: transparent; }
.timing-row.unavailable code { color: var(--a-text-faint); font-family: var(--a-font-ui); }
.timing-note { margin: 10px 0 0; color: var(--a-text-faint); font-size: 11px; }
.timing-total { display: flex; justify-content: space-between; max-width: 100%; margin-top: 10px; padding: 12px 0 0; border-top: 1px solid var(--a-border); color: var(--a-text); font-size: 12px; }
.actual-panel { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 14px 16px 20px; }
.actual-summary { display: grid; gap: 10px; padding-bottom: 14px; border-bottom: 1px solid var(--a-border); }
.actual-field { min-width: 0; display: grid; grid-template-columns: 86px minmax(0, 1fr); align-items: center; min-height: 24px; }
.actual-label { color: var(--a-text-faint); font-size: 12px; }
.method-pill { justify-self: start; color: var(--a-post); background: var(--a-post-bg); }
.copy-url { justify-self: start; max-width: 100%; padding: 0 0 2px; overflow: hidden; border: 0; border-bottom: 1px dashed transparent; background: transparent; color: var(--a-text-soft); font: 12px/20px var(--a-font-mono); text-align: left; text-overflow: ellipsis; white-space: nowrap; cursor: copy; }
.copy-url:hover { color: var(--a-primary); border-bottom-color: currentColor; }
.actual-section { margin-top: 16px; }
.actual-section > header { min-height: 28px; display: flex; align-items: center; gap: 8px; color: var(--a-text); font-size: var(--a-font-result); }
.actual-section > header > span { color: var(--a-text-faint); font: 11px var(--a-font-mono); }
.actual-params { overflow: hidden; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); }
.actual-param { min-height: 28px; display: grid; grid-template-columns: minmax(130px, 22%) minmax(180px, 34%) minmax(240px, 1fr); border-bottom: 1px solid var(--a-border); font-size: var(--a-font-result); }
.actual-param:last-child { border-bottom: 0; }
.actual-param > * { min-width: 0; padding: 5px 9px; overflow-wrap: anywhere; border-right: 1px solid var(--a-border); }
.actual-param > :last-child { border-right: 0; }
.actual-param code { color: var(--a-post); font: 11px/18px var(--a-font-mono); }
.actual-param .encoded-param { color: var(--a-text-faint); }
.actual-param-head { background: var(--a-bg-3); color: var(--a-text-faint); font-weight: var(--a-fw-medium); }
.actual-param-head span { color: inherit; }
.actual-none { margin: 0; padding: 10px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); color: var(--a-text-faint); font-size: 11px; }
.actual-headers { overflow: hidden; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); }
.actual-header { min-height: 28px; display: grid; grid-template-columns: minmax(150px, 28%) minmax(260px, 1fr); border-bottom: 1px solid var(--a-border); font-size: var(--a-font-result); }
.actual-header:last-child { border-bottom: 0; }
.actual-header > * { min-width: 0; padding: 5px 9px; overflow-wrap: anywhere; border-right: 1px solid var(--a-border); }
.actual-header > :last-child { border-right: 0; }
.actual-header code { color: var(--a-post); font: 11px/18px var(--a-font-mono); }
.actual-header-head { background: var(--a-bg-3); color: var(--a-text-faint); font-weight: var(--a-fw-medium); }
.code-section { min-height: 220px; display: flex; flex-direction: column; }
.code-head { flex: 0 0 auto; }
.code-tabs { display: flex; gap: 2px; margin-left: 6px; }
.code-tabs button { height: 24px; padding: 0 8px; border: 0; border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim); font: 11px var(--a-font-ui); cursor: pointer; }
.code-tabs button:hover { background: var(--a-hover); color: var(--a-text); }
.code-tabs button.on { background: var(--a-selected); color: var(--a-primary); }
.request-code { flex: 1 1 auto; min-height: 170px; max-height: 360px; margin: 0; padding: 12px; overflow: auto; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-1); color: var(--a-text-soft); font: 11px/18px var(--a-font-mono); white-space: pre-wrap; overflow-wrap: anywhere; }
.actual-empty { flex: 1 1 auto; }
@media (max-width: 700px) {
  .tier, .format-label { display: none; }
  .sections button { padding: 0 5px; }
  .sections button::after { left: 5px; right: 5px; }
  .metric:first-of-type { display: none; }
  .actual-field { grid-template-columns: 72px minmax(0, 1fr); }
  .code-head { flex-wrap: wrap; }
  .code-tabs { order: 3; width: 100%; margin-left: 0; }
}
</style>
