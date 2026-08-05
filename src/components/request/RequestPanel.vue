<script setup lang="ts">
import { computed, ref } from 'vue'
import type { AuthConfig, Body, KeyValue } from '@/ipc'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { exportCurl } from '@/features/curl'

/**
 * 请求编辑区 —— v0.1 先落地 Params、Headers、Body 三条发送主链。
 *
 * Tab store 使用 shallowRef + markRaw，不允许在这里直接 v-model 修改行对象；
 * 所有编辑都复制数组后通过 tabs.patch 整体替换，保证界面和发送状态同步。
 */
const TABS = ['Params', 'Headers', 'Body', 'Auth', 'Scripts', 'Settings'] as const
type PanelTab = (typeof TABS)[number]
type RowField = 'params' | 'pathParams' | 'headers'
type BodyType = Body['type']

type BodyOption = { type: BodyType; label: string }
const BODY_TYPES: BodyOption[] = [
  { type: 'none', label: 'none' },
  { type: 'json', label: 'JSON' },
  { type: 'text', label: 'raw' },
  { type: 'formData', label: 'form-data' },
  { type: 'urlEncoded', label: 'x-www-form-urlencoded' },
  { type: 'binary', label: 'binary' },
  { type: 'graphQl', label: 'GraphQL' },
]

type AuthType = AuthConfig['type']
const AUTH_TYPES: { type: AuthType; label: string }[] = [
  { type: 'inherit', label: '继承' },
  { type: 'none', label: '无鉴权' },
  { type: 'basic', label: 'Basic Auth' },
  { type: 'bearer', label: 'Bearer Token' },
  { type: 'apiKey', label: 'API Key' },
]

interface EditorSection {
  kind: RowField
  label: string
  rows: KeyValue[]
  hint?: string
}

const tabs = useTabsStore()
const tab = computed(() => tabs.active)
const current = ref<PanelTab>('Params')
const exportState = ref('')

const activeParamCount = computed(() => {
  const currentTab = tab.value
  if (!currentTab) return 0
  return [...currentTab.pathParams, ...currentTab.params].filter((row) => row.enabled && row.key).length
})
const activeHeaderCount = computed(
  () => tab.value?.headers.filter((row) => row.enabled && row.key).length ?? 0,
)

/** 当前 tab 下需要显示的参数/请求头表格。 */
const editorSections = computed<EditorSection[]>(() => {
  const currentTab = tab.value
  if (!currentTab) return []

  if (current.value === 'Params') {
    const sections: EditorSection[] = []
    if (currentTab.pathParams.length > 0) {
      sections.push({
        kind: 'pathParams',
        label: 'Path 参数',
        rows: currentTab.pathParams,
        hint: '由 URL 中的 :id 或 {id} 自动识别。',
      })
    }
    sections.push({
      kind: 'params',
      label: 'Query 参数',
      rows: currentTab.params,
      hint: '禁用参数会保留在草稿中，但不会发送。',
    })
    return sections
  }

  if (current.value === 'Headers') {
    return [
      {
        kind: 'headers',
        label: '请求 Headers',
        rows: currentTab.headers,
        hint: '请求头名称不区分大小写；重复名称由后端按最终请求规则处理。',
      },
    ]
  }

  return []
})

const body = computed<Body>(() => tab.value?.body ?? { type: 'none' })
const bodyType = computed(() => body.value.type)
const bodyContent = computed(() => ('content' in body.value ? body.value.content : ''))
const textContentType = computed(() =>
  body.value.type === 'text' ? body.value.contentType : 'text/plain',
)
const bodyPlaceholder = computed(() =>
  bodyType.value === 'json' ? '{\n  "key": "value"\n}' : '输入原始请求体',
)
const auth = computed<AuthConfig>(() => tab.value?.auth ?? { type: 'none' })

function patchTab(delta: Partial<Tab>) {
  const currentTab = tab.value
  if (!currentTab) return
  tabs.patch(currentTab.id, { ...delta, dirty: true })
}

function rowsFor(kind: RowField): KeyValue[] {
  return tab.value?.[kind] ?? []
}

function patchRows(kind: RowField, rows: KeyValue[]) {
  const delta: Partial<Tab> = { dirty: true }
  if (kind === 'params') delta.params = rows
  else if (kind === 'pathParams') delta.pathParams = rows
  else delta.headers = rows

  const currentTab = tab.value
  if (currentTab) tabs.patch(currentTab.id, delta)
}

