<script setup lang="ts">
import { computed, ref } from 'vue'
import type { Body, KeyValue } from '@/ipc'
import { useTabsStore, type Tab } from '@/stores/tabs'

/**
 * 请求编辑区 —— v0.1 先落地 Params、Headers、Body 三条发送主链。
 *
 * Tab store 使用 shallowRef + markRaw，不允许在这里直接 v-model 修改行对象；
 * 所有编辑都复制数组后通过 tabs.patch 整体替换，保证界面和发送状态同步。
 */
const TABS = ['Params', 'Headers', 'Body', 'Auth', 'Scripts', 'Settings'] as const
type PanelTab = (typeof TABS)[number]
type RowField = 'params' | 'pathParams' | 'headers'
type BodyType = 'none' | 'json' | 'text'

type BodyOption = { type: BodyType; label: string }
const BODY_TYPES: BodyOption[] = [
  { type: 'none', label: 'none' },
  { type: 'json', label: 'JSON' },
  { type: 'text', label: 'raw' },
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
  } else {
    nextBody = {
      type: 'text',
      content,
      contentType: currentBody.type === 'text' ? currentBody.contentType : 'text/plain',
    }
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
          v-else
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
        <p class="body-hint">JSON 内容按原文发送；格式校验和格式化将在编辑器增强阶段加入。</p>
      </section>

      <p v-else class="todo">
        {{ current }} —— 当前切片暂未接入。先完成 Params、Headers、Body 后再实现鉴权、脚本和请求设置。
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
.todo { padding: 12px; color: var(--a-text-dim); font-size: 12px; }
</style>
