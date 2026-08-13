<script setup lang="ts">
import { computed, defineAsyncComponent, ref } from 'vue'
import { open as openFileDialog } from '@tauri-apps/plugin-dialog'
import type { AuthConfig, Body, KeyValue } from '@/ipc'
import { useTabsStore, type Tab } from '@/stores/tabs'
import {
  AppButton,
  AppIcon,
  AppInput,
  AppInputNumber,
  AppRadioGroup,
  AppSelect,
  AppTooltip,
} from '@/components/ui'
import { IconFolderOpen } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import ParameterEditor from './ParameterEditor.vue'
import VariableInput from './VariableInput.vue'
import { commonHeadersFor, isKnownOfficialHeader } from '@/features/commonHeaders'
import { booleanSetting, httpVersionSetting, numberSetting, settingBooleanValue } from '@/features/requestSettings'

/**
 * 请求编辑区 —— v0.1 先落地 Params、Headers、Body 三条发送主链。
 *
 * Tab store 使用 shallowRef + markRaw，不允许在这里直接 v-model 修改行对象；
 * 所有编辑都复制数组后通过 tabs.patch 整体替换，保证界面和发送状态同步。
 */
const TABS = ['Params', 'Headers', 'Body', 'Auth', 'Settings'] as const
const JsonBodyEditor = defineAsyncComponent(() => import('./JsonBodyEditor.vue'))
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
}

const tabs = useTabsStore()
const tab = computed(() => tabs.active)
const current = ref<PanelTab>('Params')
const props = withDefaults(defineProps<{
  showCommonHeaders?: boolean
}>(), {
  showCommonHeaders: false,
})
const emit = defineEmits<{
  'update:showCommonHeaders': [show: boolean]
}>()
const headers = computed<KeyValue[]>(() => tab.value?.headers ?? [])
const visibleHeaderIndices = computed(() => headers.value.flatMap((header, index) =>
  props.showCommonHeaders || !isKnownOfficialHeader(header.key) ? [index] : [],
))
const visibleHeaders = computed(() => visibleHeaderIndices.value
  .map((index) => headers.value[index])
  .filter((header): header is KeyValue => header != null))
const commonHeaders = computed(() => commonHeadersFor(headers.value))

