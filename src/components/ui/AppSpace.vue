<script setup lang="ts">
import { NSpace } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NSpace class="app-space" v-bind="$attrs">
    <slot />
  </NSpace>
</template>
