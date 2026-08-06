<script setup lang="ts">
import { computed, h, ref } from 'vue'
import type { DataTableColumns } from 'naive-ui'
import type { AuthConfig, Body, KeyValue } from '@/ipc'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { exportCurl } from '@/features/curl'
import {
  AppButton,
  AppCheckbox,
  AppDataTable,
  AppIcon,
  AppInput,
  AppRadioGroup,
  AppSelect,
  AppTooltip,
} from '@/components/ui'
import { IconClose, IconCloseCircle, IconDrag, IconInfo, IconRemoveCircle } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'

/**
 * 请求编辑区 —— v0.1 先落地 Params、Headers、Body 三条发送主链。
 *
 * Tab store 使用 shallowRef + markRaw，不允许在这里直接 v-model 修改行对象；
 * 所有编辑都复制数组后通过 tabs.patch 整体替换，保证界面和发送状态同步。
 */
const TABS = ['Params', 'Headers', 'Body', 'Auth', 'Scripts', 'Settings'] as const
type PanelTab = (typeof TABS)[number]
const TAB_HINTS: Partial<Record<PanelTab, string>> = {
  Params: 'Path 参数由 URL 中的 :id 或 {id} 自动识别；禁用的 Query 参数会保留在草稿中，但不会发送。',
  Headers: '请求头名称不区分大小写；重复名称由后端按最终请求规则处理。',
}
const SHOW_PARAMETER_TYPE = false
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
const pendingDelete = ref<string | null>(null)
const draggedRow = ref<{ kind: RowField; index: number } | null>(null)

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
  const rows = rowsFor(kind)
  const currentRow = rows[index]
  if (!currentRow) return
  const nextRow = { ...currentRow, ...delta }
  const empty = !nextRow.key && !nextRow.value && !nextRow.description
  patchRows(
    kind,
    empty
      ? rows.filter((_, rowIndex) => rowIndex !== index)
      : rows.map((row, rowIndex) => rowIndex === index ? nextRow : row),
  )
}

function removeRow(kind: RowField, index: number) {
  patchRows(kind, rowsFor(kind).filter((_, rowIndex) => rowIndex !== index))
  pendingDelete.value = null
}

function deleteKey(kind: RowField, index: number) {
  return `${kind}:${index}`
}

function confirmRemoveRow(kind: RowField, index: number) {
  const key = deleteKey(kind, index)
  if (pendingDelete.value === key) removeRow(kind, index)
  else pendingDelete.value = key
}

function startRowPointerDrag(kind: RowField, index: number, event: PointerEvent) {
  if (event.button !== 0) return
  const handle = event.currentTarget as HTMLElement
  draggedRow.value = { kind, index }
  handle.setPointerCapture(event.pointerId)

  const onMove = (moveEvent: PointerEvent) => moveEvent.preventDefault()
  const onEnd = (endEvent: PointerEvent) => {
    handle.releasePointerCapture(event.pointerId)
    handle.removeEventListener('pointermove', onMove)
    handle.removeEventListener('pointerup', onEnd)
    handle.removeEventListener('pointercancel', onCancel)

    const source = draggedRow.value
    draggedRow.value = null
    if (!source) return
    const target = document
      .elementFromPoint(endEvent.clientX, endEvent.clientY)
      ?.closest<HTMLElement>('[data-parameter-row]')
    if (!target || target.dataset.parameterKind !== source.kind) return
    const targetIndex = Number(target.dataset.parameterIndex)
    if (!Number.isInteger(targetIndex)) return

    const rows = [...rowsFor(source.kind)]
    const [moved] = rows.splice(source.index, 1)
    if (!moved) return
    const belowMiddle = endEvent.clientY > target.getBoundingClientRect().top + target.offsetHeight / 2
    let insertAt = targetIndex + (belowMiddle ? 1 : 0)
    if (source.index < insertAt) insertAt--
    insertAt = Math.max(0, Math.min(rows.length, insertAt))
    if (insertAt === source.index) return
    rows.splice(insertAt, 0, moved)
    patchRows(source.kind, rows)
    pendingDelete.value = null
  }
  const onCancel = () => {
    draggedRow.value = null
    handle.removeEventListener('pointermove', onMove)
    handle.removeEventListener('pointerup', onEnd)
    handle.removeEventListener('pointercancel', onCancel)
  }
  handle.addEventListener('pointermove', onMove)
  handle.addEventListener('pointerup', onEnd)
  handle.addEventListener('pointercancel', onCancel)
  event.preventDefault()
}

