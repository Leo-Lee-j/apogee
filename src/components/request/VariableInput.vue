<script setup lang="ts">
import { computed, nextTick, ref, useAttrs, watch } from 'vue'
import { NInput, NPopover, type InputInst } from 'naive-ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { resolveVariablePreview } from '@/features/variablePreview'

defineOptions({ inheritAttrs: false })

const props = withDefaults(defineProps<{
  value: string
  multiline?: boolean
  variables?: boolean
  size?: 'small' | 'medium' | 'large'
}>(), {
  multiline: false,
  variables: true,
  size: 'medium',
})

const emit = defineEmits<{
  'update:value': [value: string]
}>()

const attrs = useAttrs()
const workspace = useWorkspaceStore()
const input = ref<InputInst | null>(null)
const open = ref(false)
const activeIndex = ref(0)
const tokenStart = ref(-1)
const tokenEnd = ref(-1)
const query = ref('')
const currentValue = ref(props.value)

const variables = computed(() => {
  if (!props.variables) return []
  const environment = workspace.activeEnvironment
  if (!environment) return []

  const items = environment.variables
    .filter((variable) => variable.enabled && variable.key.trim())
    .map((variable) => ({ key: variable.key.trim(), value: variable.value }))
  if (environment.host.trim()) items.unshift({ key: 'host', value: environment.host.trim() })
  return items.map((variable) => ({ ...variable, preview: resolveVariablePreview(variable.key, items) }))
})

const matches = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase()
  const items = needle
    ? variables.value.filter((variable) => variable.key.toLocaleLowerCase().includes(needle))
    : variables.value
  return items.slice(0, 12)
})

const inputAttrs = computed(() => {
  const { class: _class, style: _style, onKeydown: _onKeydown, onFocus: _onFocus, onBlur: _onBlur, onClick: _onClick, ...rest } = attrs
  return rest
})

/**
 * 父级经 attrs 传入的事件监听器不能走 v-bind：组件自身也绑了 @blur/@focus/@click，
 * Vue 会把两边的 onX 合并成数组，而 naive-ui Popover 的 appendEvents 只认单个函数，
 * 遇到数组会把数组本身当 handler 包装，失焦时抛「originalHandler is not a function」。
 * 所以这些事件从 inputAttrs 里剥掉，统一在这里手动转发。
 */
function forwardHandler(name: string, event: unknown) {
  const listener = (attrs as Record<string, unknown>)[name]
  if (Array.isArray(listener)) {
    for (const handler of listener) {
      if (typeof handler === 'function') handler(event)
    }
  } else if (typeof listener === 'function') {
    listener(event)
  }
}

function nativeInput(): HTMLInputElement | HTMLTextAreaElement | null {
  return input.value?.inputElRef ?? input.value?.textareaElRef ?? null
}

function refresh(value = currentValue.value, caretOverride?: number) {
  const element = nativeInput()
  const caret = caretOverride ?? element?.selectionStart ?? value.length
  const before = value.slice(0, caret)
  const start = before.lastIndexOf('{{')
  const close = before.lastIndexOf('}}')

  if (start < 0 || close > start) {
    open.value = false
    return
  }

  const fragment = before.slice(start + 2)
  if (fragment.includes('{') || fragment.includes('}') || /\s/.test(fragment)) {
    open.value = false
    return
  }

  tokenStart.value = start
  tokenEnd.value = caret
  query.value = fragment
  activeIndex.value = 0
  open.value = variables.value.length > 0
}

function update(value: string) {
  const element = nativeInput()
  const caret = element?.selectionStart ?? value.length
  currentValue.value = value
  emit('update:value', value)
  void nextTick(() => refresh(value, caret))
}

function select(key: string) {
  if (tokenStart.value < 0) return
  const replacement = `{{${key}}}`
  const value = currentValue.value.slice(0, tokenStart.value) + replacement + currentValue.value.slice(tokenEnd.value)
  const caret = tokenStart.value + replacement.length
  currentValue.value = value
  tokenStart.value = -1
  tokenEnd.value = -1
  open.value = false
  emit('update:value', value)
  void nextTick(() => {
    const element = nativeInput()
    input.value?.focus()
    element?.setSelectionRange(caret, caret)
    window.requestAnimationFrame(() => nativeInput()?.setSelectionRange(caret, caret))
  })
}

