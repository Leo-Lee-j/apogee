<script setup lang="ts">
import { NIcon } from 'naive-ui'
import type { Component } from 'vue'

defineOptions({ inheritAttrs: false })

withDefaults(
  defineProps<{
    component: Component
    size?: number | string
  }>(),
  { size: 16 },
)
</script>

<template>
  <NIcon class="app-icon" :size="size" v-bind="$attrs">
    <component :is="component" />
  </NIcon>
</template>
