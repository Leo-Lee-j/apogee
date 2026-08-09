<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { NTabPane, NTabs } from 'naive-ui'
import { AppAlert, AppButton, AppTag } from '@/components/ui'
import { AppIcon } from '@/components/ui'
import { IconArrowLeft, IconClipboard, IconClose, IconCloudUpload, IconDocument, IconFlash } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import { importCurl } from '@/features/curl'
import { ipc, inTauri, type RequestSpec } from '@/ipc'

/**
 * 新建请求标签里的选项面板。
 *
 * 面板状态归属于当前 Tab：打开新建请求时会先追加一个独立的“新建请求”标签，
 * 因此它不会覆盖原来活跃接口的请求/响应内容。切换到其他标签时，面板自然离开
 * 视口；选择一种请求后，当前标签再转换为普通请求编辑标签。
 */

const tabs = useTabsStore()
const workspace = useWorkspaceStore()
const panel = ref<HTMLElement | null>(null)
const actionError = ref('')
const actionNotice = ref('')
const actionBusy = ref(false)
const curlMode = ref(false)
const curlCommand = ref('')
const importWarnings = ref<string[]>([])
const previewSpec = ref<RequestSpec | null>(null)
const previewError = ref('')
const previewLoading = ref(false)
const previewTab = ref<'json' | 'visual'>('json')
let previewTimer: ReturnType<typeof setTimeout> | undefined
let previewSequence = 0

const previewText = computed(() => previewSpec.value ? JSON.stringify(previewSpec.value, null, 2) : '')
const visualHeaders = computed(() => (previewSpec.value?.headers ?? []).filter((item) => item.enabled && item.key))
const visualParams = computed(() => (previewSpec.value?.params ?? []).filter((item) => item.enabled && item.key))
const visualAuth = computed(() => {
  const auth = previewSpec.value?.auth
  if (!auth || auth.type === 'none') return '无鉴权'
  if (auth.type === 'basic') return `Basic Auth (${auth.username})`
  if (auth.type === 'bearer') return 'Bearer Token'
  if (auth.type === 'apiKey') return `API Key (${auth.key}, ${auth.location})`
  return '继承'
})
const visualBody = computed(() => {
  const body = previewSpec.value?.body
  if (!body || body.type === 'none') return '无请求体'
  if ('content' in body) return body.type === 'json' ? `JSON\n${body.content}` : body.content
  if (body.type === 'binary') return `文件：${body.path}`
  if (body.type === 'graphQl') return `GraphQL\n${body.query}`
  return body.rows.filter((item) => item.enabled && item.key).map((item) => `${item.key}: ${item.value}`).join('\n') || '空'
})

async function updateCurlPreview(command: string) {
  const sequence = ++previewSequence
  const value = command.trim()
  previewSpec.value = null
  previewError.value = ''
  if (!value) {
    previewLoading.value = false
    return
  }
  previewLoading.value = true
  try {
    const result = await importCurl(value)
    if (sequence !== previewSequence) return
    previewSpec.value = result.spec
    importWarnings.value = result.warnings
  } catch (error) {
    if (sequence !== previewSequence) return
    previewError.value = error instanceof Error ? error.message : '无法解析这条 cURL 命令。'
    importWarnings.value = []
  } finally {
    if (sequence === previewSequence) previewLoading.value = false
  }
}

watch([curlCommand, curlMode], ([command, mode]) => {
  if (previewTimer) clearTimeout(previewTimer)
  if (!mode) {
    previewSequence += 1
    previewSpec.value = null
    previewError.value = ''
    previewLoading.value = false
    return
  }
  previewTimer = setTimeout(() => { void updateCurlPreview(command) }, 220)
})

onBeforeUnmount(() => {
  if (previewTimer) clearTimeout(previewTimer)
  previewSequence += 1
})

const targetNode = computed(() => {
  const parentId = tabs.active?.newRequestParentId
  return parentId ? workspace.nodes?.find((node) => node.id === parentId) ?? null : null
})
const targetHint = computed(() =>
  targetNode.value
    ? `创建位置：${targetNode.value.name}`
    : '未指定位置，将保存到当前工作区的默认集合',
)

interface NewOption {
  id: string
  title: string
  desc: string
  icon: 'bolt' | 'file' | 'clipboard' | 'import'
  kbd?: string
  badge?: string
  reason?: string
  run?: () => void | Promise<void>
}

/** 选择快捷请求：把当前独立面板标签转换为空白请求编辑标签。 */
function quickRequest() {
  tabs.resolveNewPanel()
}

