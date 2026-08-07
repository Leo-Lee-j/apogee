<script setup lang="ts">
import { ref } from 'vue'
import { NInput, type InputInst } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'medium' })
const input = ref<InputInst | null>(null)

defineExpose({
  focus: () => input.value?.focus(),
  blur: () => input.value?.blur(),
  select: () => input.value?.select(),
})
</script>

<template>
  <NInput ref="input" class="app-input" v-bind="$attrs" :size="props.size" />
</template>

<style scoped>
.app-input :deep(.n-input-wrapper) { align-items: center; }
.app-input :deep(.n-input__input-el) {
  height: 100%;
  padding-block: 0;
  line-height: 20px;
}
</style>
