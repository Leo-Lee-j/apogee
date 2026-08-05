<script setup lang="ts">
import { NDropdown } from 'naive-ui'

type DropdownSize = 'small' | 'medium' | 'large' | 'huge'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: DropdownSize }>(), { size: 'small' })
</script>

<template>
  <NDropdown class="app-dropdown" v-bind="$attrs" :size="props.size">
    <slot />
  </NDropdown>
</template>
