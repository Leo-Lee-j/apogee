<script setup lang="ts">
import { computed, ref, useSlots } from 'vue'
import { useUiStore, SPLIT_DEFAULT_RATIO, SPLIT_MAX_RATIO, SPLIT_MIN_RATIO, type LayoutMode } from '@/stores/ui'
import WorkspacePanel from './WorkspacePanel.vue'

const props = withDefaults(defineProps<{
  split?: boolean
  useGlobalLayout?: boolean
  layout?: Exclude<LayoutMode, 'focus'>
  initialRatio?: number
  panelPadding?: string | number
}>(), { split: false, useGlobalLayout: false, layout: 'side-by-side', initialRatio: SPLIT_DEFAULT_RATIO })

const slots = useSlots()
const ui = useUiStore()
const localRatio = ref(props.initialRatio)
const dragging = ref(false)
const hasSecondary = computed(() => props.split && Boolean(slots.secondary))
const activeLayout = computed<LayoutMode>(() => props.useGlobalLayout ? ui.layout : props.layout)
const ratio = computed(() => props.useGlobalLayout ? ui.requestRatio : localRatio.value)
const direction = computed(() => activeLayout.value === 'side-by-side' ? 'row' : 'column')
const primaryFlex = computed(() => activeLayout.value === 'focus' ? '0 0 0' : `${ratio.value} 1 0`)
const secondaryFlex = computed(() => activeLayout.value === 'focus' ? '1 1 0' : `${1 - ratio.value} 1 0`)

function setRatio(value: number) {
  const next = Math.min(SPLIT_MAX_RATIO, Math.max(SPLIT_MIN_RATIO, value))
  if (props.useGlobalLayout) ui.setRequestRatio(next)
  else localRatio.value = next
}
function startResize(event: PointerEvent) {
  if (event.button !== 0) return
  const divider = event.currentTarget as HTMLElement
  const container = divider.parentElement
  if (!container) return
  const vertical = direction.value === 'row'
  const rect = container.getBoundingClientRect()
  divider.setPointerCapture(event.pointerId)
  dragging.value = true
  const move = (moveEvent: PointerEvent) => {
    const offset = vertical ? moveEvent.clientX - rect.left : moveEvent.clientY - rect.top
    setRatio(offset / (vertical ? rect.width : rect.height))
  }
  const end = () => {
    dragging.value = false
    divider.removeEventListener('pointermove', move)
    divider.removeEventListener('pointerup', end)
    divider.removeEventListener('pointercancel', end)
  }
  divider.addEventListener('pointermove', move)
  divider.addEventListener('pointerup', end)
  divider.addEventListener('pointercancel', end)
  event.preventDefault()
}
function resizeWithKey(event: KeyboardEvent) {
  const step = event.shiftKey ? 0.05 : 0.02
  const horizontal = direction.value === 'row'
  if ((horizontal && event.key === 'ArrowLeft') || (!horizontal && event.key === 'ArrowUp')) setRatio(ratio.value - step)
  else if ((horizontal && event.key === 'ArrowRight') || (!horizontal && event.key === 'ArrowDown')) setRatio(ratio.value + step)
  else if (event.key === 'Home') setRatio(SPLIT_MIN_RATIO)
  else if (event.key === 'End') setRatio(SPLIT_MAX_RATIO)
  else if (event.key === 'Enter' || event.key === ' ') setRatio(SPLIT_DEFAULT_RATIO)
  else return
  event.preventDefault()
}
</script>

<template>
  <WorkspacePanel v-if="!hasSecondary" :panel-padding="props.panelPadding"><slot /></WorkspacePanel>
  <div v-else class="workspace-content" :class="{ dragging, vertical: direction === 'row' }" :style="{ flexDirection: direction }">
    <WorkspacePanel :collapsed="activeLayout === 'focus'" :panel-padding="props.panelPadding" :style="{ flex: primaryFlex }"><slot name="primary" /></WorkspacePanel>
    <div v-if="activeLayout !== 'focus'" class="splitter" role="separator" :aria-orientation="direction === 'row' ? 'vertical' : 'horizontal'" tabindex="0" @pointerdown="startResize" @keydown="resizeWithKey" @dblclick="setRatio(SPLIT_DEFAULT_RATIO)" />
    <WorkspacePanel :panel-padding="props.panelPadding" :style="{ flex: secondaryFlex }"><slot name="secondary" /></WorkspacePanel>
  </div>
</template>

<style scoped>
.workspace-content { flex: 1 1 auto; min-width: 0; min-height: 0; display: flex; gap: 0; }
.splitter { flex: 0 0 3px; position: relative; z-index: 2; touch-action: none; cursor: row-resize; }
.splitter.vertical { cursor: col-resize; }
.workspace-content.vertical .splitter { cursor: col-resize; }
.splitter::after { content: ''; position: absolute; inset: 0; background: transparent; transition: background var(--a-dur) var(--a-ease); }
.splitter:hover::after, .splitter:focus-visible::after, .workspace-content.dragging .splitter::after { background: var(--a-primary); }
</style>
