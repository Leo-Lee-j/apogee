<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import Sortable from 'sortablejs'
import { open as openFileDialog } from '@tauri-apps/plugin-dialog'
import type { KeyValue } from '@/ipc'
import { AppButton, AppCheckbox, AppIcon, AppInput, AppSelect } from '@/components/ui'
import { IconCloseCircle, IconDrag, IconFolderOpen, IconRemoveCircle } from '@/components/ui/icons'
import VariableInput from './VariableInput.vue'

type Field = 'key' | 'value' | 'description'

const props = defineProps<{
  rows: KeyValue[]
  allowCreate?: boolean
  showKind?: boolean
}>()

const emit = defineEmits<{
  updateField: [index: number, field: Field, value: string]
  updateEnabled: [index: number, enabled: boolean]
  setAllEnabled: [enabled: boolean]
  remove: [index: number]
  reorder: [from: number, to: number]
  updateKind: [index: number, kind: 'text' | 'file']
}>()

const list = ref<HTMLElement | null>(null)
const pendingDelete = ref<number | null>(null)
const draft = reactive({ key: '', value: '', description: '' })
let sortable: Sortable | null = null

const enabledCount = computed(() => props.rows.filter((row) => row.enabled).length)

function reorder(from: number, to: number) {
  if (from === to || to < 0 || to >= props.rows.length) return
  emit('reorder', from, to)
}

function handleKeydown(index: number, event: KeyboardEvent) {
  if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return
  event.preventDefault()
  reorder(index, index + (event.key === 'ArrowUp' ? -1 : 1))
}

function confirmRemove(index: number) {
  if (pendingDelete.value === index) {
    emit('remove', index)
    pendingDelete.value = null
  } else {
    pendingDelete.value = index
  }
}

async function chooseFile(index: number) {
  const selected = await openFileDialog({ multiple: false })
  if (typeof selected === 'string') emit('updateField', index, 'value', selected)
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
    onEnd: ({ oldDraggableIndex, newDraggableIndex }) => {
      if (oldDraggableIndex == null || newDraggableIndex == null) return
      reorder(oldDraggableIndex, newDraggableIndex)
    },
  })
})

onBeforeUnmount(() => sortable?.destroy())
function commitDraft() {
  if (!draft.key && !draft.value && !draft.description) return
  const index = props.rows.length
  if (draft.key) emit('updateField', index, 'key', draft.key)
  if (draft.value) emit('updateField', index, 'value', draft.value)
  if (draft.description) emit('updateField', index, 'description', draft.description)
  draft.key = ''
  draft.value = ''
  draft.description = ''
}

function commitDraftOnFocusout(event: FocusEvent) {
  const row = event.currentTarget as HTMLElement
  const next = event.relatedTarget as Node | null
  if (!next || !row.contains(next)) commitDraft()
}

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
          @update:checked="emit('setAllEnabled', $event)"
        />
      </div>
      <div role="columnheader">参数名</div>
      <div role="columnheader">参数值</div>
      <div role="columnheader">说明</div>
      <div v-if="showKind" role="columnheader">类型</div>
      <div aria-hidden="true" />
    </div>

    <div ref="list" class="parameter-rows" role="rowgroup">
      <div
        v-for="(row, index) in rows"
        :key="`row-${index}`"
        class="parameter-row parameter-grid"
        :class="{ 'parameter-row--off': !row.enabled }"
        role="row"
      >
        <div class="parameter-enabled" role="cell">
          <button
            type="button"
            class="parameter-drag-handle"
            title="拖动调整顺序；Alt + 上下方向键也可排序"
            aria-label="拖动调整参数顺序"
            @keydown="handleKeydown(index, $event)"
          >
            <AppIcon :component="IconDrag" :size="15" />
          </button>
          <AppCheckbox
            :checked="row.enabled"
            :aria-label="`启用 ${row.key || '空行'}`"
            @update:checked="emit('updateEnabled', index, $event)"
          />
        </div>
        <div role="cell">
          <VariableInput
            :value="row.key"
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            @update:value="emit('updateField', index, 'key', $event)"
          />
        </div>
        <div role="cell">
          <VariableInput
            :value="row.value"
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            :placeholder="showKind && row.kind === 'file' ? '本地文件路径' : undefined"
            @update:value="emit('updateField', index, 'value', $event)"
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
            :value="row.kind ?? 'text'"
            size="small"
            :options="[{ label: '文本', value: 'text' }, { label: '文件', value: 'file' }]"
            @update:value="emit('updateKind', index, $event as 'text' | 'file')"
          />
        </div>
        <div role="cell">
          <AppInput
            :value="row.description ?? ''"
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            @update:value="emit('updateField', index, 'description', $event)"
          />
        </div>
        <div class="parameter-actions" role="cell">
          <AppButton
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

      <div
        v-if="allowCreate"
        :key="'draft'"
        class="parameter-row parameter-row--draft parameter-grid"
        role="row"
        @focusout="commitDraftOnFocusout"
      >
        <div role="cell" />
        <div role="cell">
          <VariableInput
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            placeholder="输入参数名"
            :value="draft.key"
            @update:value="draft.key = $event"
            @keydown.enter.prevent="commitDraft"
          />
        </div>
        <div role="cell">
          <VariableInput
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            :value="draft.value"
            @update:value="draft.value = $event"
            @keydown.enter.prevent="commitDraft"
          />
        </div>
        <div v-if="showKind" role="cell">
          <AppSelect
            value="text"
            size="small"
            :options="[{ label: '文本', value: 'text' }, { label: '文件', value: 'file' }]"
            @update:value="emit('updateKind', props.rows.length, $event as 'text' | 'file')"
          />
        </div>
        <div role="cell">
          <AppInput
            size="small"
            :bordered="true"
            :input-props="{ spellcheck: false }"
            :value="draft.description"
            @update:value="draft.description = $event"
            @keydown.enter.prevent="commitDraft"
          />
        </div>
        <div role="cell" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.parameter-editor { min-width: 630px; background: transparent; }
.parameter-grid {
  display: grid;
  grid-template-columns: 40px minmax(150px, 1fr) minmax(180px, 1.2fr) minmax(180px, 1fr) 40px;
  align-items: center;
}
.parameter-editor--with-kind .parameter-grid {
  grid-template-columns: 40px minmax(130px, 1fr) minmax(160px, 1.1fr) 100px minmax(150px, 1fr) 40px;
}
.parameter-editor--with-kind .parameter-header > :nth-child(4) { grid-column: 5; }
.parameter-editor--with-kind .parameter-header > :nth-child(5) { grid-column: 4; }
.parameter-header {
  min-height: 34px;
  color: var(--a-text-faint);
  font-size: 11px;
  border-bottom: 1px solid var(--a-border);
}
.parameter-row {
  position: relative;
  min-height: 38px;
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
  left: -16px;
  top: 4px;
  width: 16px;
  height: 30px;
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
.parameter-row :deep(.n-input__border) { border-color: transparent !important; }
.parameter-row :deep(.n-input),
.parameter-row :deep(.n-input__input-el) { height: 32px; min-height: 32px; }
</style>