/**
 * 选择集合内接口：创建并打开持久化请求，然后移除承载选项面板的临时标签。
 * 这样不会留下一个多余的“新建请求”标签。
 */
async function savedRequest() {
  const panelTab = tabs.active
  if (!panelTab?.newPanel) return
  const panelId = panelTab.id
  const request = await workspace.createRequest(panelTab.newRequestParentId, '新建请求')
  if (!request) {
    throw new Error(workspace.lastError?.message ?? '创建请求失败，请先创建一个集合。')
  }
  tabs.close(panelId)
}

function openCurlImport() {
  curlMode.value = true
  actionError.value = ''
  void nextTick(() => panel.value?.querySelector<HTMLTextAreaElement>('.curl-input')?.focus())
}

async function applyCurlImport() {
  const current = tabs.active
  if (!current?.newPanel) return
  actionError.value = ''
  actionBusy.value = true
  try {
    const result = await importCurl(curlCommand.value)
    const spec = result.spec
    importWarnings.value = result.warnings
    let title = '导入的请求'
    try { title = new URL(spec.url).pathname.split('/').filter(Boolean).pop() || new URL(spec.url).hostname }
    catch { /* 保留默认标题，URL 编辑器会继续允许用户修正。 */ }
    tabs.patch(current.id, {
      title,
      method: spec.method,
      url: spec.url,
      params: spec.params ?? [],
      pathParams: spec.pathParams ?? [],
      headers: spec.headers ?? [],
      body: spec.body ?? { type: 'none' },
      auth: spec.auth ?? { type: 'none' },
      settings: spec.settings ?? {},
      newPanel: false,
      newRequestParentId: undefined,
      dirty: true,
    })
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : '无法解析这条 cURL 命令。'
  } finally {
    actionBusy.value = false
  }
}

async function openPostmanImport() {
  if (!inTauri) { actionError.value = '请从 Apogee 桌面应用导入文件'; return }
  const { open } = await import('@tauri-apps/plugin-dialog')
  const selected = await open({ multiple: false, filters: [{ name: 'Postman Collection', extensions: ['json'] }] })
  if (typeof selected !== 'string') return
  const result = await ipc.postmanImportWorkspace(selected)
  await workspace.load()
  actionNotice.value = `已导入 ${result.importedRequests} 个请求和 ${result.importedFolders} 个文件夹`
  if (result.warnings.length) actionNotice.value += `；${result.warnings.join('；')}`
}

const GROUPS: { title: string; items: NewOption[] }[] = [
  {
    title: '请求',
    items: [
      {
        id: 'quick',
        title: '快捷请求',
        desc: '不进集合，发完即走。适合临时试一个接口',
        icon: 'bolt',
        kbd: 'Ctrl+T',
        run: quickRequest,
      },
      {
        id: 'saved',
        title: '集合内接口',
        desc: '保存到当前创建位置，落盘为单个文件（Git 可见）',
        icon: 'file',
        run: savedRequest,
      },
    ],
  },
  {
    title: '导入',
    items: [
      {
        id: 'curl',
        title: '粘贴 cURL',
        desc: '把一条 curl 命令还原成请求',
        icon: 'clipboard',
        run: openCurlImport,
      },
      {
        id: 'file',
        title: '从其他工具导入',
        desc: 'Postman / OpenAPI / Insomnia / HAR',
        icon: 'import',
        run: openPostmanImport,
        badge: 'v0.2',
        reason: '导入 Postman Collection 并保留文件夹层级。',
      },
    ],
  },
]

function closePanel() {
  const id = tabs.active?.id
  if (id) tabs.close(id)
}

async function pick(option: NewOption) {
  if (!option.run || actionBusy.value) return
  actionError.value = ''
  actionBusy.value = true
  try {
    await option.run()
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : '创建失败，请稍后重试。'
  } finally {
    actionBusy.value = false
  }
}

function cards(): HTMLButtonElement[] {
  return panel.value ? [...panel.value.querySelectorAll<HTMLButtonElement>('.card')] : []
}

function moveFocus(step: number) {
  const list = cards()
  if (list.length === 0) return
  const at = list.indexOf(document.activeElement as HTMLButtonElement)
  const next = at < 0 ? 0 : (at + step + list.length) % list.length
  list[next]?.focus()
}

