<script setup lang="ts">
import { NSelect } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize; width?: string }>(), { size: 'medium' })
</script>

<template>
  <NSelect class="app-select" v-bind="$attrs" :size="props.size" :style="props.width ? { width: props.width, maxWidth: '100%' } : undefined" />
</template>