function updateRow(kind: RowField, index: number, delta: Partial<KeyValue>) {
  const rows = rowsFor(kind).map((row, rowIndex) =>
    rowIndex === index ? { ...row, ...delta } : row,
  )
  patchRows(kind, rows)
}

function onEnabled(kind: RowField, index: number, event: Event) {
  updateRow(kind, index, { enabled: (event.target as HTMLInputElement).checked })
}

function onText(
  kind: RowField,
  index: number,
  field: 'key' | 'value' | 'description',
  event: Event,
) {
  const delta: Partial<KeyValue> = { [field]: (event.target as HTMLInputElement).value }
  updateRow(kind, index, delta)
}

function addRow(kind: RowField) {
  patchRows(kind, [
    ...rowsFor(kind),
    { enabled: true, key: '', value: '', description: '', kind: 'text' },
  ])
}

function removeRow(kind: RowField, index: number) {
  patchRows(kind, rowsFor(kind).filter((_, rowIndex) => rowIndex !== index))
}

function setBodyType(nextType: BodyType) {
  const currentBody = body.value
  const content = 'content' in currentBody ? currentBody.content : ''
  let nextBody: Body

  if (nextType === 'none') {
    nextBody = { type: 'none' }
  } else if (nextType === 'json') {
    nextBody = { type: 'json', content }
  } else if (nextType === 'text') {
    nextBody = {
      type: 'text',
      content,
      contentType: currentBody.type === 'text' ? currentBody.contentType : 'text/plain',
    }
  } else if (nextType === 'formData') {
    nextBody = { type: 'formData', rows: currentBody.type === 'formData' ? currentBody.rows : [] }
  } else if (nextType === 'urlEncoded') {
    nextBody = { type: 'urlEncoded', rows: currentBody.type === 'urlEncoded' ? currentBody.rows : [] }
  } else if (nextType === 'binary') {
    nextBody = { type: 'binary', path: currentBody.type === 'binary' ? currentBody.path : '' }
  } else {
    nextBody = currentBody.type === 'graphQl'
      ? currentBody
      : { type: 'graphQl', query: '', variables: '{}', operationName: undefined }
  }

  patchTab({ body: nextBody })
}

function updateBodyContent(event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  if (body.value.type === 'json') patchTab({ body: { type: 'json', content: value } })
  else if (body.value.type === 'text') {
    patchTab({ body: { type: 'text', content: value, contentType: body.value.contentType } })
  }
}

function updateTextContentType(event: Event) {
  if (body.value.type !== 'text') return
  patchTab({
    body: {
      type: 'text',
      content: body.value.content,
      contentType: (event.target as HTMLInputElement).value,
    },
  })
}

function bodyRows(): KeyValue[] {
  return body.value.type === 'formData' || body.value.type === 'urlEncoded' ? body.value.rows : []
}

function patchBodyRows(rows: KeyValue[]) {
  if (body.value.type === 'formData') patchTab({ body: { type: 'formData', rows } })
  else if (body.value.type === 'urlEncoded') patchTab({ body: { type: 'urlEncoded', rows } })
}

function updateBodyRow(index: number, delta: Partial<KeyValue>) {
  patchBodyRows(bodyRows().map((row, rowIndex) => rowIndex === index ? { ...row, ...delta } : row))
}

function addBodyRow() {
  patchBodyRows([...bodyRows(), { enabled: true, key: '', value: '', description: '', kind: 'text' }])
}

function removeBodyRow(index: number) {
  patchBodyRows(bodyRows().filter((_, rowIndex) => rowIndex !== index))
}

function patchBodyField(field: string, value: string) {
  const currentBody = body.value
  if (currentBody.type === 'binary' && field === 'path') {
    patchTab({ body: { ...currentBody, path: value } })
  } else if (currentBody.type === 'graphQl') {
    patchTab({ body: { ...currentBody, [field]: value || (field === 'operationName' ? undefined : '') } })
  }
}

function setAuthType(type: AuthType) {
  const currentAuth = auth.value
  let next: AuthConfig
  if (type === 'inherit' || type === 'none') next = { type }
  else if (type === 'basic') next = currentAuth.type === 'basic' ? currentAuth : { type, username: '', password: '' }
  else if (type === 'bearer') next = currentAuth.type === 'bearer' ? currentAuth : { type, token: '' }
  else next = currentAuth.type === 'apiKey'
    ? currentAuth
    : { type, key: '', value: '', location: 'header' }
  patchTab({ auth: next })
}

function patchAuth(delta: Record<string, string>) {
  patchTab({ auth: { ...auth.value, ...delta } as AuthConfig })
}

