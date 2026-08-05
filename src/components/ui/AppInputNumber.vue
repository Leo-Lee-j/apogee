<script setup lang="ts">
import { NInputNumber } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'small' })
</script>

<template>
  <NInputNumber class="app-input-number" v-bind="$attrs" :size="props.size" />
</template>
