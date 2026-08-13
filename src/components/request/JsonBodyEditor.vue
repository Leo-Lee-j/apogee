<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, useId, watch } from 'vue'
import { NPopover } from 'naive-ui'
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
import { defaultKeymap, indentWithTab } from '@codemirror/commands'
import { json } from '@codemirror/lang-json'
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { EditorState } from '@codemirror/state'
import { EditorView, highlightActiveLine, keymap, lineNumbers } from '@codemirror/view'
import { resolveVariablePreview } from '@/features/variablePreview'
import { useWorkspaceStore } from '@/stores/workspace'

/**
 * 可编辑的 JSON 请求体。
 *
 * 请求模型仍保留原始字符串：除了支持环境变量插值外，用户也可以故意发送不合法
 * JSON 来调试服务端。因此此组件只提供实时语法反馈与格式化，绝不拦截保存或发送。
 */
const props = withDefaults(defineProps<{
  value: string
  placeholder?: string
}>(), {
  placeholder: '{\n  "key": "value"\n}',
})

const emit = defineEmits<{
  'update:value': [value: string]
}>()

type Validation =
  | { state: 'empty' }
  | { state: 'valid' }
  | { state: 'template' }
  | { state: 'invalid'; message: string }

const workspace = useWorkspaceStore()
const host = ref<HTMLDivElement | null>(null)
const view = shallowRef<EditorView | null>(null)
const draft = ref(props.value)
const variablePickerOpen = ref(false)
const validationId = useId()
let synchronizingExternalValue = false

const variables = computed(() => {
  const environment = workspace.activeEnvironment
  if (!environment) return []

  const values = environment.variables
    .filter((variable) => variable.enabled && variable.key.trim())
    .map((variable) => ({ key: variable.key.trim(), value: variable.value }))
  if (environment.host.trim()) values.unshift({ key: 'host', value: environment.host.trim() })

  return values.map((variable) => ({
    ...variable,
    preview: resolveVariablePreview(variable.key, values),
  }))
})

function hasCompleteVariableTemplate(source: string): boolean {
  // 后端会接受任意非空名称并在发送前解析；完整占位符的最终 JSON 结构取决于变量值。
  return /\{\{[^{}]*\S[^{}]*\}\}/.test(source)
}

function describeJsonError(source: string, error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  // Chromium 的错误通常只有 position；将其转换为人可定位的行列号。
  const position = /position\s+(\d+)/i.exec(message)?.[1]
  if (!position || /line\s+\d+/i.test(message)) return message

  const offset = Number(position)
  if (!Number.isFinite(offset)) return message
  const preceding = source.slice(0, offset)
  const line = preceding.split('\n').length
  const column = offset - preceding.lastIndexOf('\n')
  return `第 ${line} 行，第 ${column} 列：${message}`
}

function validateJson(source: string): Validation {
  if (!source.trim()) return { state: 'empty' }
  // `{{enabled}}` 等占位符可能在解析后成为 true、数字、对象等任何 JSON 值。
  // 在不知道当前变量值时不应把这类可发送模板误判为不合法 JSON。
  if (hasCompleteVariableTemplate(source)) return { state: 'template' }

  try {
    JSON.parse(source)
    return { state: 'valid' }
  } catch (error) {
    return { state: 'invalid', message: describeJsonError(source, error) }
  }
}

const validation = computed(() => validateJson(draft.value))
const isFormatReady = computed(() => validation.value.state === 'valid')

function syncEditorAccessibility() {
  const editor = view.value
  if (!editor) return
  editor.contentDOM.setAttribute('aria-describedby', validationId)
  if (validation.value.state === 'invalid') editor.contentDOM.setAttribute('aria-invalid', 'true')
  else editor.contentDOM.removeAttribute('aria-invalid')
}

function commit(value: string) {
  draft.value = value
  emit('update:value', value)
}

function replaceEditorDocument(value: string) {
  const editor = view.value
  if (!editor || editor.state.doc.toString() === value) return

  synchronizingExternalValue = true
  editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: value } })
  synchronizingExternalValue = false
}

