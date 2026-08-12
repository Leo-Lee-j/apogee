<script setup lang="ts">
import { computed, ref } from 'vue'
import { NInput, type InputInst } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize; width?: string }>(), { size: 'medium' })
const input = ref<InputInst | null>(null)

/** 与 theme.ts Input.height* 对齐，避免表单/flex 布局把输入框压扁。 */
const sizeHeight = computed(() => {
  if (props.size === 'small') return '30px'
  if (props.size === 'large') return '40px'
  return '36px'
})

defineExpose({
  focus: () => input.value?.focus(),
  blur: () => input.value?.blur(),
  select: () => input.value?.select(),
})
</script>

<template>
  <NInput
    ref="input"
    class="app-input"
    v-bind="$attrs"
    :size="props.size"
    :style="{ '--app-input-height': sizeHeight, ...(props.width ? { width: props.width, maxWidth: '100%' } : {}) }"
  />
</template>

<style scoped>
/* class 挂在 NInput 根节点（.n-input）上，高度直接钉在根上。 */
.app-input {
  --app-input-height: 36px;
  height: var(--app-input-height) !important;
  min-height: var(--app-input-height) !important;
}
.app-input :deep(.n-input-wrapper) {
  align-items: center;
  height: 100%;
  min-height: var(--app-input-height);
}
.app-input :deep(.n-input__input-el) {
  height: 100%;
  min-height: calc(var(--app-input-height) - 2px);
  padding-block: 0;
  line-height: 20px;
}
</style>
