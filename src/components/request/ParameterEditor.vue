<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Sortable from 'sortablejs'
import { open as openFileDialog } from '@tauri-apps/plugin-dialog'
import { normalizeError, type AppError, type KeyValue } from '@/ipc'
import { parseParameterRows, type ParameterFormat } from '@/features/requestParameters'
import { AppButton, AppCheckbox, AppIcon, AppInput, AppSelect } from '@/components/ui'
import { IconCloseCircle, IconDrag, IconFolderOpen, IconRemoveCircle } from '@/components/ui/icons'
import VariableInput from './VariableInput.vue'

type Field = 'key' | 'value' | 'description'

const props = withDefaults(defineProps<{
  rows: KeyValue[]
  allowCreate?: boolean
  showKind?: boolean
  readOnlyKey?: boolean
  allowRemove?: boolean
  pasteFormat?: ParameterFormat
  canEdit?: () => boolean
}>(), { allowRemove: true, pasteFormat: 'query' })

const emit = defineEmits<{
  appendRows: [rows: KeyValue[]]
  updateField: [index: number, field: Field, value: string]
  updateEnabled: [index: number, enabled: boolean]
  setAllEnabled: [enabled: boolean]
  remove: [index: number]
  reorder: [from: number, to: number]
  updateKind: [index: number, kind: 'text' | 'file']
}>()

const list = ref<HTMLElement | null>(null)
const pendingDelete = ref<number | null>(null)
const fileError = ref<AppError | null>(null)
const pasteError = ref('')
const emptyRow: KeyValue = { enabled: true, key: '', value: '', description: '', kind: 'text' }
// 新增行和提交后的行复用同一 DOM；首个输入即入 store，快捷键发送无需先失焦。
const displayedRows = computed(() => props.allowCreate ? [...props.rows, emptyRow] : props.rows)
let sortable: Sortable | null = null
let mounted = true
const canEdit = () => mounted && (props.canEdit?.() ?? true)

const enabledCount = computed(() => props.rows.filter((row) => row.enabled).length)

function reorder(from: number, to: number) {
  if (!canEdit() || from === to || to < 0 || to >= props.rows.length) return
  emit('reorder', from, to)
}

function handleKeydown(index: number, event: KeyboardEvent) {
  if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return
  event.preventDefault()
  reorder(index, index + (event.key === 'ArrowUp' ? -1 : 1))
}

function confirmRemove(index: number) {
  if (!canEdit() || !props.allowRemove) return
  if (pendingDelete.value === index) {
    emit('remove', index)
    pendingDelete.value = null
  } else {
    pendingDelete.value = index
  }
}

async function chooseFile(index: number) {
  const row = props.rows[index]
  if (!canEdit() || !row) return
  fileError.value = null
  try {
    const selected = await openFileDialog({ multiple: false })
    // 对话框返回时可能已经切换请求、删除/编辑了原行或重新排序。
    const currentIndex = props.rows.indexOf(row)
    if (canEdit() && currentIndex >= 0 && typeof selected === 'string') {
      emit('updateField', currentIndex, 'value', selected)
    }
  } catch (error) {
    if (canEdit() && props.rows.includes(row)) fileError.value = {
      ...normalizeError(error), hints: [{ label: '重试选择文件，或手动填写本地文件路径' }],
    }
  }
}

function updateField(index: number, field: Field, value: string) {
  if (!canEdit() || (field === 'key' && props.readOnlyKey)) return
  if (index < props.rows.length) emit('updateField', index, field, value)
  else if (props.allowCreate && value) emit('appendRows', [{ ...emptyRow, [field]: value }])
}

function updateKind(index: number, kind: 'text' | 'file') {
  if (!canEdit()) return
  if (index < props.rows.length) emit('updateKind', index, kind)
  else if (props.allowCreate) emit('appendRows', [{ ...emptyRow, kind }])
}

function handlePaste(index: number, event: ClipboardEvent) {
  if (!canEdit() || !props.allowCreate || props.readOnlyKey || index !== props.rows.length) return
  const text = event.clipboardData?.getData('text/plain') ?? ''
  if (!/[\n\t=&]/.test(text) && !text.startsWith('?')) return
  event.preventDefault()
  const parsed = parseParameterRows(text, [], props.pasteFormat)
  pasteError.value = parsed.errors.map(error => `第 ${error.line} 行：${error.message}`).join('；')
  if (!parsed.errors.length && parsed.rows.length) emit('appendRows', parsed.rows)
}

