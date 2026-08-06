<script setup lang="ts">
import { NPopover } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NPopover class="app-popover" v-bind="$attrs">
    <template #trigger>
      <slot name="trigger" />
    </template>
    <slot />
  </NPopover>
</template>
