<script setup lang="ts">
import { NInput } from 'naive-ui'

type ControlSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'small' })
</script>

<template>
  <NInput class="app-input" v-bind="$attrs" :size="props.size" />
</template>