onMounted(() => {
  if (!list.value) return
  sortable = Sortable.create(list.value, {
    animation: 140,
    handle: '.parameter-drag-handle',
    draggable: '.parameter-row:not(.parameter-row--draft)',
    forceFallback: true,
    fallbackOnBody: true,
    fallbackTolerance: 4,
    ghostClass: 'parameter-row--ghost',
    chosenClass: 'parameter-row--chosen',
    dragClass: 'parameter-row--dragging',
    onEnd: ({ item, oldDraggableIndex, newDraggableIndex }) => {
      if (oldDraggableIndex == null || newDraggableIndex == null) return
      // Sortable 已移动 DOM，先恢复 Vue 看到的旧顺序，再让响应式渲染应用新顺序。
      if (list.value) {
        item.remove()
        list.value.insertBefore(item, list.value.children[oldDraggableIndex] ?? null)
      }
      reorder(oldDraggableIndex, newDraggableIndex)
    },
  })
})

onBeforeUnmount(() => { mounted = false; sortable?.destroy() })

watch(() => props.rows, () => { pendingDelete.value = null })
</script>

<template>
  <div class="parameter-editor" :class="{ 'parameter-editor--with-kind': showKind }">
    <div class="parameter-header parameter-grid" role="row">
      <div class="parameter-enabled" role="columnheader">
        <AppCheckbox
          v-if="rows.length"
          :checked="enabledCount === rows.length"
          :indeterminate="enabledCount > 0 && enabledCount < rows.length"
          :aria-label="enabledCount === rows.length ? '禁用全部参数' : '启用全部参数'"
          @update:checked="canEdit() && emit('setAllEnabled', $event)"
        />
      </div>
      <div role="columnheader">参数名</div>
      <div role="columnheader">参数值</div>
      <div v-if="showKind" role="columnheader">类型</div>
      <div role="columnheader">说明</div>
      <div aria-hidden="true" />
    </div>

    <div ref="list" class="parameter-rows" role="rowgroup">
      <div
        v-for="(row, index) in displayedRows"
        :key="`row-${index}`"
        class="parameter-row parameter-grid"
        :class="{ 'parameter-row--off': !row.enabled, 'parameter-row--draft': index === rows.length }"
        role="row"
      >
        <div class="parameter-enabled" role="cell">
          <button
            v-if="index < rows.length"
            type="button"
            class="parameter-drag-handle"
            title="拖动调整顺序；Alt + 上下方向键也可排序"
            aria-label="拖动调整参数顺序"
            @keydown="handleKeydown(index, $event)"
          >
            <AppIcon :component="IconDrag" :size="15" />
          </button>
          <AppCheckbox
            v-if="index < rows.length"
            :checked="row.enabled"
            :aria-label="`启用 ${row.key || '空行'}`"
            @update:checked="canEdit() && emit('updateEnabled', index, $event)"
          />
        </div>
        <div role="cell">
          <VariableInput
            :value="row.key"
            size="small"
            :bordered="true"
            :readonly="readOnlyKey"
            :variables="!readOnlyKey"
            :title="readOnlyKey ? '参数名由 URL 生成，请在地址栏修改占位符' : undefined"
            :input-props="{ spellcheck: false, 'aria-label': '参数名' }"
            :placeholder="index === rows.length ? '输入参数名' : undefined"
            @update:value="updateField(index, 'key', $event)"
            @paste="handlePaste(index, $event)"
          />
        </div>
        <div role="cell">
          <VariableInput
            :value="row.value"
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false, 'aria-label': '参数值' }"
            :placeholder="showKind && row.kind === 'file' ? '本地文件路径' : undefined"
            @update:value="updateField(index, 'value', $event)"
          />
          <AppButton
            v-if="showKind && row.kind === 'file'"
            quaternary
            circle
            size="tiny"
            title="选择文件"
            aria-label="选择文件"
            @click="chooseFile(index)"
          >
            <template #icon><AppIcon :component="IconFolderOpen" :size="15" /></template>
          </AppButton>
        </div>
        <div v-if="showKind" role="cell">
          <AppSelect
            :value="row.kind === 'file' ? 'file' : 'text'"
            size="small"
            :options="[{ label: '文本', value: 'text' }, { label: '文件', value: 'file' }]"
            @update:value="updateKind(index, $event as 'text' | 'file')"
          />
        </div>
        <div role="cell">
          <AppInput
            :value="row.description ?? ''"
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false, 'aria-label': '参数说明' }"
            @update:value="updateField(index, 'description', $event)"
          />
        </div>
        <div class="parameter-actions" role="cell">
          <AppButton
            v-if="index < rows.length && allowRemove"
            quaternary
            circle
            size="tiny"
            :type="pendingDelete === index ? 'error' : 'default'"
            :title="pendingDelete === index ? '再次点击确认删除' : '删除参数'"
            :aria-label="pendingDelete === index ? '确认删除参数' : '删除参数'"
            @click="confirmRemove(index)"
          >
            <template #icon>
              <AppIcon :component="pendingDelete === index ? IconCloseCircle : IconRemoveCircle" :size="17" />
            </template>
          </AppButton>
        </div>
      </div>

    </div>
    <p v-if="pasteError" class="parameter-error" role="alert">{{ pasteError }}</p>
    <div v-if="fileError" class="parameter-error" role="alert">
      {{ fileError.message }}
      <span v-for="hint in fileError.hints" :key="hint.label">{{ hint.label }}</span>
    </div>
  </div>
