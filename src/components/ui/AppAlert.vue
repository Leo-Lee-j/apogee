<script setup lang="ts">
import { NAlert } from 'naive-ui'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{
  showIcon?: boolean
  title?: string
  type?: 'default' | 'success' | 'warning' | 'error' | 'info'
}>(), {
  showIcon: false,
  type: 'default',
})
</script>

<template>
  <NAlert
    class="app-alert"
    v-bind="$attrs"
    :show-icon="props.showIcon"
    :title="props.title"
    :type="props.type"
  >
    <template v-if="$slots.icon" #icon>
      <slot name="icon" />
    </template>
    <slot />
  </NAlert>
</template>
