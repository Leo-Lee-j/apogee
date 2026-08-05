<script setup lang="ts">
import { NTag } from 'naive-ui'

type TagSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(
  defineProps<{ size?: TagSize; bordered?: boolean }>(),
  { size: 'small', bordered: false },
)
</script>

<template>
  <NTag class="app-tag" v-bind="$attrs" :size="props.size" :bordered="props.bordered">
    <slot />
  </NTag>
</template>