function onKeydown(e: KeyboardEvent) {
  switch (e.key) {
    case 'Escape':
      e.preventDefault()
      closePanel()
      break
    case 'ArrowRight':
      e.preventDefault()
      moveFocus(1)
      break
    case 'ArrowLeft':
      e.preventDefault()
      moveFocus(-1)
      break
    case 'ArrowDown':
      e.preventDefault()
      moveFocus(2)
      break
    case 'ArrowUp':
      e.preventDefault()
      moveFocus(-2)
      break
    default:
      break
  }
}

onMounted(async () => {
  await nextTick()
  cards()[0]?.focus()
})
</script>

<template>
  <section
    id="new-request-panel"
    ref="panel"
    class="panel"
    role="region"
    aria-labelledby="new-title"
    @keydown="onKeydown"
  >
    <NaiveThemeScope>
      <header class="hd">
        <div>
          <h2 id="new-title">新建请求</h2>
          <p class="sub">这是一个独立标签，选择请求类型后开始调试</p>
        </div>
        <AppButton
          class="x"
          quaternary
          circle
          size="small"
          :disabled="actionBusy"
          aria-label="关闭新建请求标签"
          title="关闭新建请求标签"
          @click="closePanel"
        >
          <template #icon>
            <AppIcon :component="IconClose" :size="16" />
          </template>
        </AppButton>
      </header>

      <p class="target" aria-live="polite">{{ targetHint }}</p>

      <AppAlert v-if="actionNotice" type="success" class="action-notice" :show-icon="false" closable @close="actionNotice = ''">
        {{ actionNotice }}
      </AppAlert>

      <AppAlert
        v-if="actionError"
        class="action-error"
        type="error"
        :show-icon="false"
        closable
        @close="actionError = ''"
      >
        {{ actionError }}
      </AppAlert>

      <div v-if="curlMode" class="curl-form">
        <div class="curl-workspace">
          <section class="curl-column">
            <label for="curl-command">cURL 命令</label>
            <textarea id="curl-command" v-model="curlCommand" class="curl-input" spellcheck="false" placeholder="curl 'https://api.example.com/users' --header 'Accept: application/json'" />
          </section>
          <section class="curl-column preview-column" aria-live="polite">
            <div class="preview-label">
              <span>解析预览</span>
              <span v-if="previewLoading" class="preview-state">解析中…</span>
            </div>
            <NTabs v-if="previewSpec" v-model:value="previewTab" class="preview-tabs" type="line" size="small">
              <NTabPane name="json" tab="JSON 参数">
                <pre class="curl-preview">{{ previewText }}</pre>
              </NTabPane>
              <NTabPane name="visual" tab="可视化预览">
                <div class="visual-preview">
                  <div class="visual-row visual-request">
                    <span :class="['visual-method', `method-${previewSpec.method.toLowerCase()}`]">{{ previewSpec.method }}</span>
                    <code>{{ previewSpec.url }}</code>
                  </div>
                  <div class="visual-section">
                    <h4>Query 参数</h4>
                    <div v-if="visualParams.length" class="visual-list">
                      <div v-for="item in visualParams" :key="`param-${item.key}`" class="visual-item"><code>{{ item.key }}</code><span>{{ item.value }}</span></div>
                    </div>
                    <p v-else class="visual-muted">无</p>
                  </div>
                  <div class="visual-section">
                    <h4>Headers</h4>
                    <div v-if="visualHeaders.length" class="visual-list">
                      <div v-for="item in visualHeaders" :key="`header-${item.key}`" class="visual-item"><code>{{ item.key }}</code><span>{{ item.value }}</span></div>
                    </div>
                    <p v-else class="visual-muted">无</p>
                  </div>
                  <div class="visual-section"><h4>鉴权</h4><p class="visual-value">{{ visualAuth }}</p></div>
                  <div class="visual-section"><h4>请求体</h4><pre class="visual-body">{{ visualBody }}</pre></div>
                </div>
              </NTabPane>
            </NTabs>
            <div v-else-if="previewError" class="preview-empty error">{{ previewError }}</div>
            <div v-else class="preview-empty">{{ curlCommand.trim() ? '等待解析…' : '粘贴 cURL 命令后实时显示解析结果' }}</div>
          </section>
        </div>
        <AppAlert v-if="importWarnings.length" type="warning" :show-icon="false">
          {{ importWarnings.join('；') }}
        </AppAlert>
        <div class="curl-actions">
          <AppButton secondary :disabled="actionBusy" @click="curlMode = false">
            <template #icon><AppIcon :component="IconArrowLeft" :size="15" /></template>
            返回
          </AppButton>
          <AppButton type="primary" :loading="actionBusy" @click="applyCurlImport">
            <template #icon><AppIcon :component="IconCloudUpload" :size="15" /></template>
            导入请求
          </AppButton>
        </div>
      </div>

      <div v-else class="groups">
        <section v-for="group in GROUPS" :key="group.title">
          <h3 class="gt">{{ group.title }}</h3>
          <div class="grid">
            <button
              v-for="option in group.items"
              :key="option.id"
              class="card"
              :class="{ off: !option.run }"
              type="button"
              :aria-disabled="!option.run || actionBusy"
              :title="option.reason"
              @click="pick(option)"
            >
              <span class="ic">
                <AppIcon
                  :component="option.icon === 'bolt' ? IconFlash : option.icon === 'file' ? IconDocument : option.icon === 'clipboard' ? IconClipboard : IconCloudUpload"
                  :size="18"
                />
              </span>
              <span class="tx">
                <span class="top">
                  <span class="t">{{ option.title }}</span>
                  <kbd v-if="option.kbd">{{ option.kbd }}</kbd>
                  <AppTag
                    v-else-if="option.badge"
                    class="badge"
                    size="small"
                    type="warning"
                    :bordered="false"
                  >
                    {{ option.badge }}
                  </AppTag>
                </span>
                <span class="d">{{ option.desc }}</span>
              </span>
            </button>
          </div>
        </section>
      </div>

      <footer class="ft">
        <span><kbd>Ctrl</kbd>+<kbd>T</kbd> 直接创建空白请求标签</span>
        <span class="dim">按 <kbd>Esc</kbd> 关闭此标签</span>
      </footer>
    </NaiveThemeScope>
  </section>
