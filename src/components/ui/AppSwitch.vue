<script setup lang="ts">
import { NSwitch } from 'naive-ui'

type SwitchSize = 'small' | 'medium'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: SwitchSize }>(), { size: 'small' })
</script>

<template>
  <NSwitch class="app-switch" v-bind="$attrs" :size="props.size" />
</template>
