<script setup lang="ts">
import { NTooltip } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NTooltip v-bind="$attrs">
    <template #trigger><slot name="trigger" /></template>
    <slot />
  </NTooltip>
</template>
