<script setup lang="ts">
import { NSelect } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'medium' })
</script>

<template>
  <NSelect class="app-select" v-bind="$attrs" :size="props.size" />
</template>
