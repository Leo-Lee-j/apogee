<script setup lang="ts">
import { NPopconfirm } from 'naive-ui'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ showIcon?: boolean }>(), { showIcon: false })
</script>

<template>
  <NPopconfirm class="app-popconfirm" v-bind="$attrs" :show-icon="props.showIcon">
    <template #trigger>
      <slot name="trigger" />
    </template>
    <slot />
  </NPopconfirm>
</template>
