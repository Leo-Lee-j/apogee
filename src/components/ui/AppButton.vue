<script setup lang="ts">
import { NButton } from 'naive-ui'

type ControlSize = 'tiny' | 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: ControlSize }>(), { size: 'medium' })
</script>

<template>
  <NButton class="app-button" v-bind="$attrs" :size="props.size">
    <template v-if="$slots.icon" #icon>
      <slot name="icon" />
    </template>
    <slot />
  </NButton>
</template>
