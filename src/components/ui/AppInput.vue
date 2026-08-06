<script setup lang="ts">
import { ref } from 'vue'
import { NInput, type InputInst } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'small' })
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