function formatJson(space?: number) {
  if (validation.value.state !== 'valid') return
  const formatted = JSON.stringify(JSON.parse(draft.value), null, space)
  commit(formatted)
  replaceEditorDocument(formatted)
  view.value?.focus()
}

function insertVariable(key: string) {
  const editor = view.value
  if (!editor) return

  const selection = editor.state.selection.main
  const value = `{{${key}}}`
  editor.dispatch({
    changes: { from: selection.from, to: selection.to, insert: value },
    selection: { anchor: selection.from + value.length },
    scrollIntoView: true,
  })
  variablePickerOpen.value = false
  editor.focus()
}

function mountEditor() {
  if (!host.value || view.value) return

  const extensions = [
    lineNumbers(),
    highlightActiveLine(),
    json(),
    closeBrackets(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    keymap.of([
      { key: 'Shift-Alt-f', run: () => { formatJson(2); return true } },
      indentWithTab,
      ...closeBracketsKeymap,
      ...defaultKeymap,
    ]),
    EditorView.contentAttributes.of({
      'aria-label': 'JSON 请求体内容',
      'aria-multiline': 'true',
      'aria-describedby': validationId,
    }),
    EditorView.updateListener.of((update) => {
      if (!update.docChanged || synchronizingExternalValue) return
      commit(update.state.doc.toString())
    }),
    EditorView.theme({
      '&': {
        height: '100%',
        backgroundColor: 'var(--a-bg-3)',
        color: 'var(--a-text)',
        // 与响应查看器共用结果字号设置；JSON 正文仍始终使用等宽字体。
        fontSize: 'var(--a-font-result)',
      },
      '.cm-scroller': { fontFamily: 'var(--a-font-editor)', lineHeight: 'var(--a-lh-result)' },
      '.cm-content': { padding: '10px 0 14px' },
      '.cm-line': { padding: '0 10px' },
      '.cm-gutters': {
        minHeight: '100%',
        border: 'none',
        backgroundColor: 'var(--a-bg-2)',
        color: 'var(--a-text-faint)',
      },
      '.cm-activeLine': { backgroundColor: 'var(--a-hover)' },
      '.cm-activeLineGutter': { backgroundColor: 'var(--a-hover)' },
      '.cm-selectionBackground, ::selection': { backgroundColor: 'var(--a-selected) !important' },
      '.cm-cursor': { borderLeftColor: 'var(--a-text)' },
    }),
  ]

  view.value = new EditorView({
    state: EditorState.create({ doc: draft.value, extensions }),
    parent: host.value,
  })
  syncEditorAccessibility()
}

watch(() => props.value, (value) => {
  if (value === draft.value) return
  draft.value = value
  replaceEditorDocument(value)
})
watch(validation, () => syncEditorAccessibility(), { flush: 'post' })

onMounted(mountEditor)
onBeforeUnmount(() => {
  view.value?.destroy()
  view.value = null
})
</script>

<template>
  <section class="json-body-editor" aria-label="JSON 请求体编辑器">
    <header class="json-toolbar">
      <strong>JSON</strong>
      <span class="json-state" :class="validation.state">
        <template v-if="validation.state === 'valid'">格式正确</template>
        <template v-else-if="validation.state === 'empty'">尚未输入内容</template>
        <template v-else-if="validation.state === 'template'">待变量解析</template>
        <template v-else>格式有误</template>
      </span>
      <span class="json-toolbar-spacer" />
      <NPopover
        trigger="click"
        placement="bottom-end"
        :show="variablePickerOpen"
        :show-arrow="false"
        @update:show="variablePickerOpen = $event"
      >
        <template #trigger>
          <button type="button" :disabled="!variables.length || !view">插入变量</button>
        </template>
        <div class="variable-menu" role="listbox" aria-label="当前环境变量">
          <button
            v-for="variable in variables"
            :key="variable.key"
            type="button"
            role="option"
            @mousedown.prevent="insertVariable(variable.key)"
          >
            <code>{{ variable.key }}</code>
            <span :class="{ invalid: variable.preview.error }">
              {{ variable.preview.error || variable.preview.value || '空值' }}
            </span>
          </button>
        </div>
      </NPopover>
      <button type="button" :disabled="!isFormatReady" title="格式化 JSON（Shift+Alt+F）" @click="formatJson(2)">格式化</button>
      <button type="button" :disabled="!isFormatReady" title="压缩 JSON" @click="formatJson()">压缩</button>
    </header>

    <div class="json-editor-frame" :class="{ invalid: validation.state === 'invalid' }">
      <div ref="host" class="json-editor-host" />
      <span v-if="validation.state === 'empty'" class="json-placeholder">{{ placeholder }}</span>
    </div>

    <p v-if="validation.state === 'invalid'" :id="validationId" class="json-error" role="alert">
      JSON 格式有误：{{ validation.message }}。仍会保留原始内容，便于调试服务端行为。
    </p>
    <p v-else-if="validation.state === 'template'" :id="validationId" class="json-help">
      检测到 <code v-pre>{{variable}}</code> 模板；发送前会按当前环境解析，因此此处不自动格式化或判定最终 JSON。
    </p>
    <p v-else :id="validationId" class="json-help">
      可用“插入变量”添加 <code v-pre>{{variable}}</code>；格式化只会处理不含待解析变量且语法正确的 JSON。
    </p>
  </section>
</template>

<style scoped>
.json-body-editor {
  display: flex;
  flex: 1 1 auto;
  flex-direction: column;
  min-height: 236px;
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-3);
}
.json-toolbar {
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 7px;
  min-height: 34px;
  padding: 0 8px 0 10px;
  border-bottom: 1px solid var(--a-border);
  background: var(--a-bg-2);
  font-size: 12px;
}
.json-toolbar strong { font-weight: var(--a-fw-medium); }
.json-toolbar-spacer { flex: 1 1 auto; }
.json-toolbar button {
  height: 24px;
  padding: 0 7px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text-dim);
  font: 11px var(--a-font-ui);
  cursor: pointer;
}
.json-toolbar button:hover:not(:disabled) { background: var(--a-hover); color: var(--a-text); }
.json-toolbar button:focus-visible { outline: 2px solid var(--a-focus); outline-offset: -1px; }
.json-toolbar button:disabled { color: var(--a-text-faint); cursor: not-allowed; opacity: .65; }
.json-state { font-size: 11px; }
.json-state.valid { color: var(--a-success); }
.json-state.empty { color: var(--a-text-faint); }
.json-state.template { color: var(--a-primary); }
.json-state.invalid { color: var(--a-error); }
.json-editor-frame { position: relative; flex: 1 1 auto; min-height: 180px; overflow: hidden; }
.json-editor-frame.invalid { box-shadow: inset 0 -2px var(--a-error); }
.json-editor-host { width: 100%; height: 100%; }
.json-placeholder {
  position: absolute;
  top: 10px;
  left: 42px;
  color: var(--a-text-faint);
  font: var(--a-font-result) / var(--a-lh-result) var(--a-font-editor);
  pointer-events: none;
  white-space: pre;
}
.json-error, .json-help {
  flex: 0 0 auto;
  margin: 0;
  padding: 7px 10px;
  border-top: 1px solid var(--a-border);
  font-size: 11px;
  line-height: 1.45;
}
.json-error { color: var(--a-error); background: color-mix(in srgb, var(--a-error) 7%, var(--a-bg-2)); }
.json-help { color: var(--a-text-faint); background: var(--a-bg-2); }
.json-help code { color: var(--a-primary); font-family: var(--a-font-editor); }
.variable-menu { min-width: 238px; max-height: 248px; overflow: auto; padding: 4px; }
.variable-menu button {
  display: grid;
  grid-template-columns: minmax(82px, .8fr) minmax(118px, 1.2fr);
  align-items: center;
  gap: 10px;
  width: 100%;
  min-height: 32px;
  padding: 4px 6px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text);
  text-align: left;
  cursor: pointer;
}
.variable-menu button:hover { background: var(--a-hover); }
.variable-menu code, .variable-menu span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.variable-menu code { color: var(--a-primary); font: 11px var(--a-font-editor); }
.variable-menu span { color: var(--a-text-faint); font: 11px var(--a-font-editor); }
.variable-menu span.invalid { color: var(--a-error); }
</style>