function keydown(event: KeyboardEvent) {
  if (!open.value || matches.value.length === 0) {
    forwardHandler('onKeydown', event)
    return
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    event.stopPropagation()
    const direction = event.key === 'ArrowDown' ? 1 : -1
    activeIndex.value = (activeIndex.value + direction + matches.value.length) % matches.value.length
  } else if (event.key === 'Enter' || event.key === 'Tab') {
    event.preventDefault()
    event.stopPropagation()
    const variable = matches.value[activeIndex.value]
    if (variable) select(variable.key)
  } else if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    open.value = false
  } else {
    forwardHandler('onKeydown', event)
  }
}

watch(matches, (items) => {
  if (activeIndex.value >= items.length) activeIndex.value = 0
})

watch(() => props.value, (value) => {
  currentValue.value = value
})

defineExpose({
  focus: () => input.value?.focus(),
  blur: () => input.value?.blur(),
  select: () => input.value?.select(),
})
</script>

<template>
  <NPopover
    class="variable-popover"
    trigger="manual"
    placement="bottom-start"
    :show="open && matches.length > 0"
    :show-arrow="false"
    :width="280"
    :content-style="{ padding: '0', background: 'transparent', boxShadow: 'none', border: '0' }"
    @clickoutside="open = false"
    raw
  >
    <template #trigger>
      <NInput
        ref="input"
        v-bind="inputAttrs"
        :class="attrs.class"
        :style="attrs.style"
        :value="currentValue"
        :type="multiline ? 'textarea' : 'text'"
        :size="size"
        @update:value="update"
        @click="refresh(); forwardHandler('onClick', $event)"
        @keydown="keydown"
        @focus="forwardHandler('onFocus', $event)"
        @blur="open = false; forwardHandler('onBlur', $event)"
      />
    </template>

    <div class="variable-menu" role="listbox" aria-label="当前环境变量">
      <button
        v-for="(variable, index) in matches"
        :key="variable.key"
        class="variable-option"
        :class="{ active: index === activeIndex }"
        type="button"
        role="option"
        :aria-selected="index === activeIndex"
        @mouseenter="activeIndex = index"
        @mousedown.prevent="select(variable.key)"
      >
        <span class="variable-name">{{ variable.key }}</span>
        <span class="variable-value" :class="{ invalid: variable.preview.error }" :title="variable.preview.error || variable.preview.value">
          {{ variable.preview.error || variable.preview.value || '空值' }}
        </span>
      </button>
    </div>
  </NPopover>
</template>

<style scoped>
:global(.variable-popover.n-popover-shared),
:global(.variable-popover.n-popover--raw) {
  padding: 0 !important;
  border: 0 !important;
  background: transparent !important;
  box-shadow: none !important;
}
.variable-menu {
  overflow: auto;
  max-height: 260px;
  padding: 4px;
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
  box-shadow: 0 8px 24px rgb(0 0 0 / 22%);
}
.variable-option {
  display: grid;
  grid-template-columns: minmax(88px, 1fr) minmax(110px, 1.35fr);
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
.variable-option.active { background: var(--a-selected); }
.variable-name {
  overflow: hidden;
  padding: 3px 7px;
  border: 1px solid color-mix(in srgb, var(--a-primary) 28%, var(--a-border));
  border-radius: var(--a-radius-sm);
  background: color-mix(in srgb, var(--a-primary) 8%, var(--a-bg-3));
  color: var(--a-primary-hover);
  font: 12px/18px var(--a-font-mono);
  font-weight: var(--a-fw-medium);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.variable-value {
  overflow: hidden;
  color: var(--a-text-faint);
  font: 11px var(--a-font-mono);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.variable-value.invalid { color: var(--a-error); }
</style>