function toggleCommonHeaders() {
  emit('update:showCommonHeaders', !props.showCommonHeaders)
}

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
      })
    }
    sections.push({
      kind: 'params',
      label: 'Query 参数',
      rows: currentTab.params,
    })
    return sections
  }

  if (current.value === 'Headers') {
    return [{
      kind: 'headers',
      label: '请求 Headers',
      rows: visibleHeaders.value,
    }]
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

/** 把界面中的 Header 行索引映射回完整的持久化 Header 数组。 */
function sourceIndexFor(kind: RowField, displayedIndex: number): number | undefined {
  const sourceIndex = kind === 'headers'
    ? visibleHeaderIndices.value[displayedIndex]
    : displayedIndex
  return sourceIndex != null && sourceIndex < rowsFor(kind).length ? sourceIndex : undefined
}

function updateRow(kind: RowField, index: number, delta: Partial<KeyValue>) {
  const sourceIndex = sourceIndexFor(kind, index)
  if (sourceIndex == null) return
  const rows = rowsFor(kind)
  const currentRow = rows[sourceIndex]
  if (!currentRow) return
  const nextRow = { ...currentRow, ...delta }
  const empty = !nextRow.key && !nextRow.value && !nextRow.description
  patchRows(
    kind,
    empty
      ? rows.filter((_, rowIndex) => rowIndex !== sourceIndex)
      : rows.map((row, rowIndex) => rowIndex === sourceIndex ? nextRow : row),
  )
}

function removeRow(kind: RowField, index: number) {
  const sourceIndex = sourceIndexFor(kind, index)
  if (sourceIndex == null) return
  patchRows(kind, rowsFor(kind).filter((_, rowIndex) => rowIndex !== sourceIndex))
}

function setAllRowsEnabled(kind: RowField, enabled: boolean) {
  if (kind !== 'headers') {
    patchRows(kind, rowsFor(kind).map((row) => ({ ...row, enabled })))
    return
  }

  const visible = new Set(visibleHeaderIndices.value)
  patchRows('headers', rowsFor('headers').map((row, index) =>
    visible.has(index) ? { ...row, enabled } : row,
  ))
}

function updateParameterField(
  kind: RowField,
  index: number,
  field: 'key' | 'value' | 'description',
  value: string,
) {
  if (sourceIndexFor(kind, index) != null) {
    updateRow(kind, index, { [field]: value })
    return
  }
  if (!value) return
  patchRows(kind, [
    ...rowsFor(kind),
    { enabled: true, key: '', value: '', description: '', kind: 'text', [field]: value },
  ])
}

function reorderRows(kind: RowField, from: number, to: number) {
  if (kind !== 'headers') {
    const rows = [...rowsFor(kind)]
    const [moved] = rows.splice(from, 1)
    if (!moved) return
    rows.splice(to, 0, moved)
    patchRows(kind, rows)
    return
  }

  const sourceIndices = visibleHeaderIndices.value
  if (from === to || to < 0 || to >= sourceIndices.length) return
  const displayedRows = sourceIndices.map((index) => rowsFor('headers')[index]).filter((row): row is KeyValue => row != null)
  const [moved] = displayedRows.splice(from, 1)
  if (!moved) return
  displayedRows.splice(to, 0, moved)

  const nextRows = [...rowsFor('headers')]
  sourceIndices.forEach((sourceIndex, displayedIndex) => {
    const row = displayedRows[displayedIndex]
    if (row) nextRows[sourceIndex] = row
  })
  patchRows('headers', nextRows)
}

function displayedRowsFor(kind: RowField): KeyValue[] {
  return kind === 'headers' ? visibleHeaders.value : rowsFor(kind)
}

/** 用批量编辑结果替换可见 Header，保留默认隐藏的通用 Header。 */
function patchVisibleHeaders(nextVisibleRows: KeyValue[]) {
  const visible = new Set(visibleHeaderIndices.value)
  let nextVisibleIndex = 0
  const nextRows = rowsFor('headers').flatMap((row, index) => {
    if (!visible.has(index)) return [row]
    const replacement = nextVisibleRows[nextVisibleIndex]
    nextVisibleIndex += 1
    return replacement ? [replacement] : []
  })
  patchRows('headers', [...nextRows, ...nextVisibleRows.slice(nextVisibleIndex)])
}

/**
 * 批量编辑：Params 只编辑 Query 参数（Path 参数由 URL 自动识别，不参与批量），
 * Headers 仅编辑当前可见项；默认隐藏的通用 Header 会原样保留。
 */
const bulkEditing = ref(false)
const bulkDraft = ref('')

const bulkKind = computed<RowField | null>(() => {
  if (current.value === 'Params') return 'params'
  if (current.value === 'Headers') return 'headers'
  return null
})

function rowsToBulkText(rows: KeyValue[]): string {
  return rows
    .map((row) => {
      const pair = row.value ? `${row.key}: ${row.value}` : row.key
      return row.enabled ? pair : `// ${pair}`
    })
    .join('\n')
}

function bulkTextToRows(text: string): KeyValue[] {
  return text.split('\n').flatMap((line): KeyValue[] => {
    let enabled = true
    let raw = line.trim()
    if (raw.startsWith('//')) {
      enabled = false
      raw = raw.slice(2).trimStart()
    }
    if (!raw) return []
    const colon = raw.indexOf(':')
    const tab = raw.indexOf('\t')
    let sep = colon
    if (sep === -1 || (tab !== -1 && tab < sep)) sep = tab
    const key = sep === -1 ? raw : raw.slice(0, sep).trim()
    if (!key) return []
    const value = sep === -1 ? '' : raw.slice(sep + 1).trim()
    return [{ enabled, key, value, description: '', kind: 'text' }]
  })
}

function openBulkEdit() {
  const kind = bulkKind.value
  if (!kind || !tab.value) return
  bulkDraft.value = rowsToBulkText(displayedRowsFor(kind))
  bulkEditing.value = true
}

function applyBulkEdit() {
  const kind = bulkKind.value
  if (kind === 'headers') patchVisibleHeaders(bulkTextToRows(bulkDraft.value))
  else if (kind) patchRows(kind, bulkTextToRows(bulkDraft.value))
  bulkEditing.value = false
}

function cancelBulkEdit() {
  bulkEditing.value = false
}

function selectTab(next: PanelTab) {
  if (next === current.value) return
  if (bulkEditing.value) applyBulkEdit()
  current.value = next
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

function bodyRows(): KeyValue[] {
  return body.value.type === 'formData' || body.value.type === 'urlEncoded' ? body.value.rows : []
}

function patchBodyRows(rows: KeyValue[]) {
  if (body.value.type === 'formData') patchTab({ body: { type: 'formData', rows } })
  else if (body.value.type === 'urlEncoded') patchTab({ body: { type: 'urlEncoded', rows } })
}

function updateBodyRow(index: number, delta: Partial<KeyValue>) {
  const rows = bodyRows()
  if (index < rows.length) {
    patchBodyRows(rows.map((row, rowIndex) => rowIndex === index ? { ...row, ...delta } : row))
    return
  }
  patchBodyRows([...rows, { enabled: true, key: '', value: '', description: '', kind: 'text', ...delta }])
}

function removeBodyRow(index: number) {
  patchBodyRows(bodyRows().filter((_, rowIndex) => rowIndex !== index))
}

function reorderBodyRows(from: number, to: number) {
  const rows = [...bodyRows()]
  const [moved] = rows.splice(from, 1)
  if (!moved) return
  rows.splice(to, 0, moved)
  patchBodyRows(rows)
}

function patchBodyField(field: string, value: string) {
  const currentBody = body.value
  if (currentBody.type === 'binary' && field === 'path') {
    patchTab({ body: { ...currentBody, path: value } })
  } else if (currentBody.type === 'graphQl') {
    patchTab({ body: { ...currentBody, [field]: value || (field === 'operationName' ? undefined : '') } })
  }
}

async function chooseBinaryFile() {
  const selected = await openFileDialog({ multiple: false })
  if (typeof selected === 'string') patchBodyField('path', selected)
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

function patchBooleanSetting(field: 'followRedirects' | 'verifyTls', value: string | number | null) {
  patchTab({ settings: { ...tab.value?.settings, [field]: booleanSetting(value) } })
}

function patchNumberSetting(field: 'timeoutMs' | 'maxRedirects', value: number | null) {
  patchTab({ settings: { ...tab.value?.settings, [field]: numberSetting(value) } })
}

function patchVersion(value: string | number | null) {
  patchTab({ settings: { ...tab.value?.settings, httpVersion: httpVersionSetting(value) } })
}
</script>

<template>
  <section class="req">
  <NaiveThemeScope>
    <nav class="tabs" role="tablist">
      <span v-for="t in TABS" :key="t" class="tab-item">
        <button
          type="button"
          role="tab"
          :aria-selected="current === t"
          :class="{ on: current === t }"
          @click="selectTab(t)"
        >
          {{ t }}
        </button>
      </span>
      <span class="spacer" />
      <template v-if="bulkKind">
        <template v-if="bulkEditing">
          <AppButton quaternary size="small" title="放弃本次批量编辑，返回表格" @click="cancelBulkEdit">取消</AppButton>
          <AppButton quaternary size="small" class="link" title="应用批量编辑，返回表格" @click="applyBulkEdit">完成</AppButton>
        </template>
        <AppButton v-else quaternary size="small" class="link" title="以纯文本批量编辑 key: value 列表" @click="openBulkEdit">批量编辑</AppButton>
      </template>
    </nav>

    <div v-if="!tab" class="empty">先新建一个请求，再开始编辑。</div>
    <div v-else class="body">
      <template v-if="current === 'Params' || current === 'Headers'">
        <section v-if="bulkEditing" class="parameter-table bulk-editor">
          <textarea
            v-model="bulkDraft"
            class="bulk-textarea"
            spellcheck="false"
            aria-label="批量编辑内容"
            :placeholder="'每行一个参数，格式：key: value\n以 // 开头的行应用后禁用该参数'"
          />
          <p class="bulk-hint">每行一个参数；`key: value`，以 `//` 开头的行应用后禁用该参数。</p>
        </section>
        <template v-else>
          <section v-for="section in editorSections" :key="section.kind" class="parameter-group">
            <header class="section-head">
              <strong>{{ section.label }}</strong>
              <span>{{ section.rows.length + (section.kind === 'headers' && props.showCommonHeaders ? commonHeaders.length : 0) }} 项</span>
              <button
                v-if="section.kind === 'headers'"
                type="button"
                class="common-header-toggle"
                :aria-pressed="props.showCommonHeaders"
                :title="props.showCommonHeaders ? '隐藏客户端和协议自动生成的通用 Header' : '显示客户端和协议自动生成的通用 Header'"
                @click="toggleCommonHeaders"
              >
                {{ props.showCommonHeaders ? '隐藏通用 Header' : '显示通用 Header' }}
              </button>
            </header>
            <div v-if="section.kind === 'headers' && props.showCommonHeaders" class="common-headers" role="table" aria-label="通用请求 Headers">
              <div v-if="commonHeaders.length" class="common-header-table">
                <div class="common-header-row common-header-head" role="row"><span>名称</span><span>值</span><span>来源</span></div>
                <div v-for="header in commonHeaders" :key="header.key" class="common-header-row" role="row">
                  <code>{{ header.key }}</code><span>{{ header.value }}</span><span>自动附加</span>
                </div>
              </div>
              <p v-else class="common-header-empty">当前请求已显式配置所有可预览的通用 Header。</p>
            </div>
            <div class="parameter-table">
              <ParameterEditor
                :rows="section.rows"
                :allow-create="section.kind !== 'pathParams'"
                @update-field="(index, field, value) => updateParameterField(section.kind, index, field, value)"
                @update-enabled="(index, enabled) => updateRow(section.kind, index, { enabled })"
                @set-all-enabled="setAllRowsEnabled(section.kind, $event)"
                @remove="removeRow(section.kind, $event)"
                @reorder="(from, to) => reorderRows(section.kind, from, to)"
              />
            </div>
          </section>
        </template>
      </template>

      <section v-else-if="current === 'Body'" class="config-block body-editor">
        <header class="config-head body-type-head">
          <AppTooltip>
            <template #trigger><strong class="hint-title">请求体类型</strong></template>
            选择发送内容的编码方式。
          </AppTooltip>
          <AppRadioGroup
            class="body-types"
            :value="bodyType"
            :options="BODY_TYPES.map((option) => ({ label: option.label, value: option.type }))"
            name="request-body-type"
            @update:value="setBodyType($event as BodyType)"
          />
        </header>

        <div class="body-content">
          <div v-if="bodyType === 'none'" class="editor-empty">此请求不发送请求体。</div>
        <JsonBodyEditor
          v-else-if="bodyType === 'json'"
          :value="bodyContent"
          @update:value="body.type === 'json' && patchTab({ body: { type: 'json', content: $event } })"
        />
        <VariableInput
          v-else-if="bodyType === 'text'"
          class="payload"
          :value="bodyContent"
          multiline
          :placeholder="bodyPlaceholder"
          spellcheck="false"
          aria-label="请求体内容"
          @update:value="body.type === 'text' && patchTab({ body: { ...body, content: $event } })"
        />
        <label v-if="bodyType === 'text'" class="content-type">
          Content-Type
          <AppInput :value="textContentType" :input-props="{ spellcheck: false }" @update:value="body.type === 'text' && patchTab({ body: { ...body, contentType: $event } })" />
        </label>
        <template v-else-if="bodyType === 'formData' || bodyType === 'urlEncoded'">
          <div class="body-parameter-table">
            <ParameterEditor
              :rows="bodyRows()"
              allow-create
              :show-kind="bodyType === 'formData'"
              @update-field="(index, field, value) => updateBodyRow(index, { [field]: value })"
              @update-enabled="(index, enabled) => updateBodyRow(index, { enabled })"
              @update-kind="(index, kind) => updateBodyRow(index, { kind })"
              @set-all-enabled="(enabled) => patchBodyRows(bodyRows().map((row) => ({ ...row, enabled })))"
              @remove="removeBodyRow"
              @reorder="(from, to) => reorderBodyRows(from, to)"
            />
          </div>
        </template>
        <label v-else-if="bodyType === 'binary'" class="field-stack">
          <span>本地文件路径</span>
          <div class="binary-path">
            <AppInput :value="body.type === 'binary' ? body.path : ''" :input-props="{ spellcheck: false }" placeholder="C:\path\to\payload.bin" @update:value="patchBodyField('path', $event)" />
            <AppButton secondary size="small" @click="chooseBinaryFile">
              <template #icon><AppIcon :component="IconFolderOpen" :size="15" /></template>
              选择文件
            </AppButton>
          </div>
        </label>
        <div v-else-if="body.type === 'graphQl'" class="graphql-grid">
          <label class="field-stack"><span>Query</span><VariableInput :value="body.query" multiline spellcheck="false" @update:value="patchBodyField('query', $event)" /></label>
          <label class="field-stack"><span>Variables (JSON)</span><VariableInput :value="body.variables" multiline spellcheck="false" @update:value="patchBodyField('variables', $event)" /></label>
          <label class="field-stack"><span>Operation Name（可选）</span><AppInput :value="body.operationName ?? ''" @update:value="patchBodyField('operationName', $event)" /></label>
        </div>
        </div>
      </section>

      <section v-else-if="current === 'Auth'" class="config-block auth-editor">
        <label class="field-stack auth-kind">
          <span>鉴权类型</span>
          <AppSelect :value="auth.type" :options="AUTH_TYPES.map((option) => ({ label: option.label, value: option.type }))" @update:value="setAuthType($event as AuthType)" />
        </label>
        <p v-if="auth.type === 'inherit'" class="editor-empty">继承集合或文件夹的鉴权配置；当前未配置上级时等同无鉴权。</p>
        <p v-else-if="auth.type === 'none'" class="editor-empty">此请求不附加鉴权信息。</p>
        <template v-else-if="auth.type === 'basic'">
          <label class="field-stack"><span>用户名</span><AppInput :value="auth.username" autocomplete="username" @update:value="patchAuth({ username: $event })" /></label>
          <label class="field-stack"><span>密码</span><AppInput :value="auth.password" type="password" show-password-on="click" autocomplete="current-password" @update:value="patchAuth({ password: $event })" /></label>
        </template>
        <label v-else-if="auth.type === 'bearer'" class="field-stack"><span>Token</span><AppInput :value="auth.token" type="password" show-password-on="click" autocomplete="off" @update:value="patchAuth({ token: $event })" /></label>
        <template v-else-if="auth.type === 'apiKey'">
          <label class="field-stack"><span>Key</span><AppInput :value="auth.key" @update:value="patchAuth({ key: $event })" /></label>
          <label class="field-stack"><span>Value</span><AppInput :value="auth.value" type="password" show-password-on="click" autocomplete="off" @update:value="patchAuth({ value: $event })" /></label>
          <label class="field-stack"><span>添加到</span><AppSelect :value="auth.location" :options="[{ label: 'Header', value: 'header' }, { label: 'Query', value: 'query' }, { label: 'Cookie', value: 'cookie' }]" @update:value="patchAuth({ location: $event })" /></label>
        </template>
      </section>

      <section v-else-if="current === 'Settings'" class="config-block request-settings">
        <header class="section-head"><strong>请求设置</strong><span>未设置的项目继承上级默认值</span></header>
        <div class="settings-grid">
          <label class="field-stack">
            <span>超时（毫秒）</span>
            <AppInputNumber :value="tab.settings.timeoutMs ?? null" :min="0" :step="1000" placeholder="继承" @update:value="patchNumberSetting('timeoutMs', $event)" />
          </label>
          <label class="field-stack">
            <span>最大重定向次数</span>
            <AppInputNumber :value="tab.settings.maxRedirects ?? null" :min="0" :max="20" :step="1" placeholder="继承" @update:value="patchNumberSetting('maxRedirects', $event)" />
          </label>
          <label class="field-stack">
            <span>跟随重定向</span>
            <AppSelect :value="settingBooleanValue(tab.settings.followRedirects)" :options="[{ label: '继承', value: 'inherit' }, { label: '开启', value: 'true' }, { label: '关闭', value: 'false' }]" @update:value="patchBooleanSetting('followRedirects', $event)" />
          </label>
          <label class="field-stack">
            <span>TLS 证书校验</span>
            <AppSelect :value="settingBooleanValue(tab.settings.verifyTls)" :options="[{ label: '继承', value: 'inherit' }, { label: '校验', value: 'true' }, { label: '忽略（不安全）', value: 'false' }]" @update:value="patchBooleanSetting('verifyTls', $event)" />
          </label>
          <label class="field-stack">
            <span>HTTP 版本</span>
            <AppSelect :value="tab.settings.httpVersion ?? 'inherit'" :options="[{ label: '继承', value: 'inherit' }, { label: 'HTTP/1.1', value: 'http1' }, { label: 'HTTP/2', value: 'http2' }]" @update:value="patchVersion($event)" />
          </label>
        </div>
      </section>

      <p v-else class="todo">
        {{ current }} —— 当前切片暂未接入。
      </p>
    </div>
  </NaiveThemeScope>
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
.tabs button.on { color: var(--a-primary); background: var(--a-selected); box-shadow: none; border-radius: 6px; }
.tab-item { display: inline-flex; align-items: center; height: 100%; }
.badge {
  font-size: var(--a-fs-xs); color: var(--a-text-faint);
  background: var(--a-bg-3); border-radius: var(--a-radius-pill); padding: 0 5px; margin-left: 2px;
}
.spacer { flex: 1 1 auto; }
.link { color: var(--a-primary) !important; }
.bulk-editor { display: flex; flex-direction: column; gap: 8px; min-height: 100%; }
.bulk-textarea {
  flex: 1 1 auto; min-height: 240px; width: 100%; box-sizing: border-box;
  background: var(--a-bg-3); border: 1px solid var(--a-border); border-radius: var(--a-radius-md);
  color: var(--a-text); caret-color: var(--a-text); padding: 10px; font: 12px/1.6 var(--a-font-editor);
  /* 原生 textarea + 全局自定义滚动条的组合在 WebView2 下偶发光标消失，
     显式声明 text 光标强制 WebView 重绘指针。 */
  cursor: text;
}
.bulk-textarea:focus { outline: 2px solid var(--a-focus); outline-offset: -2px; border-color: var(--a-focus); }
.bulk-hint { margin: 0 2px; color: var(--a-text-faint); font-size: 11px; }
.body { flex: 1 1 auto; overflow: auto; min-height: 0; padding: 8px; }
.empty, .editor-empty { padding: 16px; color: var(--a-text-dim); font-size: 12px; }
.config-block {
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
}
.config-block + .config-block { margin-top: 8px; }
.parameter-group { overflow: hidden; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-2); }
.parameter-group + .parameter-group { margin-top: 8px; }
.parameter-group > .section-head span { margin-left: auto; color: var(--a-text-faint); font-size: 11px; font-weight: 400; }
.common-header-toggle {
  flex: 0 0 auto; height: 24px; padding: 0 7px; border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm); background: transparent; color: var(--a-text-dim);
  font: 11px var(--a-font-ui); cursor: pointer;
}
.common-header-toggle:hover { border-color: var(--a-primary); color: var(--a-primary); }
.common-header-toggle[aria-pressed='true'] { border-color: var(--a-primary); background: var(--a-selected); color: var(--a-primary); }
.common-headers { border-bottom: 1px solid var(--a-border); background: color-mix(in srgb, var(--a-bg-1) 48%, transparent); }
.common-header-table { overflow-x: auto; }
.common-header-row {
  min-height: 28px; display: grid; grid-template-columns: minmax(120px, 28%) minmax(180px, 1fr) 84px;
  border-bottom: 1px solid var(--a-border); color: var(--a-text-soft); font-size: 11px;
}
.common-header-row:last-child { border-bottom: 0; }
.common-header-row > * { min-width: 0; padding: 5px 9px; overflow-wrap: anywhere; border-right: 1px solid var(--a-border); }
.common-header-row > :last-child { border-right: 0; color: var(--a-text-faint); }
.common-header-row code { color: var(--a-post); font: 11px/18px var(--a-font-editor); }
.common-header-head { background: var(--a-bg-3); color: var(--a-text-faint); font-weight: var(--a-fw-medium); }
.common-header-head span { color: inherit; }
.common-header-empty { margin: 0; padding: 9px 10px; color: var(--a-text-faint); font-size: 11px; }
.parameter-table { min-width: 0; overflow-x: auto; padding: 8px; }
.config-head, .section-head {
  display: flex; align-items: center; gap: 8px;
  min-height: 34px; padding: 0 10px;
  border-bottom: 1px solid var(--a-border);
  background: var(--a-bg-2);
  font-size: 12px;
}
.section-head strong { font-weight: 600; }
.body-type-head { justify-content: space-between; }
.hint-title { cursor: help; border-bottom: 1px dotted var(--a-text-faint); }
.app-data-table :deep(.n-data-table-th) { font-size: 11px; color: var(--a-text-faint); }
.app-data-table :deep(.n-data-table-td) { height: 38px; padding: 0 6px; }
.app-data-table :deep(.n-data-table-th),
.app-data-table :deep(.n-data-table-td) {
  border-right: 0 !important;
  border-bottom: 0 !important;
}
.app-data-table :deep(.n-input),
.app-data-table :deep(.n-input__input-el) { height: 32px; min-height: 32px; }
.description-input { flex: 1 1 0; width: 0; min-width: 0; }
.description-input .app-input { width: 100%; }
.row-delete { flex: 0 0 28px; align-self: center; color: var(--a-text-faint); }
.row-delete.n-button--error-type { color: var(--a-error); }
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
  color: var(--a-text); font: 12px/1 var(--a-font-editor); padding: 0 6px;
}
.cell:focus { background: var(--a-bg-3); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.del { width: 24px; height: 24px; display: inline-flex; align-items: center; justify-content: center; background: transparent; border: none; color: var(--a-text-faint); cursor: pointer; }
.del:hover { color: var(--a-error); }
.body-editor { display: flex; flex-direction: column; min-height: 100%; }
.body-content { display: flex; flex: 1 1 auto; flex-direction: column; gap: 8px; min-height: 0; padding: 8px; }
.body-parameter-table { min-width: 0; overflow-x: auto; }
.body-types { display: flex; align-items: center; gap: 14px; flex: 0 0 auto; }
.body-type { display: inline-flex; align-items: center; gap: 4px; color: var(--a-text-dim); font-size: 12px; cursor: pointer; }
.body-type:has(input:checked) { color: var(--a-text); }
.payload {
  flex: 1 1 auto; min-height: 180px; width: 100%; resize: none;
  box-sizing: border-box; background: var(--a-bg-3); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); color: var(--a-text); padding: 10px;
  font: 12px/1.5 var(--a-font-editor);
}
.payload :deep(.n-input-wrapper) { padding: 0; }
.payload :deep(.n-input__textarea-el) { min-height: 180px; padding: 10px; font: 12px/1.5 var(--a-font-editor); }
.payload:focus, .content-type input:focus { border-color: var(--a-focus); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.content-type { display: flex; align-items: center; gap: 8px; color: var(--a-text-dim); font-size: 12px; }
.content-type input {
  flex: 1; height: 26px; box-sizing: border-box; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text);
  padding: 0 7px; font: 12px var(--a-font-editor);
}
.binary-path { display: flex; align-items: center; gap: 8px; }
.binary-path :deep(.n-input) { flex: 1 1 auto; min-width: 0; }
.body-table { border: 1px solid var(--a-border); }
.select-cell { appearance: auto; }
.field-stack { display: flex; flex-direction: column; gap: 5px; color: var(--a-text-dim); font-size: 12px; }
.field-stack > input, .field-stack > select, .auth-editor select {
  min-height: 30px; box-sizing: border-box; background: var(--a-bg-3); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); color: var(--a-text); padding: 4px 8px; font: 12px var(--a-font-editor);
}
.graphql-grid { display: grid; grid-template-columns: 2fr 1fr; gap: 8px; min-height: 220px; }
.graphql-grid :deep(.n-input) { min-height: 180px; background: var(--a-bg-3); }
.graphql-grid :deep(.n-input__textarea-el) { min-height: 180px; padding: 8px; font: 12px/1.5 var(--a-font-editor); }
.graphql-grid label:last-child { grid-column: 1 / -1; }
.auth-editor { display: grid; grid-template-columns: minmax(180px, 1fr) minmax(240px, 2fr); align-content: start; gap: 12px; padding: 12px; }
.auth-editor > .auth-kind { margin: -12px -12px 0; padding: 12px; border-bottom: 1px solid var(--a-border); }
.auth-kind { grid-column: 1 / -1; max-width: 280px; }
.auth-editor .editor-empty { padding: 0; grid-column: 1 / -1; }
.request-settings { overflow: hidden; }
.request-settings .section-head span { margin-left: auto; color: var(--a-text-faint); font-size: 11px; font-weight: 400; }
.settings-grid { display: grid; grid-template-columns: repeat(2, minmax(180px, 1fr)); gap: 14px; padding: 12px; align-content: start; }
.settings-grid :deep(.n-input-number), .settings-grid :deep(.n-select) { width: 100%; }
.todo { padding: 12px; color: var(--a-text-dim); font-size: 12px; }
@media (max-width: 720px) {
  .graphql-grid, .auth-editor, .settings-grid { grid-template-columns: 1fr; }
  .graphql-grid label:last-child, .auth-kind { grid-column: 1; }
  .request-settings .section-head span { display: none; }
}
</style>