</template>

<style scoped>
.panel {
  width: 100%;
  height: 100%;
  flex: 1 1 0;
  min-height: 0;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  background: var(--a-bg-2);
}
.hd {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 16px 8px;
}
.hd h2 {
  margin: 0;
  font: var(--a-fw-semibold) var(--a-fs-lg) / 1.4 var(--a-font-ui);
  color: var(--a-text);
}
.sub {
  margin: 3px 0 0;
  color: var(--a-text-dim);
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
}
.x {
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  padding: 0;
  color: var(--a-text-dim);
}
.x:hover { color: var(--a-text); }
.ico {
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentcolor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.target {
  flex: 0 0 auto;
  margin: 0 16px 8px;
  padding: 6px 8px;
  background: var(--a-bg-3);
  border-left: 2px solid var(--a-primary);
  color: var(--a-text-dim);
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
}
.groups {
  flex: 1 1 auto;
  overflow-y: auto;
  min-height: 0;
  padding: 0 16px 4px;
}
.curl-form { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; gap: 8px; padding: 8px 16px 16px; }
.curl-workspace { flex: 1 1 auto; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 12px; }
.curl-column { min-width: 0; min-height: 0; display: flex; flex-direction: column; gap: 6px; }
.curl-column > label, .preview-label { color: var(--a-text-dim); font: var(--a-fw-medium) var(--a-fs-sm) var(--a-font-ui); }
.preview-label { display: flex; justify-content: space-between; align-items: center; }
.preview-state { color: var(--a-primary); font-size: var(--a-fs-xs); font-weight: var(--a-fw-normal); }
.curl-input { flex: 1 1 auto; min-height: 180px; resize: none; box-sizing: border-box; background: var(--a-bg-3); border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text); padding: 10px; font: var(--a-fs-sm)/1.6 var(--a-font-mono); }
.curl-input:focus { outline: 2px solid var(--a-focus); outline-offset: -2px; }
.preview-tabs { flex: 1 1 auto; min-height: 180px; display: flex; flex-direction: column; }
.preview-tabs :deep(.n-tabs-nav) { flex: 0 0 auto; margin-bottom: 6px; }
.preview-tabs :deep(.n-tabs-pane-wrapper), .preview-tabs :deep(.n-tab-pane) { flex: 1 1 auto; min-height: 0; }
.curl-preview { height: 100%; min-height: 180px; overflow: auto; box-sizing: border-box; margin: 0; padding: 10px; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-3); color: var(--a-text); font: var(--a-fs-sm)/1.6 var(--a-font-mono); white-space: pre-wrap; word-break: break-word; }
.visual-preview { height: 100%; min-height: 180px; overflow: auto; box-sizing: border-box; padding: 10px; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-3); }
.visual-request { display: flex; align-items: flex-start; gap: 8px; padding-bottom: 10px; border-bottom: 1px solid var(--a-border); }
.visual-method { flex: 0 0 auto; padding: 2px 6px; border-radius: var(--a-radius-sm); background: var(--a-get-bg); color: var(--a-get); font: var(--a-fw-semibold) var(--a-fs-xs)/1.4 var(--a-font-mono); }
.visual-method.method-post { background: var(--a-post-bg); color: var(--a-post); }
.visual-method.method-put { background: var(--a-put-bg); color: var(--a-put); }
.visual-method.method-patch { background: var(--a-patch-bg); color: var(--a-patch); }
.visual-method.method-delete { background: var(--a-delete-bg); color: var(--a-delete); }
.visual-request code { min-width: 0; color: var(--a-text); font: var(--a-fs-sm)/1.5 var(--a-font-mono); overflow-wrap: anywhere; }
.visual-section { padding-top: 10px; }
.visual-section h4 { margin: 0 0 5px; color: var(--a-text-dim); font: var(--a-fw-medium) var(--a-fs-xs)/1.4 var(--a-font-ui); }
.visual-list { display: grid; gap: 4px; }
.visual-item { display: grid; grid-template-columns: minmax(90px, 36%) minmax(0, 1fr); gap: 8px; font: var(--a-fs-xs)/1.5 var(--a-font-mono); }
.visual-item code { overflow-wrap: anywhere; color: var(--a-primary-hover); }
.visual-item span { overflow-wrap: anywhere; color: var(--a-text-soft); }
.visual-muted, .visual-value { margin: 0; color: var(--a-text-faint); font: var(--a-fs-sm)/1.5 var(--a-font-ui); }
.visual-value { color: var(--a-text-soft); }
.visual-body { max-height: 180px; overflow: auto; margin: 0; padding: 7px; border-radius: var(--a-radius-sm); background: var(--a-bg-2); color: var(--a-text-soft); font: var(--a-fs-xs)/1.5 var(--a-font-mono); white-space: pre-wrap; word-break: break-word; }
.preview-empty { flex: 1 1 auto; min-height: 180px; display: flex; align-items: center; justify-content: center; box-sizing: border-box; padding: 16px; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text-faint); font: var(--a-fs-sm)/1.5 var(--a-font-ui); text-align: center; }
.preview-empty.error { color: var(--a-danger); align-items: flex-start; justify-content: flex-start; text-align: left; white-space: pre-wrap; }
.curl-actions { display: flex; justify-content: flex-end; gap: 8px; }
@media (max-width: 720px) { .curl-workspace { grid-template-columns: minmax(0, 1fr); } }
.gt {
  margin: 8px 0 6px;
  font: var(--a-fw-medium) var(--a-fs-xs) / 1 var(--a-font-ui);
  color: var(--a-text-faint);
  text-transform: none;
}
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.card {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 12px;
  text-align: left;
  background: var(--a-bg-3);
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  color: var(--a-text);
  font: inherit;
  cursor: pointer;
}
.card:not(.off):hover { border-color: var(--a-primary); background: var(--a-selected); }
.card.off { cursor: default; background: transparent; border-color: var(--a-border); }
.card.off .top .t { color: var(--a-text-dim); }
.card.off .d { color: var(--a-text-faint); }
.card .ic { flex: 0 0 auto; color: var(--a-primary); margin-top: 1px; }
.card.off .ic { color: var(--a-text-ghost); }
.card .ic .ico { width: 16px; height: 16px; }
.tx { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.top { display: flex; align-items: center; gap: 6px; }
.top .t { font: var(--a-fw-semibold) var(--a-fs-base) / 1.3 var(--a-font-ui); }
.card .d { font: var(--a-fs-sm) / 1.5 var(--a-font-ui); color: var(--a-text-dim); }
.badge {
  flex: 0 0 auto;
  border-radius: var(--a-radius-pill);
  font: var(--a-fw-medium) var(--a-fs-xs) / 16px var(--a-font-ui);
  white-space: nowrap;
}
.action-error { flex: 0 0 auto; margin: 0 16px 4px; }
kbd {
  padding: 0 4px;
  background: var(--a-bg-2);
  border: 1px solid var(--a-border-strong);
  border-radius: var(--a-radius-sm);
  color: var(--a-text-dim);
  font: var(--a-fs-xs) / 15px var(--a-font-mono);
}
.card kbd { background: var(--a-bg-2); }
.ft {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px 12px;
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
  color: var(--a-text-dim);
}
.ft .dim { color: var(--a-text-faint); }
</style>