function editorRowProps(kind: RowField, row: EditableRow) {
  return row.draft ? {} : {
    'data-parameter-row': '',
    'data-parameter-kind': kind,
    'data-parameter-index': String(row.rowIndex),
  }
}

function setAllRowsEnabled(kind: RowField, enabled: boolean) {
  patchRows(kind, rowsFor(kind).map((row) => ({ ...row, enabled })))
}

function enabledHeader(kind: RowField) {
  const rows = rowsFor(kind)
  if (rows.length === 0) return null
  const enabledCount = rows.filter((row) => row.enabled).length
  return h(AppCheckbox, {
    checked: enabledCount === rows.length,
    indeterminate: enabledCount > 0 && enabledCount < rows.length,
    'aria-label': enabledCount === rows.length ? '禁用全部参数' : '启用全部参数',
    'onUpdate:checked': (checked: boolean) => setAllRowsEnabled(kind, checked),
  })
}

type EditableRow = KeyValue & { rowIndex: number; draft?: boolean }

function editorRows(section: EditorSection): EditableRow[] {
  const rows = section.rows.map((row, rowIndex) => ({ ...row, rowIndex }))
  if (section.kind === 'pathParams') return rows
  return [
    ...rows,
    {
      enabled: false,
      key: '',
      value: '',
      description: '',
      kind: 'text' as const,
      rowIndex: section.rows.length,
      draft: true,
    },
  ]
}

function updateEditableRow(
  kind: RowField,
  row: EditableRow,
  field: 'key' | 'value' | 'description',
  value: string,
) {
  if (!row.draft) {
    updateRow(kind, row.rowIndex, { [field]: value })
    return
  }
  if (!value) return
  patchRows(kind, [
    ...rowsFor(kind),
    { enabled: true, key: '', value: '', description: '', kind: 'text', [field]: value },
  ])
}

function textCell(kind: RowField, row: EditableRow, field: 'key' | 'value' | 'description') {
  return h(AppInput, {
    value: row[field] ?? '',
    size: 'small',
    bordered: false,
    inputProps: { spellcheck: false },
    placeholder: row.draft && field === 'key' ? '输入参数名' : undefined,
    'onUpdate:value': (value: string) => updateEditableRow(kind, row, field, value),
  })
}

function descriptionCell(kind: RowField, row: EditableRow) {
  const children = [h('div', {
    class: 'description-input',
    style: { flex: '1 1 0', width: '0', minWidth: '0' },
  }, [textCell(kind, row, 'description')])]
  if (!row.draft) {
    const confirming = pendingDelete.value === deleteKey(kind, row.rowIndex)
    children.push(h(AppButton, {
      class: 'row-delete',
      type: confirming ? 'error' : 'default',
      quaternary: true,
      circle: true,
      size: 'tiny',
      title: confirming ? '再次点击确认删除' : '删除参数',
      'aria-label': confirming ? '确认删除参数' : '删除参数',
      onClick: () => confirmRemoveRow(kind, row.rowIndex),
    }, { icon: () => h(AppIcon, { component: confirming ? IconCloseCircle : IconRemoveCircle, size: 17 }) }))
  }
  return h('div', {
    class: 'description-cell',
    style: {
      display: 'flex',
      flexDirection: 'row',
      alignItems: 'center',
      gap: '5px',
      width: '100%',
      minWidth: '0',
      flexWrap: 'nowrap',
    },
  }, children)
}