</template>

<style scoped>
.parameter-error { color: var(--a-error); margin: 6px 0; font-size: var(--a-fs-xs); }
.parameter-error span { display: block; }
.parameter-editor { min-width: 630px; background: transparent; }
.parameter-grid {
  display: grid;
  grid-template-columns: 40px minmax(150px, 1fr) minmax(180px, 1.2fr) minmax(180px, 1fr) 40px;
  align-items: center;
}
.parameter-editor--with-kind .parameter-grid {
  grid-template-columns: 40px minmax(130px, 1fr) minmax(160px, 1.1fr) 100px minmax(150px, 1fr) 40px;
}
.parameter-header {
  min-height: max(34px, var(--a-h-control-sm));
  color: var(--a-text-faint);
  font-size: var(--a-fs-xs);
  line-height: var(--a-lh-xs);
  border-bottom: 1px solid var(--a-border);
}
.parameter-row {
  position: relative;
  min-height: max(38px, var(--a-h-input-sm));
  background: transparent;
  border-bottom: 1px solid var(--a-border);
  transition: opacity 120ms ease, background-color 120ms ease;
}
.parameter-row:last-child { border-bottom: 0; }
.parameter-row > div { min-width: 0; box-sizing: border-box; padding: 0 6px 0 0; }
.parameter-editor--with-kind .parameter-row > :nth-child(3) { display: flex; align-items: center; gap: 2px; }
.parameter-editor--with-kind .parameter-row > :nth-child(3) :deep(.n-input) { flex: 1 1 auto; min-width: 0; }
.parameter-row--off { opacity: 0.48; }
.parameter-row--draft { opacity: 0.72; }
.parameter-row--chosen { background: var(--a-bg-3); }
.parameter-row--ghost { opacity: 0.3; }
.parameter-row--dragging { box-shadow: 0 4px 14px rgb(0 0 0 / 18%); }
.parameter-enabled { display: flex; align-items: center; justify-content: center; padding: 0 !important; }
.parameter-drag-handle {
  position: absolute;
  left: -6px;
  top: calc((100% - var(--a-h-input-sm)) / 2);
  width: 16px;
  height: var(--a-h-input-sm);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  color: var(--a-text-faint);
  background: transparent;
  border: 0;
  cursor: grab;
  opacity: 0;
  touch-action: none;
}
.parameter-row:hover .parameter-drag-handle,
.parameter-drag-handle:focus-visible { opacity: 1; }
.parameter-drag-handle:hover { color: var(--a-text); }
.parameter-drag-handle:active { cursor: grabbing; }
.parameter-actions { display: flex; align-items: center; justify-content: center; }
.parameter-row :deep(.n-input) {
  --n-color: transparent;
  --n-color-focus: transparent;
  --n-border: transparent;
  --n-border-hover: transparent;
  --n-padding-left: 0px;
  --n-padding-right: 0px;
  background: transparent;
}
.parameter-row :deep(.n-input__input-el),
.parameter-row :deep(.n-input__textarea-el) {
  font-family: var(--a-font-editor);
  font-size: var(--a-fs-editor-sm);
  line-height: var(--a-lh-sm);
}
.parameter-row :deep(.n-input__border) { border-color: transparent !important; }
.parameter-row :deep(.n-input),
.parameter-row :deep(.n-input__input-el) { height: max(32px, var(--a-h-input-sm)); min-height: max(32px, var(--a-h-input-sm)); }
</style>