async function copyCurl() {
  const currentTab = tab.value
  if (!currentTab) return
  try {
    await navigator.clipboard.writeText(exportCurl(currentTab))
    exportState.value = '已复制'
  } catch (error) {
    exportState.value = error instanceof Error ? error.message : '复制失败'
  }
  window.setTimeout(() => { exportState.value = '' }, 1800)
}
</script>

<template>
  <section class="req">
    <nav class="tabs" role="tablist">
      <button
        v-for="t in TABS"
        :key="t"
        type="button"
        role="tab"
        :aria-selected="current === t"
        :class="{ on: current === t }"
        @click="current = t"
      >
        {{ t }}
        <span v-if="t === 'Params'" class="badge">{{ activeParamCount }}</span>
        <span v-if="t === 'Headers'" class="badge">{{ activeHeaderCount }}</span>
      </button>
      <span class="spacer" />
      <span v-if="exportState" class="export-state" role="status">{{ exportState }}</span>
      <button class="link" type="button" title="复制为 cURL" @click="copyCurl">导出 cURL</button>
      <button class="link" type="button" title="批量编辑将在请求模型稳定后接入">批量编辑</button>
    </nav>

    <div v-if="!tab" class="empty">先新建一个请求，再开始编辑。</div>
    <div v-else class="body">
      <template v-if="current === 'Params' || current === 'Headers'">
        <section v-for="section in editorSections" :key="section.kind" class="table-section">
          <header class="section-head">
            <strong>{{ section.label }}</strong>
            <span class="section-count">{{ section.rows.filter((row) => row.enabled && row.key).length }} 个启用</span>
          </header>
          <p v-if="section.hint" class="section-hint">{{ section.hint }}</p>
          <table class="kv">
            <thead>
              <tr><th class="c" /><th>Key</th><th>Value</th><th>Description</th><th class="c" /></tr>
            </thead>
            <tbody>
              <tr
                v-for="(row, index) in section.rows"
                :key="`${section.kind}-${index}`"
                :class="{ off: !row.enabled }"
              >
                <td class="c">
                  <input
                    :checked="row.enabled"
                    type="checkbox"
                    :aria-label="`启用 ${row.key || '空行'}`"
                    @change="onEnabled(section.kind, index, $event)"
                  />
                </td>
                <td><input :value="row.key" class="cell" spellcheck="false" @input="onText(section.kind, index, 'key', $event)" /></td>
                <td><input :value="row.value" class="cell" spellcheck="false" @input="onText(section.kind, index, 'value', $event)" /></td>
                <td><input :value="row.description ?? ''" class="cell" @input="onText(section.kind, index, 'description', $event)" /></td>
                <td class="c">
                  <button class="del" type="button" aria-label="删除" @click="removeRow(section.kind, index)">×</button>
                </td>
              </tr>
              <tr><td class="c" /><td colspan="4"><button class="link" type="button" @click="addRow(section.kind)">+ 新增</button></td></tr>
            </tbody>
          </table>
        </section>
      </template>

      <section v-else-if="current === 'Body'" class="body-editor">
        <div class="body-types" role="radiogroup" aria-label="请求体类型">
          <label v-for="option in BODY_TYPES" :key="option.type" class="body-type">
            <input
              :checked="bodyType === option.type"
              type="radio"
              name="request-body-type"
              @change="setBodyType(option.type)"
            />
            {{ option.label }}
          </label>
        </div>

        <div v-if="bodyType === 'none'" class="editor-empty">此请求不发送请求体。</div>
        <textarea
          v-else-if="bodyType === 'json' || bodyType === 'text'"
          class="payload"
          :value="bodyContent"
          :placeholder="bodyPlaceholder"
          spellcheck="false"
          aria-label="请求体内容"
          @input="updateBodyContent"
        />
        <label v-if="bodyType === 'text'" class="content-type">
          Content-Type
          <input :value="textContentType" spellcheck="false" @input="updateTextContentType" />
        </label>
        <table v-else-if="bodyType === 'formData' || bodyType === 'urlEncoded'" class="kv body-table">
          <thead><tr><th class="c" /><th>Key</th><th>Value</th><th v-if="bodyType === 'formData'">类型</th><th class="c" /></tr></thead>
          <tbody>
            <tr v-for="(row, index) in bodyRows()" :key="index" :class="{ off: !row.enabled }">
              <td class="c"><input :checked="row.enabled" type="checkbox" @change="updateBodyRow(index, { enabled: ($event.target as HTMLInputElement).checked })" /></td>
              <td><input :value="row.key" class="cell" spellcheck="false" @input="updateBodyRow(index, { key: ($event.target as HTMLInputElement).value })" /></td>
              <td><input :value="row.value" class="cell" spellcheck="false" :placeholder="row.kind === 'file' ? '本地文件路径' : ''" @input="updateBodyRow(index, { value: ($event.target as HTMLInputElement).value })" /></td>
              <td v-if="bodyType === 'formData'">
                <select class="cell select-cell" :value="row.kind ?? 'text'" @change="updateBodyRow(index, { kind: ($event.target as HTMLSelectElement).value as 'text' | 'file' })">
                  <option value="text">文本</option><option value="file">文件</option>
                </select>
              </td>
              <td class="c"><button class="del" type="button" aria-label="删除" @click="removeBodyRow(index)">×</button></td>
            </tr>
            <tr><td class="c" /><td colspan="4"><button class="link" type="button" @click="addBodyRow">+ 新增</button></td></tr>
          </tbody>
        </table>
        <label v-else-if="bodyType === 'binary'" class="field-stack">
          <span>本地文件路径</span>
          <input :value="body.type === 'binary' ? body.path : ''" spellcheck="false" placeholder="C:\path\to\payload.bin" @input="patchBodyField('path', ($event.target as HTMLInputElement).value)" />
        </label>
        <div v-else-if="body.type === 'graphQl'" class="graphql-grid">
          <label class="field-stack"><span>Query</span><textarea :value="body.query" spellcheck="false" @input="patchBodyField('query', ($event.target as HTMLTextAreaElement).value)" /></label>
          <label class="field-stack"><span>Variables (JSON)</span><textarea :value="body.variables" spellcheck="false" @input="patchBodyField('variables', ($event.target as HTMLTextAreaElement).value)" /></label>
          <label class="field-stack"><span>Operation Name（可选）</span><input :value="body.operationName ?? ''" @input="patchBodyField('operationName', ($event.target as HTMLInputElement).value)" /></label>
        </div>
        <p class="body-hint">文本内容按原文发送；文件项与 binary 使用流式上传。</p>
      </section>

      <section v-else-if="current === 'Auth'" class="auth-editor">
        <label class="field-stack auth-kind">
          <span>鉴权类型</span>
          <select :value="auth.type" @change="setAuthType(($event.target as HTMLSelectElement).value as AuthType)">
            <option v-for="option in AUTH_TYPES" :key="option.type" :value="option.type">{{ option.label }}</option>
          </select>
        </label>
        <p v-if="auth.type === 'inherit'" class="editor-empty">继承集合或文件夹的鉴权配置；当前未配置上级时等同无鉴权。</p>
        <p v-else-if="auth.type === 'none'" class="editor-empty">此请求不附加鉴权信息。</p>
        <template v-else-if="auth.type === 'basic'">
          <label class="field-stack"><span>用户名</span><input :value="auth.username" autocomplete="username" @input="patchAuth({ username: ($event.target as HTMLInputElement).value })" /></label>
          <label class="field-stack"><span>密码</span><input :value="auth.password" type="password" autocomplete="current-password" @input="patchAuth({ password: ($event.target as HTMLInputElement).value })" /></label>
        </template>
        <label v-else-if="auth.type === 'bearer'" class="field-stack"><span>Token</span><input :value="auth.token" type="password" autocomplete="off" @input="patchAuth({ token: ($event.target as HTMLInputElement).value })" /></label>
        <template v-else-if="auth.type === 'apiKey'">
          <label class="field-stack"><span>Key</span><input :value="auth.key" @input="patchAuth({ key: ($event.target as HTMLInputElement).value })" /></label>
          <label class="field-stack"><span>Value</span><input :value="auth.value" type="password" autocomplete="off" @input="patchAuth({ value: ($event.target as HTMLInputElement).value })" /></label>
          <label class="field-stack"><span>添加到</span><select :value="auth.location" @change="patchAuth({ location: ($event.target as HTMLSelectElement).value })"><option value="header">Header</option><option value="query">Query</option><option value="cookie">Cookie</option></select></label>
        </template>
      </section>

      <p v-else class="todo">
        {{ current }} —— 当前切片暂未接入。
      </p>
    </div>
  </section>