function editorColumns(kind: RowField): DataTableColumns<EditableRow> {
  const columns: DataTableColumns<EditableRow> = [
    {
      title: () => enabledHeader(kind), key: 'enabled', width: 52, align: 'center',
      render: (row) => row.draft ? null : h('div', { class: 'enabled-cell' }, [
        h('span', {
          class: 'row-drag-handle',
          title: '拖动调整参数顺序',
          'aria-label': '拖动调整参数顺序',
          onPointerdown: (event: PointerEvent) => startRowPointerDrag(kind, row.rowIndex, event),
        }, [h(AppIcon, { component: IconDrag, size: 15 })]),
        h(AppCheckbox, {
          checked: row.enabled,
          'aria-label': `启用 ${row.key || '空行'}`,
          'onUpdate:checked': (checked: boolean) => updateRow(kind, row.rowIndex, { enabled: checked }),
        }),
      ]),
    },
    { title: '参数名', key: 'key', minWidth: 150, render: (row) => textCell(kind, row, 'key') },
    { title: '参数值', key: 'value', minWidth: 180, render: (row) => textCell(kind, row, 'value') },
  ]
  if (SHOW_PARAMETER_TYPE) {
    columns.push({ title: '类型', key: 'kind', width: 100, render: (row) => row.kind ?? 'text' })
  }
  columns.push({ title: '说明', key: 'description', minWidth: 180, render: (row) => descriptionCell(kind, row) })
  return columns
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

const bodyTableRows = computed<EditableRow[]>(() => bodyRows().map((row, rowIndex) => ({ ...row, rowIndex })))
const bodyColumns = computed<DataTableColumns<EditableRow>>(() => {
  const columns: DataTableColumns<EditableRow> = [
    {
      key: 'enabled', width: 38, align: 'center',
      render: (row) => h(AppCheckbox, {
        checked: row.enabled,
        'onUpdate:checked': (checked: boolean) => updateBodyRow(row.rowIndex, { enabled: checked }),
      }),
    },
    {
      title: 'Key', key: 'key', minWidth: 160,
      render: (row) => h(AppInput, { value: row.key, bordered: false, 'onUpdate:value': (value: string) => updateBodyRow(row.rowIndex, { key: value }) }),
    },
    {
      title: 'Value', key: 'value', minWidth: 220,
      render: (row) => h(AppInput, { value: row.value, bordered: false, placeholder: row.kind === 'file' ? '本地文件路径' : '', 'onUpdate:value': (value: string) => updateBodyRow(row.rowIndex, { value }) }),
    },
  ]
  if (bodyType.value === 'formData') {
    columns.push({
      title: '类型', key: 'kind', width: 110,
      render: (row) => h(AppSelect, {
        value: row.kind ?? 'text', bordered: false,
        options: [{ label: '文本', value: 'text' }, { label: '文件', value: 'file' }],
        'onUpdate:value': (value: 'text' | 'file') => updateBodyRow(row.rowIndex, { kind: value }),
      }),
    })
  }
  columns.push({
    key: 'actions', width: 42, align: 'center',
    render: (row) => h(AppButton, {
      quaternary: true, circle: true, size: 'tiny', title: '删除', 'aria-label': '删除',
      onClick: () => removeBodyRow(row.rowIndex),
    }, { icon: () => h(AppIcon, { component: IconClose, size: 13 }) }),
  })
  return columns
})

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
  <NaiveThemeScope>
    <nav class="tabs" role="tablist">
      <span v-for="t in TABS" :key="t" class="tab-item">
        <button
          type="button"
          role="tab"
          :aria-selected="current === t"
          :class="{ on: current === t }"
          @click="current = t"
        >
          {{ t }}
        </button>
        <AppTooltip v-if="TAB_HINTS[t]">
          <template #trigger>
            <span class="tab-hint" tabindex="0" :aria-label="`${t} 说明`">
              <AppIcon :component="IconInfo" :size="13" />
            </span>
          </template>
          {{ TAB_HINTS[t] }}
        </AppTooltip>
      </span>
      <span class="spacer" />
      <span v-if="exportState" class="export-state" role="status">{{ exportState }}</span>
      <AppButton quaternary size="small" class="link" title="复制为 cURL" @click="copyCurl">导出 cURL</AppButton>
      <AppButton quaternary size="small" class="link" title="批量编辑将在请求模型稳定后接入">批量编辑</AppButton>
    </nav>

    <div v-if="!tab" class="empty">先新建一个请求，再开始编辑。</div>
    <div v-else class="body">
      <template v-if="current === 'Params' || current === 'Headers'">
        <section v-for="section in editorSections" :key="section.kind" class="parameter-table">
          <AppDataTable
            :columns="editorColumns(section.kind)"
            :data="editorRows(section)"
            :row-key="(row: EditableRow) => `${section.kind}-${row.rowIndex}`"
            :row-props="(row: EditableRow) => editorRowProps(section.kind, row)"
            :row-class-name="(row: EditableRow) => row.draft ? 'draft' : row.enabled ? '' : 'off'"
            :single-line="false"
            :bordered="false"
            size="small"
          />
        </section>
      </template>

      <section v-else-if="current === 'Body'" class="config-block body-editor">
        <header class="config-head body-type-head">
          <AppTooltip>
            <template #trigger><strong class="hint-title">请求体类型</strong></template>
            选择发送内容的编码方式。
          </AppTooltip>
          <AppRadioGroup
            :value="bodyType"
            :options="BODY_TYPES.map((option) => ({ label: option.label, value: option.type }))"
            name="request-body-type"
            @update:value="setBodyType($event as BodyType)"
          />
        </header>

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
          <AppInput :value="textContentType" :input-props="{ spellcheck: false }" @update:value="body.type === 'text' && patchTab({ body: { ...body, contentType: $event } })" />
        </label>
        <template v-else-if="bodyType === 'formData' || bodyType === 'urlEncoded'">
          <AppDataTable :columns="bodyColumns" :data="bodyTableRows" :row-key="(row: EditableRow) => row.rowIndex" :bordered="false" size="small" />
          <footer class="config-footer"><AppButton quaternary size="small" class="link" @click="addBodyRow">+ 新增</AppButton></footer>
        </template>
        <label v-else-if="bodyType === 'binary'" class="field-stack">
          <span>本地文件路径</span>
          <AppInput :value="body.type === 'binary' ? body.path : ''" :input-props="{ spellcheck: false }" placeholder="C:\path\to\payload.bin" @update:value="patchBodyField('path', $event)" />
        </label>
        <div v-else-if="body.type === 'graphQl'" class="graphql-grid">
          <label class="field-stack"><span>Query</span><textarea :value="body.query" spellcheck="false" @input="patchBodyField('query', ($event.target as HTMLTextAreaElement).value)" /></label>
          <label class="field-stack"><span>Variables (JSON)</span><textarea :value="body.variables" spellcheck="false" @input="patchBodyField('variables', ($event.target as HTMLTextAreaElement).value)" /></label>
          <label class="field-stack"><span>Operation Name（可选）</span><AppInput :value="body.operationName ?? ''" @update:value="patchBodyField('operationName', $event)" /></label>
        </div>
        <AppTooltip class="body-hint">
          <template #trigger><span class="hint-title">发送说明</span></template>
          文本内容按原文发送；文件项与 binary 使用流式上传。
        </AppTooltip>
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
.tabs button.on { color: var(--a-text); box-shadow: inset 0 -2px 0 var(--a-primary); border-radius: 0; }
.tab-item { display: inline-flex; align-items: center; height: 100%; }
.tab-item > button { padding-right: 3px; }
.tab-hint {
  width: 18px; height: 22px;
  display: inline-flex; align-items: center; justify-content: center;
  color: var(--a-text-faint); cursor: help;
}
.tab-hint:hover, .tab-hint:focus-visible { color: var(--a-text); }
.badge {
  font-size: var(--a-fs-xs); color: var(--a-text-faint);
  background: var(--a-bg-3); border-radius: var(--a-radius-pill); padding: 0 5px; margin-left: 2px;
}
.spacer { flex: 1 1 auto; }
.export-state { color: var(--a-success); font-size: 11px; }
.link { color: var(--a-primary) !important; }
.body { flex: 1 1 auto; overflow: auto; min-height: 0; padding: 8px; }
.empty, .editor-empty { padding: 16px; color: var(--a-text-dim); font-size: 12px; }
.config-block {
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
}
.config-block + .config-block { margin-top: 8px; }
.parameter-table + .parameter-table { margin-top: 8px; }
.config-head, .section-head {
  display: flex; align-items: center; gap: 8px;
  min-height: 34px; padding: 0 10px;
  border-bottom: 1px solid var(--a-border);
  background: var(--a-bg-2);
  font-size: 12px;
}
.section-head strong { font-weight: 600; }
.body-type-head { justify-content: space-between; }
.config-footer {
  min-height: 30px; display: flex; align-items: center;
  border-top: 1px solid var(--a-border); padding: 0 6px;
}
.hint-title { cursor: help; border-bottom: 1px dotted var(--a-text-faint); }
.app-data-table :deep(.n-data-table-th) { font-size: 11px; color: var(--a-text-faint); }
.app-data-table :deep(.n-data-table-td) { height: 38px; padding: 0 6px; }
.app-data-table :deep(.off) { opacity: 0.45; }
.app-data-table :deep(.draft) { opacity: 0.72; }
.app-data-table :deep(.n-data-table-th),
.app-data-table :deep(.n-data-table-td) {
  border-right: 0 !important;
  border-bottom: 0 !important;
}
.row-drag-handle {
  position: absolute; top: 50%; left: -24px;
  transform: translateY(-50%);
  width: 24px; height: 30px;
  display: inline-flex; align-items: center; justify-content: center;
  color: var(--a-text-faint); cursor: grab; opacity: 0;
  touch-action: none; user-select: none;
}
.app-data-table :deep(.n-data-table-tr:hover) .row-drag-handle,
.row-drag-handle:focus-visible { opacity: 1; }
.row-drag-handle:hover { cursor: grab; color: var(--a-text); }
.row-drag-handle:active { cursor: grabbing; color: var(--a-text); }
.enabled-cell {
  position: relative;
  width: 100%; height: 38px;
  display: flex; align-items: center; justify-content: center;
}
.parameter-table { padding-left: 24px; }
.app-data-table :deep(.n-input),
.app-data-table :deep(.n-input__input-el) { height: 32px; min-height: 32px; }
.description-cell {
  width: 100%; min-width: 0;
  display: flex;
  flex-direction: row;
  align-items: center;
  gap: 5px;
}
.description-cell .app-input { flex: 1 1 0; width: 0; min-width: 0; }
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
  color: var(--a-text); font: 12px/1 var(--a-font-mono); padding: 0 6px;
}
.cell:focus { background: var(--a-bg-3); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.del { width: 24px; height: 24px; display: inline-flex; align-items: center; justify-content: center; background: transparent; border: none; color: var(--a-text-faint); cursor: pointer; }
.del:hover { color: var(--a-error); }
.body-editor { display: flex; flex-direction: column; gap: 8px; min-height: 100%; }
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
.body-hint { margin: 0 2px; color: var(--a-text-faint); font-size: 11px; }
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
.auth-editor > .auth-kind { margin: -12px -12px 0; padding: 12px; border-bottom: 1px solid var(--a-border); }
.auth-kind { grid-column: 1 / -1; max-width: 280px; }
.auth-editor .editor-empty { padding: 0; grid-column: 1 / -1; }
.todo { padding: 12px; color: var(--a-text-dim); font-size: 12px; }
@media (max-width: 720px) {
  .graphql-grid, .auth-editor { grid-template-columns: 1fr; }
  .graphql-grid label:last-child, .auth-kind { grid-column: 1; }
}
</style>