</template>

<style scoped>
.req { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
.tabs {
  height: var(--a-h-toolbar); flex: 0 0 auto;
  display: flex; align-items: center; gap: 2px; padding: 0 6px;
  border-bottom: 1px solid var(--a-border);
}
.tabs button {
  background: transparent; border: none; color: var(--a-text-dim);
  font: inherit; height: 24px; padding: 0 8px; border-radius: 4px; cursor: pointer;
}
.tabs button:hover { background: var(--a-hover); color: var(--a-text); }
.tabs button.on { color: var(--a-text); box-shadow: inset 0 -2px 0 var(--a-primary); border-radius: 0; }
.badge {
  font-size: var(--a-fs-xs); color: var(--a-text-faint);
  background: var(--a-bg-3); border-radius: var(--a-radius-pill); padding: 0 5px; margin-left: 2px;
}
.spacer { flex: 1 1 auto; }
.export-state { color: var(--a-success); font-size: 11px; }
.link { color: var(--a-primary) !important; }
.body { flex: 1 1 auto; overflow: auto; min-height: 0; }
.empty, .editor-empty { padding: 16px; color: var(--a-text-dim); font-size: 12px; }
.table-section + .table-section { border-top: 1px solid var(--a-border); }
.section-head { display: flex; align-items: center; gap: 8px; padding: 8px 8px 3px; font-size: 12px; }
.section-count { color: var(--a-text-faint); font-weight: normal; }
.section-hint { margin: 0; padding: 0 8px 5px; color: var(--a-text-faint); font-size: 11px; }
.kv { width: 100%; border-collapse: collapse; font-size: 12px; }
.kv th {
  text-align: left; font-weight: 500; color: var(--a-text-faint);
  padding: 4px 6px; border-bottom: 1px solid var(--a-border); position: sticky; top: 0;
  background: var(--a-bg-1);
}
.kv td { border-bottom: 1px solid var(--a-border); padding: 0; height: var(--a-row); }
.kv tr.off { opacity: 0.45; }
.c { width: 28px; text-align: center; }
.cell {
  width: 100%; height: var(--a-row); background: transparent; border: none;
  color: var(--a-text); font: 12px/1 var(--a-font-mono); padding: 0 6px;
}
.cell:focus { background: var(--a-bg-3); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.del { background: transparent; border: none; color: var(--a-text-faint); cursor: pointer; }
.del:hover { color: var(--a-error); }
.body-editor { display: flex; flex-direction: column; gap: 8px; padding: 8px; min-height: 100%; }
.body-types { display: flex; align-items: center; gap: 14px; flex: 0 0 auto; }
.body-type { display: inline-flex; align-items: center; gap: 4px; color: var(--a-text-dim); font-size: 12px; cursor: pointer; }
.body-type:has(input:checked) { color: var(--a-text); }
.payload {
  flex: 1 1 auto; min-height: 180px; width: 100%; resize: none;
  box-sizing: border-box; background: var(--a-bg-3); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); color: var(--a-text); padding: 10px;
  font: 12px/1.5 var(--a-font-mono);
}
.payload:focus, .content-type input:focus { border-color: var(--a-focus); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.content-type { display: flex; align-items: center; gap: 8px; color: var(--a-text-dim); font-size: 12px; }
.content-type input {
  flex: 1; height: 26px; box-sizing: border-box; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text);
  padding: 0 7px; font: 12px var(--a-font-mono);
}
.body-hint { margin: 0; color: var(--a-text-faint); font-size: 11px; }
.body-table { border: 1px solid var(--a-border); }
.select-cell { appearance: auto; }
.field-stack { display: flex; flex-direction: column; gap: 5px; color: var(--a-text-dim); font-size: 12px; }
.field-stack > input, .field-stack > select, .auth-editor select {
  min-height: 30px; box-sizing: border-box; background: var(--a-bg-3); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); color: var(--a-text); padding: 4px 8px; font: 12px var(--a-font-mono);
}
.graphql-grid { display: grid; grid-template-columns: 2fr 1fr; gap: 8px; min-height: 220px; }
.graphql-grid textarea { min-height: 180px; resize: vertical; background: var(--a-bg-3); border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text); padding: 8px; font: 12px/1.5 var(--a-font-mono); }
.graphql-grid label:last-child { grid-column: 1 / -1; }
.auth-editor { display: grid; grid-template-columns: minmax(180px, 1fr) minmax(240px, 2fr); align-content: start; gap: 12px; padding: 12px; }
.auth-kind { grid-column: 1 / -1; max-width: 280px; }
.auth-editor .editor-empty { padding: 0; grid-column: 1 / -1; }
.todo { padding: 12px; color: var(--a-text-dim); font-size: 12px; }
@media (max-width: 720px) {
  .graphql-grid, .auth-editor { grid-template-columns: 1fr; }
  .graphql-grid label:last-child, .auth-kind { grid-column: 1; }
}
</style>
